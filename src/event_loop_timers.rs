//! Periodic timer handling and external-event-source polling for the main
//! event loop ([`crate::event_loop::run_loop`]).
//!
//! Split out of the loop body purely to keep [`crate::event_loop`] from
//! growing past a readable size — behavior is unchanged, this is the same
//! per-iteration "background work" step as two extracted functions.

use std::time::{Duration, Instant};

use crate::app::App;
use crate::event_loop::watch_paths_for;

/// Run every periodic timer that's due this iteration: git/worktree polling,
/// decoration/pulse/rich-glow redraw cadences, PTY cleanup, CC-waiting state,
/// stats refresh, ccusage, and update-check.
pub(crate) fn run_due_timers(
    app: &mut App,
    timers: &mut crate::timer::TimerRegistry,
    file_watcher: &mut Option<crate::file_watcher::FileWatcher>,
    current_watch_paths: &mut Vec<std::path::PathBuf>,
    rich_active: bool,
    input_active: bool,
    ccusage_poll_secs: u64,
) {
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
            // Drive the rich-mode gradient borders at a steady ~30fps. The
            // effect is a whole-frame post-process, so a full repaint is
            // required to advance it; the PTY raster stays cached (gated by
            // `dirty_claude`/`dirty_shell`), so this is a cheap widget redraw.
            "rich_glow" if rich_active => {
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
                if desired != *current_watch_paths {
                    match crate::file_watcher::FileWatcher::new(&desired) {
                        Ok(w) => {
                            *current_watch_paths = desired;
                            *file_watcher = Some(w);
                        }
                        Err(e) => {
                            // Keep the previous watcher (still valid for the
                            // old path set) instead of silently downgrading
                            // to no watcher at all; retry on the next poll.
                            log::warn!("file watcher rebuild failed: {e}");
                            app.set_status(
                                format!("File watcher rebuild failed ({e})"),
                                crate::app::StatusLevel::Warning,
                            );
                        }
                    }
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
                app.dirty
                    .mark(crate::app::DirtyPanels::TERMINAL | crate::app::DirtyPanels::WORKTREE);
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
                    let info = crate::ccusage_cache::read_if_fresh(max_age)
                        .or_else(crate::ccusage_cache::fetch_and_cache);
                    if let Some(info) = info {
                        let _ = tx.send(info);
                    }
                });
            }
            "update_check" => {
                app.bg.update_check.start(|tx| {
                    let _ = tx.send(crate::update_checker::check_for_update());
                });
            }
            _ => {}
        }
    }
}

/// Poll all external event sources feeding the loop (file watcher, config
/// watcher, CC-state notification socket, MCP refresh pipe), applying their
/// debounced or immediate effects to `app`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_watchers(
    app: &mut App,
    file_watcher: &Option<crate::file_watcher::FileWatcher>,
    fs_pending: &mut bool,
    fs_first_seen: &mut Option<Instant>,
    config_watcher: &Option<crate::config_watcher::ConfigWatcher>,
    cfg_pending: &mut bool,
    cfg_first_seen: &mut Option<Instant>,
    cc_notify: &Option<crate::cc_notify::CcNotifyListener>,
    refresh_pipe: &Option<crate::refresh_pipe::RefreshPipe>,
) {
    // Debounce file-watcher refreshes to avoid expensive git operations on
    // every single file-system event.
    const FS_DEBOUNCE: Duration = Duration::from_millis(500);
    // Separate debounce for config-file changes. Shorter than FS_DEBOUNCE and
    // isolated so worktree-poll rebuilds don't reset it.
    const CONFIG_DEBOUNCE: Duration = Duration::from_millis(300);

    // File system change events (debounced).
    if let Some(watcher) = file_watcher {
        while watcher.poll().is_some() {
            if !*fs_pending {
                *fs_first_seen = Some(Instant::now());
            }
            *fs_pending = true;
        }
        if *fs_pending
            && let Some(t) = *fs_first_seen
            && t.elapsed() >= FS_DEBOUNCE
        {
            *fs_pending = false;
            *fs_first_seen = None;
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
    if let Some(watcher) = config_watcher {
        while watcher.poll().is_some() {
            if !*cfg_pending {
                *cfg_first_seen = Some(Instant::now());
            }
            *cfg_pending = true;
        }
        if *cfg_pending
            && let Some(t) = *cfg_first_seen
            && t.elapsed() >= CONFIG_DEBOUNCE
        {
            *cfg_pending = false;
            *cfg_first_seen = None;
            app.reload_appearance_config();
        }
    }

    // CC state notifications.
    if let Some(cc_notify) = cc_notify {
        while let Some(event) = cc_notify.poll() {
            app.handle_cc_notify(event);
            app.dirty
                .mark(crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL);
        }
    }

    // MCP refresh pipe — reload review comments when MCP writes to the pipe.
    if let Some(refresh_pipe) = refresh_pipe
        && refresh_pipe.poll().is_some()
    {
        // Drain any extra events (coalesce multiple rapid writes).
        while refresh_pipe.poll().is_some() {}
        app.refresh_reviews();
        app.dirty.mark_all();
        log::debug!("refresh_pipe: reloaded reviews from MCP trigger");
    }
}
