//! Main event loop: the draw → poll → handle cycle, from process start until
//! the user quits.
//!
//! Setup (file watcher, config watcher, timers, ccusage/update-check
//! bootstrap) lives here alongside the loop that owns it. Periodic timer
//! handling and external-event-source polling (file watcher, config watcher,
//! CC-state socket, MCP refresh pipe) are broken out into
//! [`crate::event_loop_timers`] to keep this file from growing further.

use super::*;

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
/// Redraw cadence for rich-mode gradient borders (~30fps). The rotating focus
/// gradient and waiting glow (`ui::rich`) derive their phase from wall-clock
/// time but only advance when the frame is redrawn; without a dedicated cadence
/// the gradient stutters at the idle/decoration tick rate. Only armed in rich
/// mode (and never overriding the faster terminal/active rates), so the cost is
/// a steady 30fps repaint while rich effects are visible.
const RICH_REFRESH_INTERVAL: Duration = Duration::from_millis(33);

/// Paths the file watcher should monitor: every worktree's path, or — when
/// there are no worktrees (e.g. a plain non-git directory) — the repo path
/// itself, so the Explorer still auto-refreshes on file changes there.
pub(crate) fn watch_paths_for(app: &App) -> Vec<std::path::PathBuf> {
    if app.worktrees.is_empty() {
        vec![app.repo_path.clone()]
    } else {
        app.worktrees.iter().map(|w| w.path.clone()).collect()
    }
}

/// Drive the draw → poll → handle cycle until the user quits.
pub(crate) fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    // Set up file watcher for auto-refresh. The watched set is rebuilt later
    // (in the worktree_poll timer) whenever it changes — e.g. the user runs
    // `git init` in a plain folder, or adds/removes a worktree — so newly
    // created files keep showing up instead of going unnoticed.
    let mut current_watch_paths = watch_paths_for(app);
    let mut file_watcher = match crate::file_watcher::FileWatcher::new(&current_watch_paths) {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("file watcher setup failed: {e}");
            app.set_status(
                format!("File watcher unavailable — auto-refresh degraded ({e})"),
                crate::app::StatusLevel::Warning,
            );
            None
        }
    };

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
    // Track region-boundary state (maximize/restore, editor & reflow open/close,
    // and the divider/split widths). When any of these change a screen region
    // hands off to a different panel; ratatui's cell diff can't see that handoff,
    // so the vacated edge keeps the previous occupant's glyphs (e.g. a restored
    // Explorer showing fragments of the editor's code, or a resized panel leaving
    // text along its old edge). A hard clear on the change resyncs the screen.
    // `(expanded, editor, reflow, explorer_w, viewer_w, terminal_split, explorer_split)`
    type LayoutKey = (Option<crate::app::Focus>, bool, bool, u16, u16, u16, u16);
    let mut last_layout_key: Option<LayoutKey> = None;

    // Debounce state for file-watcher and config-watcher events, owned here
    // (persists across iterations) and passed to
    // `event_loop_timers::poll_watchers` each tick — see that function for
    // the debounce intervals themselves.
    let mut fs_pending = false;
    let mut fs_first_seen: Option<Instant> = None;
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
    timers.register("rich_glow", RICH_REFRESH_INTERVAL);

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
        // Rich-mode gradient borders animate from wall-clock time but need the
        // frame to keep redrawing; arm a 30fps cadence whenever rich effects are
        // on screen. Party mode owns its own animation path, so exclude it here.
        let rich_active = app.rich_tier.is_rich() && !app.party_mode;
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
                app.dirty
                    .mark(crate::app::DirtyPanels::EXPLORER | crate::app::DirtyPanels::VIEWER);
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
                // Keep frames flowing while a focus-border glide is in flight.
                _ if app.has_active_transition() => TICK_RATE_ACTIVE,
                // Rich gradients want ~30fps even when otherwise idle, so the
                // rotating border never stutters. Placed below the faster
                // terminal/active rates (which already cover their cases) and
                // above the slower waiting/decoration/idle rates.
                _ if rich_active => RICH_REFRESH_INTERVAL,
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
                        // D7(a): a key press means the mouse is no longer the
                        // active input device — crossterm never reports the
                        // mouse leaving the terminal window, so the underline
                        // and the row/chip/tab highlights would otherwise
                        // linger indefinitely once the user switches to the
                        // keyboard.
                        //
                        // Pointer highlights only: `handle_key_event` below
                        // owns the hover *popup*, and it needs the stack
                        // intact to tell a pinned modal (keys drive it) from a
                        // transient one (any key dismisses it, Esc consumed).
                        app.clear_pointer_hover();
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
                    Event::Resize(_, _) => {
                        // Window resize reshapes every panel boundary; hard-clear
                        // so no panel's old edge content lingers (same desync
                        // class as a divider resize).
                        app.terminal.needs_clear = true;
                    }
                    // D7(b): the one case crossterm *does* report that reliably
                    // implies the mouse has left our surface — the terminal
                    // window itself lost focus (e.g. the user alt-tabbed away).
                    Event::FocusLost => {
                        app.clear_all_hover();
                    }
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
        // Any change to a region boundary (panel maximize/restore, editor or
        // reflow open/close, OR a divider/split resize) needs a hard clear:
        // ratatui's cell diff can't repaint a region whose new owner happens to
        // draw the glyphs the diff already believes are there, so the previous
        // occupant's content lingers along the vacated edge. See decl above.
        {
            let layout_key = (
                app.expanded_panel,
                app.editor.is_some(),
                app.reflow.active,
                app.config.layout.explorer_width_pct,
                app.config.layout.viewer_width_pct,
                app.terminal_split_pct,
                app.config.layout.explorer_split_pct,
            );
            if last_layout_key != Some(layout_key) {
                if last_layout_key.is_some() {
                    app.terminal.needs_clear = true;
                }
                last_layout_key = Some(layout_key);
            }
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
        // Keep redrawing while a focus-border transition animates; it's
        // time-based, so without this it would freeze at the idle tick rate.
        if app.has_active_transition() {
            app.dirty.mark_all();
        }
        // Drive the reflow sweep animation: keep the terminal panel dirty for
        // every frame while a sweep is in progress (focus==TerminalClaude so the
        // tick rate is already 8ms; no additional wake-up source needed).
        if app.reflow.sweep.is_some() {
            app.dirty.mark(crate::app::DirtyPanels::TERMINAL);
        }
        // A pending hard clear implies a full repaint.
        if app.terminal.needs_clear {
            app.dirty.mark_all();
        }

        // Render the frame atomically. Synchronized output (terminal mode 2026)
        // brackets the clear + draw so the terminal presents the whole frame in
        // one shot. Without it, fast back-to-back frames (8ms during scrollback)
        // can be torn/partially applied, leaving the real screen out of sync
        // with ratatui's cell buffers — which then never repaints the diverged
        // cells (the scrollback "bleed"). Terminals lacking the capability just
        // ignore the markers.
        if app.terminal.needs_clear || app.dirty.any() {
            let _ = execute!(io::stdout(), BeginSynchronizedUpdate);
            if app.terminal.needs_clear {
                terminal.clear()?;
                app.terminal.needs_clear = false;
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
            let _ = execute!(io::stdout(), EndSynchronizedUpdate);
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
        crate::event_loop_timers::run_due_timers(
            app,
            &mut timers,
            &mut file_watcher,
            &mut current_watch_paths,
            rich_active,
            input_active,
            ccusage_poll_secs,
        );

        crate::event_loop_timers::poll_watchers(
            app,
            &file_watcher,
            &mut fs_pending,
            &mut fs_first_seen,
            &config_watcher,
            &mut cfg_pending,
            &mut cfg_first_seen,
            &cc_notify,
            &refresh_pipe,
        );

        // Poll all background operations.
        app.poll_all_background_ops();

        // Auto-hover: show the popup once the mouse has rested on a symbol past
        // the idle debounce, and manage its grace window / invalidation.
        app.tick_hover();

        // Jump underline (D8/D9): a separate, faster (150ms, no grace)
        // debounce than the popup's — see `tick_underline_hover`.
        app.tick_underline_hover();

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
            // An in-flight walkthrough generation is a headless `claude`
            // child process; without this it would keep running (and
            // billing API calls) as an orphan after Conductor exits, since
            // nothing polls it once the main loop stops.
            app.shutdown_walkthrough_generation();
            return Ok(());
        }
    }
}
