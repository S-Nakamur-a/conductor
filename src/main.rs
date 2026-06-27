//! Conductor — a terminal-based Git workspace and code review tool.

mod ai_caller;
mod app;
mod background;
mod cc_notify;
mod ccusage_cache;
mod claude_sessions;
mod command_palette;
mod config;
mod config_watcher;
mod diff_state;
mod event;
mod file_watcher;
mod gemini_api;
mod git_engine;
mod grep_search;
mod jump_history;
mod keymap;
mod media_state;
mod overlay;
mod pty_manager;
mod refresh_pipe;
mod review_state;
mod review_store;
mod search_result_tree;
mod symbol_index;
mod term_caps;
mod terminal_link;
mod terminal_state;
mod text_input;
mod theme;
mod timer;
mod ui;
mod update_checker;
mod viewer;
mod worktree_ops;

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags, poll as crossterm_poll, read as crossterm_read,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::app::App;
use crate::event::{handle_key_event, handle_mouse_event, handle_paste_event};
use crate::ui::layout::render_ui;

/// Tick rate when terminal panels are focused (~120fps for responsive PTY).
const TICK_RATE_TERMINAL: Duration = Duration::from_millis(8);
/// Tick rate right after user input for responsive scrolling (~60fps).
const TICK_RATE_ACTIVE: Duration = Duration::from_millis(16);
/// Tick rate when non-terminal panels are idle (low CPU usage).
const TICK_RATE_IDLE: Duration = Duration::from_millis(500);
/// How long to keep using the active tick rate after the last input event.
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
/// Fixed interval for decoration animation updates (~10fps), independent of main tick rate.
const DECORATION_TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Interval for the "Claude is waiting" notification breathing pulse (~12fps).
/// Drives redraws while a session waits, independent of decoration/PTY activity,
/// so the pulse keeps breathing even when the user is focused elsewhere.
const PULSE_TICK_INTERVAL: Duration = Duration::from_millis(80);
/// Interval for refreshing unfocused terminal panels (~2fps).
/// Balances visibility of background PTY output with CPU usage.
const UNFOCUSED_TERMINAL_REFRESH: Duration = Duration::from_millis(500);

fn main() -> Result<()> {
    // ── Panic hook: write backtrace to ~/.config/conductor/panic.log ──
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(config_dir) = dirs::config_dir() {
                let log_dir = config_dir.join("conductor");
                let _ = std::fs::create_dir_all(&log_dir);
                let log_path = log_dir.join("panic.log");
                let bt = std::backtrace::Backtrace::force_capture();
                let payload = format!(
                    "=== Conductor panic at {} ===\n{info}\n\nBacktrace:\n{bt}\n\n",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                );
                let _ = std::fs::write(&log_path, &payload);
            }
            default_hook(info);
        }));
    }

    // ── Initialise logging (honour RUST_LOG env var) ─────────────────
    env_logger::init();

    // ── Fast-path CLI flags (must not touch the terminal) ────────────
    // `--version` also doubles as the updater's verification probe: before
    // swapping in a freshly downloaded binary, the updater spawns it with
    // `--version` and checks it exits cleanly. Keep this branch above the
    // terminal setup so it never enters raw mode or the alternate screen.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("conductor {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "conductor {}\n\nUsage: conductor [REPO_PATH]\n\n  REPO_PATH    Git repository to open (defaults to the current directory)\n\nOptions:\n  -V, --version    Print version and exit\n  -h, --help       Print this help and exit",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            _ => {}
        }
    }

    // ── Set up crossterm terminal ────────────────────────────────────
    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    log::debug!("keyboard_enhanced = {keyboard_enhanced}");
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    enter_tui(terminal.backend_mut(), keyboard_enhanced)?;

    // ── Create application state ─────────────────────────────────────
    let repo_path = match std::env::args().nth(1) {
        Some(path) => {
            let p = std::path::PathBuf::from(&path);
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir()?.join(p)
            }
        }
        None => std::env::current_dir()?,
    };
    let mut app = App::new(repo_path);

    // ── Set terminal window title ────────────────────────────────────
    let window_title = format!("conductor - {}", app.main_repo_name);
    execute!(io::stdout(), SetTitle(&window_title))?;

    // ── OSC11 background auto-detection ──────────────────────────────
    // The query must run while in raw mode and before the event loop starts
    // reading stdin (same requirement as the graphics-protocol probe below).
    // `auto_theme_for_background` handles the "only when unconfigured" guard
    // and the light/dark threshold, so main.rs stays free of inline logic.
    if let Some(lum) = term_caps::query_background_luminance() {
        let configured = app.config.ui.theme.as_deref();
        if let Some(theme) = term_caps::auto_theme_for_background(lum, configured) {
            // Session-only (persist=false); user can override via theme picker.
            app.set_theme(theme, false);
            log::info!(
                "OSC11 auto-detected light background (luminance={lum:.2}): switched to {theme}"
            );
        } else {
            log::info!(
                "OSC11 auto-detected background (luminance={lum:.2}): keeping current theme"
            );
        }
    }

    // ── Rich mode capability detection ───────────────────────────────
    // Runs after entering the alternate screen but before the event loop
    // starts reading stdin: the graphics probe (when it runs) must read the
    // terminal's query response from stdin itself, or the crossterm event
    // loop would swallow it.
    {
        let caps = term_caps::TermCaps::detect_from_env();
        let mode = app.config.rich.mode.clone();
        // `auto` probes only when env hints a graphics terminal (keeps startup
        // instant on unknown terminals); `force` always probes so it works as
        // an escape hatch on terminals the hint list doesn't know about.
        let probed = if mode == "force" || (mode != "off" && caps.graphics_likely) {
            match ratatui_image::picker::Picker::from_query_stdio() {
                Ok(picker) => Some(picker),
                Err(e) => {
                    log::warn!("graphics protocol probe failed: {e}");
                    None
                }
            }
        } else {
            None
        };
        let protocol = probed.map(|p| p.protocol_type());
        app.rich_tier = term_caps::resolve_rich_tier(&mode, &caps, protocol);
        app.rich_tier_available = app.rich_tier;
        if app.rich_tier.has_graphics() {
            app.rich_picker = probed;
        }
        log::info!(
            "rich mode: tier={:?} terminal={:?} protocol={:?}",
            app.rich_tier,
            caps.terminal_name,
            protocol
        );
        if app.rich_tier.is_rich() {
            let label = match (app.rich_tier.has_graphics(), caps.terminal_name.as_deref()) {
                (true, Some(name)) => format!("✨ Rich mode — {name} graphics detected"),
                (true, None) => String::from("✨ Rich mode — terminal graphics detected"),
                (false, Some(name)) => format!("✨ Rich mode — {name} truecolor"),
                (false, None) => String::from("✨ Rich mode — truecolor"),
            };
            app.status_message = Some(app::StatusMessage::new(
                label,
                app::StatusLevel::Info,
                app.ui_tick,
            ));
        }
    }

    // ── Build symbol index in background ─────────────────────────────
    app.start_symbol_index_build();

    // ── Main event loop ──────────────────────────────────────────────
    let result = run_loop(&mut terminal, &mut app);

    // ── Restore terminal (always, even on error) ─────────────────────
    // Best-effort: attempt every restore step even if an earlier one errors,
    // so a failure mid-teardown can't strand the user in a half-restored tty.
    let _ = leave_tui(terminal.backend_mut(), keyboard_enhanced);
    let _ = execute!(terminal.backend_mut(), SetTitle(""));
    let _ = terminal.show_cursor();

    // ── Persist view state (covers both normal quit and update-restart) ─
    // Must run before the `exec` below: `exec` replaces the process image, so
    // no Drop or later code would execute on the restart path.
    app.persist_view_state();

    // ── Restart if update was installed ───────────────────────────────
    if app.should_restart {
        println!("Restarting Conductor...");
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&app.startup_exe)
            .args(&app.startup_args)
            .exec();
        eprintln!("Failed to restart: {err}");
        std::process::exit(1);
    }

    // ── Session summary (gamification) ──────────────────────────────
    if let (Some(store), Some(session_id)) = (&app.review_store, &app.stats_session_id)
        && let Ok(stats) = store.end_stats_session(session_id)
    {
        let total = stats.reviews_created + stats.branches_created + stats.commits_made;
        if total > 0 {
            println!("\n--- Conductor Session Summary ---");
            if stats.reviews_created > 0 {
                println!("  Reviews created:  {}", stats.reviews_created);
            }
            if stats.branches_created > 0 {
                println!("  Branches created: {}", stats.branches_created);
            }
            if stats.commits_made > 0 {
                println!("  Commits made:     {}", stats.commits_made);
            }
            if let Ok(streak) = store.calculate_streak()
                && streak.consecutive_days > 0
            {
                println!("  Current streak:   {} day(s)", streak.consecutive_days);
            }
            println!("---------------------------------\n");
        }
    }

    result
}

// ── Terminal mode setup / teardown ───────────────────────────────────────
//
// `enter_tui` and `leave_tui` are exact inverses, with the enhancement-flag
// push/pop bracketing the raw-mode + alternate-screen + mouse/paste capture so
// suspend → restore round-trips cleanly. Keeping both in one place is what lets
// `main`'s startup/shutdown and the editor-suspend guard share the *same*
// symmetric sequence — a stray flag added to one but not the other would leave
// the terminal subtly broken on return.

/// Enter the full-screen TUI terminal mode (raw mode, alternate screen, mouse
/// and bracketed-paste capture, and — when supported — the kitty keyboard
/// enhancement flags).
fn enter_tui<W: io::Write>(w: &mut W, keyboard_enhanced: bool) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(
        w,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    )?;
    if keyboard_enhanced {
        execute!(
            w,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )?;
    }
    Ok(())
}

/// Leave the full-screen TUI terminal mode, returning the terminal to the
/// cooked / normal-screen baseline. The exact inverse of [`enter_tui`].
fn leave_tui<W: io::Write>(w: &mut W, keyboard_enhanced: bool) -> io::Result<()> {
    if keyboard_enhanced {
        execute!(w, PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    execute!(
        w,
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
    )?;
    Ok(())
}

/// Paths the file watcher should monitor: every worktree's path, or — when
/// there are no worktrees (e.g. a plain non-git directory) — the repo path
/// itself, so the Explorer still auto-refreshes on file changes there.
fn watch_paths_for(app: &App) -> Vec<std::path::PathBuf> {
    if app.worktrees.is_empty() {
        vec![app.repo_path.clone()]
    } else {
        app.worktrees.iter().map(|w| w.path.clone()).collect()
    }
}

/// Drive the draw → poll → handle cycle until the user quits.
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    // Set up file watcher for auto-refresh. The watched set is rebuilt later
    // (in the worktree_poll timer) whenever it changes — e.g. the user runs
    // `git init` in a plain folder, or adds/removes a worktree — so newly
    // created files keep showing up instead of going unnoticed.
    let mut current_watch_paths = watch_paths_for(app);
    let mut file_watcher = crate::file_watcher::FileWatcher::new(&current_watch_paths).ok();

    // Set up a dedicated watcher for the conductor config file. Separate from
    // the worktree file watcher: the worktree watcher is rebuilt each time the
    // worktree set changes, which would break config monitoring if they shared
    // the same watcher instance.
    let config_watcher =
        crate::config_watcher::ConfigWatcher::new(&crate::config::config_file_path()).ok();

    // Set up socket listener for CC state notifications (instant delivery).
    let cc_notify = crate::cc_notify::CcNotifyListener::new(&app.repo_path).ok();

    // Set up named pipe for MCP-triggered review refresh.
    let refresh_pipe = crate::refresh_pipe::RefreshPipe::new(&app.repo_path).ok();

    let mut last_frame_area = Rect::default();
    let mut last_claude_size: (u16, u16) = (0, 0);
    let mut last_shell_size: (u16, u16) = (0, 0);
    let mut first_frame_done = false;

    // Debounce file-watcher refreshes to avoid expensive git operations on
    // every single file-system event.
    const FS_DEBOUNCE: Duration = Duration::from_millis(500);
    let mut fs_pending = false;
    let mut fs_first_seen: Option<Instant> = None;

    // Separate debounce for config-file changes. Shorter than FS_DEBOUNCE and
    // isolated so worktree-poll rebuilds don't reset it.
    const CONFIG_DEBOUNCE: Duration = Duration::from_millis(300);
    let mut cfg_pending = false;
    let mut cfg_first_seen: Option<Instant> = None;

    // Periodic timers — consolidated into a single registry.
    let mut timers = timer::TimerRegistry::new();
    timers.register("worktree_poll", Duration::from_secs(3));
    timers.register("pty_cleanup", Duration::from_secs(10));
    timers.register("cc_waiting", Duration::from_secs(5));
    timers.register("stats_refresh", Duration::from_secs(30));

    // Track last user input to switch between active/idle tick rates.
    let mut last_input_time = Instant::now() - ACTIVITY_TIMEOUT;

    // ── ccusage polling (opt-in via [ccusage] enabled = true) ─────
    // Uses a global file cache so multiple Conductor instances don't
    // redundantly run `npx ccusage`.
    let ccusage_poll_secs = app.config.ccusage.poll_interval_secs;
    let ccusage_poll = Duration::from_secs(ccusage_poll_secs);
    let ccusage_enabled = app.config.ccusage.enabled;

    // On startup: immediately show whatever is in the cache.
    if ccusage_enabled && let Some(info) = ccusage_cache::read_any() {
        app.ccusage_info = Some(info);
    }
    // Schedule the first freshness check immediately.
    if ccusage_enabled {
        timers.register_immediate("ccusage", ccusage_poll);
    }

    // ── Update check (opt-out via [updates] check_on_startup = false) ─
    let update_check_enabled = app.config.updates.check_on_startup;
    let update_check_interval = Duration::from_secs(app.config.updates.check_interval_secs);
    if update_check_enabled {
        timers.register("update_check", update_check_interval);
    }

    if update_check_enabled {
        // Show badge immediately from cache while the background fetch runs.
        if let Some(cached) = update_checker::read_cache()
            && update_checker::is_newer(&cached.latest_version, update_checker::current_version())
        {
            app.update_info = Some(cached);
        }
        // Always fetch the latest release info in the background so we
        // never miss a new version due to stale cache data.
        app.bg.update_check.start(|tx| {
            let _ = tx.send(update_checker::check_for_update());
        });
    }

    // Seed the dirty flags so the first frame renders everything.
    app.dirty.mark_all();

    timers.register("decoration", DECORATION_TICK_INTERVAL);
    timers.register("unfocused_terminal", UNFOCUSED_TERMINAL_REFRESH);
    timers.register("pulse", PULSE_TICK_INTERVAL);

    // Maximum time to spend draining events before rendering a frame.
    // Prevents input starvation during rapid scroll (trackpad inertia
    // can generate 100+ events). Events beyond this budget are deferred
    // to the next iteration, ensuring smooth intermediate frames.
    const MAX_DRAIN: Duration = Duration::from_millis(8);

    // ══════════════════════════════════════════════════════════════
    // Main event loop — ordered for minimal input-to-pixel latency:
    //   1. Wait for events / timeout
    //   2. Handle events — drain ALL pending (coalesce rapid scroll)
    //   3. RENDER (throttled to ~60fps)
    //   4. Background work (timers, polling, file watcher)
    // ══════════════════════════════════════════════════════════════
    loop {
        // ── 1. Wait for an event ─────────────────────────────────
        let decoration_active =
            crate::ui::decoration::DecorationMode::from_str(&app.config.general.decoration)
                .has_animation();
        let pty_dirty = app.terminal.pty_manager.take_output_notify();
        if pty_dirty {
            app.terminal.dirty_claude = true;
            app.terminal.dirty_shell = true;
            if let Some(editor) = app.editor.as_mut() {
                editor.dirty = true;
            }
            app.dirty.mark(crate::app::DirtyPanels::TERMINAL);
            // The editor occupies the Explorer/Viewer columns, so its repaint
            // rides the EXPLORER/VIEWER dirty bits rather than TERMINAL.
            if app.editor.is_some() {
                app.dirty.mark(
                    crate::app::DirtyPanels::EXPLORER | crate::app::DirtyPanels::VIEWER,
                );
            }
        }

        let tick = if app.dirty.any() || pty_dirty {
            Duration::ZERO
        } else {
            match app.focus {
                crate::app::Focus::TerminalClaude
                | crate::app::Focus::TerminalShell
                | crate::app::Focus::Editor => TICK_RATE_TERMINAL,
                _ if app.update_state != crate::app::UpdateState::Idle => TICK_RATE_ACTIVE,
                _ if !app.worktree_mgr.pending_worktrees.is_empty() => TICK_RATE_ACTIVE,
                _ if app.show_panel_number_overlay => TICK_RATE_ACTIVE,
                _ if last_input_time.elapsed() < ACTIVITY_TIMEOUT => TICK_RATE_ACTIVE,
                _ if !app.terminal.cc_waiting_worktrees.is_empty() => PULSE_TICK_INTERVAL,
                // Party mode keeps animating even while idle.
                _ if app.party_mode => PULSE_TICK_INTERVAL,
                _ if decoration_active => DECORATION_TICK_INTERVAL,
                _ => TICK_RATE_IDLE,
            }
        };

        if crossterm_poll(tick)? {
            // ── 2. Handle events ─────────────────────────────────
            let drain_deadline = Instant::now() + MAX_DRAIN;
            loop {
                match crossterm_read()? {
                    // Treat auto-repeat (held key) the same as a press, so holding
                    // j/k/up/down scrolls or navigates continuously. Repeat events
                    // only arrive under the kitty keyboard protocol; without it,
                    // terminals deliver auto-repeat as a stream of Press events.
                    Event::Key(key)
                        if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
                    {
                        log::debug!(
                            "key: code={:?} mods={:?} kind={:?}",
                            key.code,
                            key.modifiers,
                            key.kind
                        );
                        last_input_time = Instant::now();
                        handle_key_event(app, key);
                    }
                    Event::Mouse(mouse) => {
                        last_input_time = Instant::now();
                        handle_mouse_event(app, mouse, last_frame_area);
                    }
                    Event::Paste(data) => {
                        last_input_time = Instant::now();
                        handle_paste_event(app, data);
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
                app.dirty.mark_all();
                // Stop draining if no more events or we've spent a full frame budget.
                if Instant::now() >= drain_deadline || !crossterm_poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        // ── 2b. Embedded editor lifecycle — close the panel once $EDITOR exits ──
        // The editor runs as an in-panel PTY (not a TUI suspend), so its exit is
        // detected here every iteration: when the child is gone we tear the panel
        // down, restore the Explorer/Viewer layout, and reload the edited file.
        if app.poll_editor_exit() {
            app.dirty.mark_all();
        }

        // ── 3. RENDER — immediately after events for lowest latency ──
        if app.terminal.needs_clear {
            terminal.clear()?;
            app.terminal.needs_clear = false;
            app.dirty.mark_all();
        }
        // Continuous dirty marking for active overlays.
        if app.update_state != crate::app::UpdateState::Idle
            || app.overlays.grep_search.running
            || app.overlays.grep_search.debounce_deadline.is_some()
            || app.show_panel_number_overlay
        {
            app.dirty.mark_all();
        }
        if !app.worktree_mgr.pending_worktrees.is_empty() {
            app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
        }

        if app.dirty.any() {
            app.ui_tick = app.ui_tick.wrapping_add(1);

            const STATUS_FADE_TICKS: u64 = 180;
            if let Some(ref msg) = app.status_message {
                let age = app.ui_tick.wrapping_sub(msg.created_at_tick);
                if age >= STATUS_FADE_TICKS {
                    app.status_message = None;
                }
            }

            // Auto-dismiss panel number overlay after timer expires.
            if app.show_panel_number_overlay && !app.show_panel_overlay() {
                app.show_panel_number_overlay = false;
                app.panel_overlay_since = None;
            }

            terminal.draw(|frame| {
                last_frame_area = frame.area();
                render_ui(frame, app);
            })?;

            app.dirty.clear();
        }

        // ── 4. Background work (ok to be slow) ──────────────────
        // Resize PTY sessions to match cached layout dimensions.
        app.sync_pty_sizes(&mut last_claude_size, &mut last_shell_size);

        if !first_frame_done {
            first_frame_done = true;
            app.perform_auto_resume();
        }

        // Periodic timers (git polling, decoration, terminal refresh, etc.)
        // Defer expensive I/O timers while user is actively interacting
        // (e.g. scrolling) to prevent mid-scroll freezes.
        let input_active = last_input_time.elapsed() < ACTIVITY_TIMEOUT;
        for name in timers.check_due() {
            match name {
                // Calm mode: only advance the decoration while the worktree panel
                // is focused, so it never moves in the user's periphery during
                // review/terminal work. It freezes in place otherwise and resumes
                // when focus returns.
                "decoration" if app.focus == crate::app::Focus::Worktree => {
                    let left_w = app.layout_cache.columns[0].width;
                    let panel_h = app.layout_cache.main_area.height;
                    let list_h = (app.worktrees.len() as u16 + 2).max(5);
                    let detail_h = (1 + app.worktree_mgr.local_branches.len() as u16 + 2).min(8);
                    let deco_h = panel_h.saturating_sub(list_h + detail_h);
                    if app.tick_decoration(left_w.saturating_sub(2), deco_h) {
                        app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
                    }
                }
                // Drive notification-bar breathing while any session waits,
                // regardless of focus or whether decoration is animating.
                "pulse" if !app.terminal.cc_waiting_worktrees.is_empty() => {
                    app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
                }
                // Drive party-mode animations (rainbow border, syntax, confetti).
                "pulse" if app.party_mode => {
                    app.dirty.mark_all();
                }
                "unfocused_terminal" => {
                    match app.focus {
                        crate::app::Focus::TerminalClaude => {
                            app.terminal.cache_shell = Default::default();
                        }
                        crate::app::Focus::TerminalShell => {
                            app.terminal.cache_claude = Default::default();
                        }
                        _ => {
                            app.terminal.cache_claude = Default::default();
                            app.terminal.cache_shell = Default::default();
                        }
                    }
                    app.dirty.mark(crate::app::DirtyPanels::TERMINAL);
                }
                // Expensive I/O timers — skip during active input to avoid scroll freezes.
                "worktree_poll" if !input_active => {
                    if app.refresh_worktrees() {
                        app.dirty.mark(
                            crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::EXPLORER,
                        );
                    }
                    // Rebuild the file watcher if the set of paths to watch
                    // changed (e.g. `git init` created the first worktree, or a
                    // worktree was added/removed). Without this the watcher would
                    // keep monitoring a stale set and miss new files.
                    let desired = watch_paths_for(app);
                    if desired != current_watch_paths {
                        current_watch_paths = desired;
                        file_watcher =
                            crate::file_watcher::FileWatcher::new(&current_watch_paths).ok();
                    }
                    // Periodic fallback: re-walk the file tree so newly created
                    // files appear even if a watcher event was missed. Cheap
                    // (lazy child loading) and only repaints when it changed.
                    if app.refresh_viewer() {
                        app.dirty.mark(
                            crate::app::DirtyPanels::EXPLORER | crate::app::DirtyPanels::VIEWER,
                        );
                    }
                    app.check_diff_viewer_staleness();
                }
                "pty_cleanup" if !input_active && app.cleanup_dead_sessions() => {
                    app.dirty.mark(
                        crate::app::DirtyPanels::TERMINAL | crate::app::DirtyPanels::WORKTREE,
                    );
                }
                "cc_waiting" if !input_active => {
                    if app.check_cc_waiting_state() {
                        app.dirty.mark(
                            crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL,
                        );
                    }
                    app.flush_deferred_prompts();
                }
                "stats_refresh" if !input_active => {
                    if let Some(store) = &app.review_store {
                        let new_stats = store.get_today_stats().ok();
                        if new_stats != app.today_stats {
                            app.today_stats = new_stats;
                            app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
                        }
                    }
                }
                "ccusage" => {
                    let max_age = ccusage_poll_secs;
                    app.bg.ccusage.start(move |tx| {
                        let info = ccusage_cache::read_if_fresh(max_age)
                            .or_else(ccusage_cache::fetch_and_cache);
                        if let Some(info) = info {
                            let _ = tx.send(info);
                        }
                    });
                }
                "update_check" => {
                    app.bg.update_check.start(|tx| {
                        let _ = tx.send(update_checker::check_for_update());
                    });
                }
                _ => {}
            }
        }

        // File system change events (debounced).
        if let Some(ref watcher) = file_watcher {
            while watcher.poll().is_some() {
                if !fs_pending {
                    fs_first_seen = Some(Instant::now());
                }
                fs_pending = true;
            }
            if fs_pending
                && let Some(t) = fs_first_seen
                && t.elapsed() >= FS_DEBOUNCE
            {
                fs_pending = false;
                fs_first_seen = None;
                app.refresh_worktrees();
                app.refresh_viewer();
                app.refresh_diff();
                if !app.bg.symbol_index.is_running() {
                    app.start_symbol_index_build();
                }
                app.dirty.mark_all();
            }
        }

        // Config file change events (debounced). Shorter debounce than FS events
        // because the two event streams are independent — a worktree-poll rebuild
        // must not reset the config debounce timer.
        if let Some(ref watcher) = config_watcher {
            while watcher.poll().is_some() {
                if !cfg_pending {
                    cfg_first_seen = Some(Instant::now());
                }
                cfg_pending = true;
            }
            if cfg_pending
                && let Some(t) = cfg_first_seen
                && t.elapsed() >= CONFIG_DEBOUNCE
            {
                cfg_pending = false;
                cfg_first_seen = None;
                app.reload_appearance_config();
            }
        }

        // CC state notifications.
        if let Some(ref cc_notify) = cc_notify {
            while let Some(event) = cc_notify.poll() {
                app.handle_cc_notify(event);
                app.dirty
                    .mark(crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL);
            }
        }

        // MCP refresh pipe — reload review comments when MCP writes to the pipe.
        if let Some(ref refresh_pipe) = refresh_pipe
            && refresh_pipe.poll().is_some()
        {
            // Drain any extra events (coalesce multiple rapid writes).
            while refresh_pipe.poll().is_some() {}
            app.refresh_reviews();
            app.dirty.mark_all();
            log::debug!("refresh_pipe: reloaded reviews from MCP trigger");
        }

        // Poll all background operations.
        app.poll_all_background_ops();

        if app.overlays.active == crate::overlay::ActiveOverlay::GrepSearch
            && app.check_grep_debounce()
        {
            app.dirty.mark_all();
        }

        if !app.terminal.deferred_prompts.is_empty() {
            app.flush_deferred_prompts();
        }

        app.terminal.pty_manager.nudge_alt_screen_sessions();

        if app.should_quit {
            return Ok(());
        }
    }
}

