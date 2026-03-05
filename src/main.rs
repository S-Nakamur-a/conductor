//! Conductor — a terminal-based Git workspace and code review tool.

mod app;
mod background;
mod ccusage_cache;
mod claude_sessions;
mod command_palette;
mod config;
mod diff_state;
mod event;
mod file_watcher;
mod git_engine;
mod grep_search;
mod keymap;
mod overlay;
mod pty_manager;
mod review_state;
mod review_store;
mod terminal_state;
mod text_input;
mod theme;
mod ui;
mod update_checker;
mod viewer;
mod worktree_ops;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    poll as crossterm_poll, read as crossterm_read,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;
use crate::event::{handle_key_event, handle_mouse_event, handle_paste_event};

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

    let mut last_frame_area = Rect::default();
    let mut last_claude_size: (u16, u16) = (0, 0);
    let mut last_shell_size: (u16, u16) = (0, 0);
    let mut first_frame_done = false;

    // Debounce file-watcher refreshes to avoid expensive git operations on
    // every single file-system event.
    const FS_DEBOUNCE: Duration = Duration::from_millis(500);
    let mut fs_pending = false;
    let mut fs_first_seen: Option<Instant> = None;

    // Periodically re-scan worktrees so that `git worktree add/remove`
    // executed inside a terminal session is reflected in the UI.
    const WORKTREE_POLL: Duration = Duration::from_secs(3);
    let mut last_worktree_poll = Instant::now();

    const PTY_CLEANUP_POLL: Duration = Duration::from_secs(10);
    let mut last_pty_cleanup = Instant::now();

    const CC_WAITING_POLL: Duration = Duration::from_millis(500);
    let mut last_cc_waiting_check = Instant::now();

    const STATS_REFRESH_POLL: Duration = Duration::from_secs(30);
    let mut last_stats_refresh = Instant::now();

    // Track last user input to switch between active/idle tick rates.
    let mut last_input_time = Instant::now() - ACTIVITY_TIMEOUT;

    // ── ccusage polling (opt-in via [ccusage] enabled = true) ─────
    // Uses a global file cache so multiple Conductor instances don't
    // redundantly run `npx ccusage`.
    let ccusage_poll_secs = app.config.ccusage.poll_interval_secs;
    let ccusage_poll = Duration::from_secs(ccusage_poll_secs);
    let ccusage_enabled = app.config.ccusage.enabled;
    let ccusage_result: Arc<Mutex<Option<crate::app::CcusageInfo>>> =
        Arc::new(Mutex::new(None));

    // On startup: immediately show whatever is in the cache.
    if ccusage_enabled {
        if let Some(info) = ccusage_cache::read_any() {
            app.ccusage_info = Some(info);
        }
    }
    // Schedule the first freshness check after a short delay so the UI
    // renders immediately, then we check/refresh the cache in background.
    let mut last_ccusage_poll = Instant::now() - ccusage_poll;

    // ── Update check (opt-out via [updates] check_on_startup = false) ─
    let update_check_enabled = app.config.updates.check_on_startup;
    let update_check_interval = Duration::from_secs(app.config.updates.check_interval_secs);
    let mut last_update_check = Instant::now();
    let update_result: Arc<Mutex<Option<update_checker::UpdateInfo>>> =
        Arc::new(Mutex::new(None));

    if update_check_enabled {
        // Show badge immediately from cache while the background fetch runs.
        if let Some(cached) = update_checker::read_cache() {
            if update_checker::is_newer(&cached.latest_version, update_checker::current_version()) {
                app.update_info = Some(cached);
            }
        }
        // Always fetch the latest release info in the background so we
        // never miss a new version due to stale cache data.
        let result_handle = Arc::clone(&update_result);
        std::thread::spawn(move || {
            if let Some(info) = update_checker::check_for_update() {
                if let Ok(mut lock) = result_handle.lock() {
                    *lock = Some(info);
                }
            }
        });
    }

    let mut needs_redraw = true;

    // Independent timer for decoration animation (ticks at fixed ~20fps).
    let mut last_decoration_time = Instant::now();

    // Timer for refreshing unfocused terminal panels.
    let mut last_unfocused_terminal_refresh = Instant::now();

    loop {
        if app.terminal.needs_clear {
            terminal.clear()?;
            app.terminal.needs_clear = false;
            needs_redraw = true;
        }

        // Terminal panels need continuous rendering for PTY output.
        if matches!(app.focus, crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell) {
            needs_redraw = true;
        }
        // Update overlays need continuous rendering for spinner animation.
        if app.update_state != crate::app::UpdateState::Idle {
            needs_redraw = true;
        }
        // Grep search streaming results need continuous rendering.
        if app.grep_search.running {
            needs_redraw = true;
        }
        // Grep debounce waiting needs active tick rate.
        if app.grep_search.debounce_deadline.is_some() {
            needs_redraw = true;
        }

        if needs_redraw {
            // Advance animation tick only on actual renders.
            app.ui_tick = app.ui_tick.wrapping_add(1);

            // Auto-clear status messages after ~3 seconds (180 ticks at 60fps).
            const STATUS_FADE_TICKS: u64 = 180;
            if let Some(ref msg) = app.status_message {
                let age = app.ui_tick.wrapping_sub(msg.created_at_tick);
                if age >= STATUS_FADE_TICKS {
                    app.status_message = None;
                }
            }

            // Draw the current frame.
            terminal.draw(|frame| {
                last_frame_area = frame.area();
                render_ui(frame, app);
            })?;

            needs_redraw = false;
        }

        // Compute PTY sizes for Claude and Shell panels.
        {
            let area = last_frame_area;
            // Must match render_ui layout: title bar (1) + notification bar (0 or 1) + main + status bar (1).
            let notif_height: u16 = if !app.terminal.cc_waiting_worktrees.is_empty() { 1 } else { 0 };
            let outer = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(notif_height),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(area);
            let main_area = outer[2];

            let (left_w, explorer_w, viewer_w) = accordion_widths(app.expanded_panel, main_area.width);
            let right_w = main_area.width.saturating_sub(left_w.saturating_add(explorer_w).saturating_add(viewer_w));

            let is_terminal_expanded = matches!(app.expanded_panel, Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell));
            let border_cols: u16 = if is_terminal_expanded { 0 } else { 2 };
            let border_rows: u16 = if is_terminal_expanded { 1 } else { 2 }; // tab bar always, bottom border only when not expanded
            if right_w > border_cols {
                let right_cols = right_w.saturating_sub(border_cols);
                // Claude: 80% of right height, Shell: 20%
                let claude_rows_total = (main_area.height as u32 * 80 / 100) as u16;
                let shell_rows_total = main_area.height.saturating_sub(claude_rows_total);
                // Subtract tab bar (1) + bottom border (0 or 1) from each
                let claude_pty_rows = claude_rows_total.saturating_sub(border_rows);
                let shell_pty_rows = shell_rows_total.saturating_sub(border_rows);

                if (claude_pty_rows, right_cols) != last_claude_size && claude_pty_rows > 0 && right_cols > 0 {
                    last_claude_size = (claude_pty_rows, right_cols);
                    app.update_claude_terminal_size(claude_pty_rows, right_cols);
                }
                if (shell_pty_rows, right_cols) != last_shell_size && shell_pty_rows > 0 && right_cols > 0 {
                    last_shell_size = (shell_pty_rows, right_cols);
                    app.update_shell_terminal_size(shell_pty_rows, right_cols);
                }
            }
        }

        // Auto-resume Claude sessions after the first frame (PTY sizes are known).
        if !first_frame_done {
            first_frame_done = true;
            app.perform_auto_resume();
        }

        // Tick decoration on a fixed timer, independent of main tick rate.
        if last_decoration_time.elapsed() >= DECORATION_TICK_INTERVAL {
            last_decoration_time = Instant::now();
            let (left_w, _, _) = accordion_widths(app.expanded_panel, last_frame_area.width);
            let panel_h = last_frame_area.height.saturating_sub(3);
            let list_h = (app.worktrees.len() as u16 + 2).max(5);
            let detail_h = (1 + app.worktree_mgr.local_branches.len() as u16 + 2).min(8);
            let deco_h = panel_h.saturating_sub(list_h + detail_h);
            if app.tick_decoration(left_w.saturating_sub(2), deco_h) {
                needs_redraw = true;
            }
        }

        // Periodically refresh unfocused terminal panels so background PTY
        // output (e.g. running builds, Claude Code responses) remains visible.
        if !matches!(app.focus, crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
            && last_unfocused_terminal_refresh.elapsed() >= UNFOCUSED_TERMINAL_REFRESH
        {
            last_unfocused_terminal_refresh = Instant::now();
            // Invalidate caches so the next draw picks up fresh PTY content.
            app.terminal.cache_claude = Default::default();
            app.terminal.cache_shell = Default::default();
            needs_redraw = true;
        }

        // Wait for an event. Use a fast tick rate shortly after user input
        // so that scrolling and navigation feel responsive, then fall back to
        // an idle rate to save CPU.
        let decoration_active = crate::ui::decoration::DecorationMode::from_str(&app.config.general.decoration)
            .has_animation();
        let tick = match app.focus {
            crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell => TICK_RATE_TERMINAL,
            _ if app.update_state != crate::app::UpdateState::Idle => TICK_RATE_ACTIVE,
            _ if !app.worktree_mgr.pending_worktrees.is_empty() => TICK_RATE_ACTIVE,
            _ if last_input_time.elapsed() < ACTIVITY_TIMEOUT => TICK_RATE_ACTIVE,
            _ if decoration_active => DECORATION_TICK_INTERVAL,
            _ => TICK_RATE_IDLE,
        };
        if crossterm_poll(tick)? {
            match crossterm_read()? {
                Event::Key(key) => { last_input_time = Instant::now(); handle_key_event(app, key); }
                Event::Mouse(mouse) => { last_input_time = Instant::now(); handle_mouse_event(app, mouse, last_frame_area); }
                Event::Paste(data) => { last_input_time = Instant::now(); handle_paste_event(app, data); }
                Event::Resize(_, _) => {}
                _ => {}
            }
            needs_redraw = true;
        }

        // Check for file system change events (debounced).
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
                        needs_redraw = true;
                    }
                }
            }
        }

        // Check if a background fetch for the switch-branch overlay has finished.
        app.poll_bg_branches();

        // Check if a background pull has finished.
        app.poll_bg_pull();

        // Poll grep search results.
        app.poll_grep_search();

        // Check grep debounce timer.
        if app.grep_search.active && app.check_grep_debounce() {
            needs_redraw = true;
        }

        // Poll update download progress.
        app.poll_update_progress();

        // Poll background PR URL lookup.
        app.poll_pr_url();

        // Poll background worktree create/delete operations.
        app.poll_worktree_ops();

        // Periodically refresh the worktree list to pick up external changes
        // (e.g. `git worktree add` run inside a terminal panel).
        if last_worktree_poll.elapsed() >= WORKTREE_POLL {
            last_worktree_poll = Instant::now();
            app.refresh_worktrees();
            app.check_diff_viewer_staleness();
            needs_redraw = true;
        }

        // Periodically remove dead PTY sessions (exited processes).
        if last_pty_cleanup.elapsed() >= PTY_CLEANUP_POLL {
            last_pty_cleanup = Instant::now();
            app.cleanup_dead_sessions();
            needs_redraw = true;
        }

        // Periodically check if any Claude Code sessions are waiting for input.
        if last_cc_waiting_check.elapsed() >= CC_WAITING_POLL {
            last_cc_waiting_check = Instant::now();
            app.check_cc_waiting_state();
            needs_redraw = true;
        }

        // Periodically refresh gamification stats (streak, today's activity).
        if last_stats_refresh.elapsed() >= STATS_REFRESH_POLL {
            last_stats_refresh = Instant::now();
            if let Some(store) = &app.review_store {
                app.today_stats = store.get_today_stats().ok();
            }
            needs_redraw = true;
        }

        // Force redraw while worktree ops are pending (for spinner animation).
        if !app.worktree_mgr.pending_worktrees.is_empty() {
            needs_redraw = true;
        }

        // ── ccusage background fetch (with global file cache) ────────
        if ccusage_enabled && last_ccusage_poll.elapsed() >= ccusage_poll {
            last_ccusage_poll = Instant::now();
            let result_handle = Arc::clone(&ccusage_result);
            let max_age = ccusage_poll_secs;
            std::thread::spawn(move || {
                // Check if another Conductor instance already refreshed
                // the cache recently — if so, just use that.
                let info = ccusage_cache::read_if_fresh(max_age)
                    .or_else(ccusage_cache::fetch_and_cache);
                if let Some(info) = info {
                    if let Ok(mut lock) = result_handle.lock() {
                        *lock = Some(info);
                    }
                }
            });
        }
        // Pick up ccusage result from background thread.
        if ccusage_enabled {
            if let Ok(mut lock) = ccusage_result.try_lock() {
                if let Some(info) = lock.take() {
                    app.ccusage_info = Some(info);
                    needs_redraw = true;
                }
            }
        }

        // Periodic update check — spawn a background fetch at the configured interval.
        if update_check_enabled && last_update_check.elapsed() >= update_check_interval {
            last_update_check = Instant::now();
            let result_handle = Arc::clone(&update_result);
            std::thread::spawn(move || {
                if let Some(info) = update_checker::check_for_update() {
                    if let Ok(mut lock) = result_handle.lock() {
                        *lock = Some(info);
                    }
                }
            });
        }

        // Pick up update check result from background thread.
        // Always accept the fresh result — it supersedes any cached data.
        if update_check_enabled {
            if let Ok(mut lock) = update_result.try_lock() {
                if let Some(info) = lock.take() {
                    if update_checker::is_newer(&info.latest_version, update_checker::current_version()) {
                        app.update_info = Some(info);
                        needs_redraw = true;
                    }
                }
            }
        }

        // Nudge PTY sessions that just entered alternate screen mode
        // (e.g. fzf) by sending a no-op resize to trigger SIGWINCH.
        app.terminal.pty_manager.nudge_alt_screen_sessions();

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Calculate accordion panel widths based on panel expansion state.
///
/// Returns `(left_width, explorer_width, viewer_width)`. The right panel gets whatever remains.
pub fn accordion_widths(expanded_panel: Option<crate::app::Focus>, total_width: u16) -> (u16, u16, u16) {
    use crate::app::Focus;

    match expanded_panel {
        Some(Focus::Worktree) => (total_width, 0, 0),
        Some(Focus::Explorer) => (0, total_width, 0),
        Some(Focus::Viewer) => (0, 0, total_width),
        Some(Focus::TerminalClaude | Focus::TerminalShell) => (0, 0, 0),
        None => {
            // Default proportions.
            let min_col = 3_u16;
            let left = ((total_width as u32 * 15 / 100) as u16).max(min_col);
            let explorer = ((total_width as u32 * 20 / 100) as u16).max(min_col);
            let viewer = ((total_width as u32 * 30 / 100) as u16).max(min_col);
            (left, explorer, viewer)
        }
    }
}

/// Top-level UI renderer — 3-column accordion layout + status bar.
fn render_ui(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let has_notifications = !app.terminal.cc_waiting_worktrees.is_empty();
    let notif_height: u16 = if has_notifications { 1 } else { 0 };

    // Outer: title bar (1 row) + notification bar (0 or 1 row) + main content + status bar (1 row).
    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(notif_height),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let title_area = outer[0];
    let notif_area = outer[1];
    let main_area = outer[2];
    let status_area = outer[3];

    // ── Title bar ───────────────────────────────────────────────────
    ui::common::render_title_bar(frame, title_area, app);

    // ── Notification bar (CC waiting) ───────────────────────────────
    if has_notifications {
        ui::common::render_notification_bar(frame, notif_area, app);
    }

    // ── Accordion column widths ─────────────────────────────────────
    let (left_w, explorer_w, viewer_w) = accordion_widths(app.expanded_panel, main_area.width);
    let right_w = main_area.width.saturating_sub(left_w.saturating_add(explorer_w).saturating_add(viewer_w));

    let columns = Layout::horizontal([
        Constraint::Length(left_w),
        Constraint::Length(explorer_w),
        Constraint::Length(viewer_w),
        Constraint::Length(right_w),
    ])
    .split(main_area);

    // ── Column 0: Worktree panel ────────────────────────────────────
    ui::worktree_panel::render(frame, columns[0], app);

    // ── Column 1: Explorer (file tree + diff list) ──────────────────
    ui::explorer_panel::render(frame, columns[1], app);

    // ── Column 2: Viewer (file content) ─────────────────────────────
    ui::viewer_panel::render(frame, columns[2], app);

    // ── Column 3: Terminal split (Claude 80% / Shell 20%) ───────────
    let terminal_split = Layout::vertical([
        Constraint::Percentage(80),
        Constraint::Percentage(20),
    ])
    .split(columns[3]);

    ui::terminal_claude::render(frame, terminal_split[0], app);
    ui::terminal_shell::render(frame, terminal_split[1], app);

    // ── Overlays ────────────────────────────────────────────────────
    // These render on top of everything else when active.
    if app.history.active {
        ui::dashboard::render_history_overlay(frame, main_area, app);
    }
    if app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::CreatingWorktree {
        ui::dashboard::render_worktree_input_overlay(frame, main_area, app);
    }
    if app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::CreatingWorktreeBase {
        ui::dashboard::render_worktree_base_input_overlay(frame, main_area, app);
    }
    if app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::ConfirmingDelete {
        render_confirming_delete_overlay(frame, main_area, app);
    }
    if app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::ConfirmingDeleteBranch {
        ui::dashboard::render_delete_branch_confirm_overlay(frame, main_area, app);
    }
    if app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::ConfirmingUngrab {
        render_confirm_overlay(frame, main_area, app, " Confirm Ungrab ", ratatui::style::Color::Yellow);
    }
    if app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::SmartDescription {
        ui::dashboard::render_smart_description_overlay(frame, main_area, app);
    }
    if app.cherry_pick.active {
        ui::dashboard::render_cherry_pick_overlay(frame, main_area, app);
    }
    if app.switch_branch.active {
        ui::dashboard::render_switch_branch_overlay(frame, main_area, app);
    }
    if app.grab.active {
        ui::dashboard::render_grab_overlay(frame, main_area, app);
    }
    if app.prune.active {
        ui::dashboard::render_prune_overlay(frame, main_area, app);
    }
    if app.repo_selector.active {
        ui::dashboard::render_repo_selector_overlay(frame, main_area, app);
    }
    if app.open_repo.active {
        ui::dashboard::render_open_repo_overlay(frame, main_area, app);
    }
    if app.review_state.input_mode != crate::review_state::ReviewInputMode::Normal {
        ui::review::render_input_overlay(frame, main_area, app);
    }
    if app.review_state.template_picker_active {
        ui::review::render_template_picker_overlay(frame, main_area, &app.review_state, &app.theme);
    }
    if app.review_state.comment_detail_active {
        ui::review::render_comment_detail_overlay(frame, main_area, app);
    }
    if app.resume_session.active {
        ui::dashboard::render_resume_session_overlay(frame, main_area, app);
    }
    if app.grep_search.active {
        ui::grep_search::render_grep_search_overlay(frame, main_area, app);
    }
    if app.command_palette.active {
        ui::dashboard::render_command_palette_overlay(frame, main_area, app);
    }
    if app.help.active {
        ui::dashboard::render_help_overlay(frame, main_area, app);
    }
    match app.update_state {
        crate::app::UpdateState::Confirming => {
            ui::dashboard::render_update_confirm_overlay(frame, main_area, app);
        }
        crate::app::UpdateState::InProgress | crate::app::UpdateState::Restarting | crate::app::UpdateState::Failed => {
            ui::dashboard::render_update_progress_overlay(frame, main_area, app);
        }
        crate::app::UpdateState::Idle => {}
    }

    // ── Skip reason modal ────────────────────────────────────────────
    if let Some(ref reason) = app.worktree_mgr.skip_reason {
        render_skip_reason_overlay(frame, main_area, reason);
    }

    // ── Status bar ──────────────────────────────────────────────────
    // Show worktree branch + repo on the right of status bar.
    let worktree_branch = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.branch.as_str())
        .unwrap_or("");
    ui::common::render_status_bar(frame, status_area, app);
    ui::common::render_worktree_label(frame, status_area, worktree_branch, &app.repo_path, &app.theme);
}

/// Render a small confirmation overlay for worktree deletion.
fn render_confirming_delete_overlay(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    render_confirm_overlay(frame, area, app, " Confirm Delete ", ratatui::style::Color::Red);
}

/// Generic small confirmation overlay with a customizable title and border color.
fn render_confirm_overlay(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    title: &str,
    border_color: ratatui::style::Color,
) {
    if let Some(ref status_msg) = app.status_message {
        let msg = &status_msg.text;
        let popup_height = 3_u16;
        let popup_width = area.width.saturating_sub(8).min(60);
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + area.height.saturating_sub(popup_height + 2);
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(ratatui::widgets::Clear, popup_area);

        let block = ratatui::widgets::Block::default()
            .title(title)
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(border_color));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Span::styled(
            msg.as_str(),
            ratatui::style::Style::default().fg(ratatui::style::Color::Yellow),
        ));
        frame.render_widget(paragraph, inner);
    }
}

/// Render a skip-reason informational popup.
fn render_skip_reason_overlay(
    frame: &mut ratatui::Frame,
    area: Rect,
    reason: &str,
) {
    let popup_height = 5_u16;
    let popup_width = area.width.saturating_sub(8).min(60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = ratatui::widgets::Block::default()
        .title(" Skipped ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let text = vec![
        ratatui::text::Line::from(ratatui::text::Span::styled(
            reason,
            ratatui::style::Style::default().fg(ratatui::style::Color::Yellow),
        )),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "(Esc) 閉じる",
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        )),
    ];
    let paragraph = ratatui::widgets::Paragraph::new(text)
        .wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}
