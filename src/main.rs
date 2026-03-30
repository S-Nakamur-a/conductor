//! Conductor — a terminal-based Git workspace and code review tool.

mod gemini_api;
mod app;
mod background;
mod cc_notify;
mod ccusage_cache;
mod jump_history;
mod symbol_index;
mod claude_sessions;
mod command_palette;
mod config;
mod diff_state;
mod event;
mod file_watcher;
mod git_engine;
mod grep_search;
mod keymap;
mod media_state;
mod overlay;
mod pty_manager;
mod review_state;
mod review_store;
mod search_result_tree;
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
    Event, KeyEventKind, KeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, poll as crossterm_poll, read as crossterm_read,
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

    // ── Set up crossterm terminal ────────────────────────────────────
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    log::debug!("keyboard_enhanced = {keyboard_enhanced}");
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    )?;
    if keyboard_enhanced {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // ── Create application state ─────────────────────────────────────
    let repo_path = match std::env::args().nth(1) {
        Some(path) => {
            let p = std::path::PathBuf::from(&path);
            if p.is_absolute() { p } else { std::env::current_dir()?.join(p) }
        }
        None => std::env::current_dir()?,
    };
    let mut app = App::new(repo_path);

    // ── Set terminal window title ────────────────────────────────────
    let window_title = format!("conductor - {}", app.main_repo_name);
    execute!(io::stdout(), SetTitle(&window_title))?;

    // ── Build symbol index in background ─────────────────────────────
    app.start_symbol_index_build();

    // ── Main event loop ──────────────────────────────────────────────
    let result = run_loop(&mut terminal, &mut app);

    // ── Restore terminal (always, even on error) ─────────────────────
    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        SetTitle(""),
    )?;
    terminal.show_cursor()?;

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
    if let (Some(store), Some(session_id)) = (&app.review_store, &app.stats_session_id) {
        if let Ok(stats) = store.end_stats_session(session_id) {
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
                if let Ok(streak) = store.calculate_streak() {
                    if streak.consecutive_days > 0 {
                        println!("  Current streak:   {} day(s)", streak.consecutive_days);
                    }
                }
                println!("---------------------------------\n");
            }
        }
    }

    result
}

/// Drive the draw → poll → handle cycle until the user quits.
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    // Set up file watcher for auto-refresh.
    let watch_paths: Vec<std::path::PathBuf> =
        app.worktrees.iter().map(|w| w.path.clone()).collect();
    let file_watcher = crate::file_watcher::FileWatcher::new(&watch_paths).ok();

    // Set up socket listener for CC state notifications (instant delivery).
    let cc_notify = crate::cc_notify::CcNotifyListener::new(&app.repo_path).ok();

    let mut last_frame_area = Rect::default();
    let mut last_claude_size: (u16, u16) = (0, 0);
    let mut last_shell_size: (u16, u16) = (0, 0);
    let mut first_frame_done = false;

    // Debounce file-watcher refreshes to avoid expensive git operations on
    // every single file-system event.
    const FS_DEBOUNCE: Duration = Duration::from_millis(500);
    let mut fs_pending = false;
    let mut fs_first_seen: Option<Instant> = None;

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
    if ccusage_enabled {
        if let Some(info) = ccusage_cache::read_any() {
            app.ccusage_info = Some(info);
        }
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
        if let Some(cached) = update_checker::read_cache() {
            if update_checker::is_newer(&cached.latest_version, update_checker::current_version()) {
                app.update_info = Some(cached);
            }
        }
        // Always fetch the latest release info in the background so we
        // never miss a new version due to stale cache data.
        app.bg_update_check_op.start(|tx| {
            let _ = tx.send(update_checker::check_for_update());
        });
    }

    // Seed the dirty flags so the first frame renders everything.
    app.dirty.mark_all();

    timers.register("decoration", DECORATION_TICK_INTERVAL);
    timers.register("unfocused_terminal", UNFOCUSED_TERMINAL_REFRESH);

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
        let decoration_active = crate::ui::decoration::DecorationMode::from_str(&app.config.general.decoration)
            .has_animation();
        let pty_dirty = app.terminal.pty_manager.take_output_notify();
        if pty_dirty {
            app.terminal.dirty_claude = true;
            app.terminal.dirty_shell = true;
            app.dirty.mark(crate::app::DirtyPanels::TERMINAL);
        }

        let tick = if app.dirty.any() || pty_dirty {
            Duration::ZERO
        } else {
            match app.focus {
                crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell => TICK_RATE_TERMINAL,
                _ if app.update_state != crate::app::UpdateState::Idle => TICK_RATE_ACTIVE,
                _ if !app.worktree_mgr.pending_worktrees.is_empty() => TICK_RATE_ACTIVE,
                _ if app.show_panel_number_overlay => TICK_RATE_ACTIVE,
                _ if last_input_time.elapsed() < ACTIVITY_TIMEOUT => TICK_RATE_ACTIVE,
                _ if decoration_active => DECORATION_TICK_INTERVAL,
                _ => TICK_RATE_IDLE,
            }
        };

        if crossterm_poll(tick)? {
            // ── 2. Handle events ─────────────────────────────────
            let drain_deadline = Instant::now() + MAX_DRAIN;
            loop {
                match crossterm_read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        log::debug!("key: code={:?} mods={:?}", key.code, key.modifiers);
                        last_input_time = Instant::now();
                        handle_key_event(app, key);
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Repeat => {
                        last_input_time = Instant::now();
                        if app.focus == crate::app::Focus::TerminalClaude
                            || app.focus == crate::app::Focus::TerminalShell
                        {
                            handle_key_event(app, key);
                        }
                    }
                    Event::Mouse(mouse) => {
                        last_input_time = Instant::now();
                        handle_mouse_event(app, mouse, last_frame_area);
                    }
                    Event::Paste(data) => { last_input_time = Instant::now(); handle_paste_event(app, data); }
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
                "decoration" => {
                    let left_w = app.layout_cache.columns[0].width;
                    let panel_h = app.layout_cache.main_area.height;
                    let list_h = (app.worktrees.len() as u16 + 2).max(5);
                    let detail_h = (1 + app.worktree_mgr.local_branches.len() as u16 + 2).min(8);
                    let deco_h = panel_h.saturating_sub(list_h + detail_h);
                    if app.tick_decoration(left_w.saturating_sub(2), deco_h) {
                        app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
                    }
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
                        app.dirty.mark(crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::EXPLORER);
                    }
                    app.check_diff_viewer_staleness();
                }
                "pty_cleanup" if !input_active => {
                    if app.cleanup_dead_sessions() {
                        app.dirty.mark(crate::app::DirtyPanels::TERMINAL | crate::app::DirtyPanels::WORKTREE);
                    }
                }
                "cc_waiting" if !input_active => {
                    if app.check_cc_waiting_state() {
                        app.dirty.mark(crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL);
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
                    app.bg_ccusage_op.start(move |tx| {
                        let info = ccusage_cache::read_if_fresh(max_age)
                            .or_else(ccusage_cache::fetch_and_cache);
                        if let Some(info) = info {
                            let _ = tx.send(info);
                        }
                    });
                }
                "update_check" => {
                    app.bg_update_check_op.start(|tx| {
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
            if fs_pending {
                if let Some(t) = fs_first_seen {
                    if t.elapsed() >= FS_DEBOUNCE {
                        fs_pending = false;
                        fs_first_seen = None;
                        app.refresh_worktrees();
                        app.refresh_viewer();
                        app.refresh_diff();
                        if !app.bg_symbol_index_op.is_running() {
                            app.start_symbol_index_build();
                        }
                        app.dirty.mark_all();
                    }
                }
            }
        }

        // CC state notifications.
        if let Some(ref cc_notify) = cc_notify {
            while let Some(event) = cc_notify.poll() {
                app.handle_cc_notify(event);
                app.dirty.mark(crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL);
            }
        }

        // Poll all background operations.
        app.poll_all_background_ops();

        if app.overlays.active == crate::overlay::ActiveOverlay::GrepSearch && app.check_grep_debounce() {
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

