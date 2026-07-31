//! Conductor — a terminal-based Git workspace and code review tool.

mod ai_caller;
mod anim;
mod app;
mod background;
mod cc_notify;
mod ccusage_cache;
mod claude_log;
mod claude_sessions;
mod command_palette;
mod config;
mod config_watcher;
mod diff_state;
mod event;
mod event_loop;
mod event_loop_timers;
mod file_watcher;
mod gemini_api;
mod git_engine;
mod go_test;
mod grep_search;
mod hover_info;
mod jump_history;
mod keymap;
mod mcp_serve;
mod media_state;
mod menu;
mod overlay;
mod pr_intake;
mod pty_manager;
mod refresh_pipe;
mod review_publish;
mod review_state;
mod review_store;
mod rust_test;
mod search_result_tree;
mod symbol_index;
mod term_caps;
mod terminal_link;
mod terminal_state;
mod test_run;
mod text_input;
mod theme;
mod timer;
mod ui;
mod update_checker;
mod viewer;
mod walkthrough;
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
    BeginSynchronizedUpdate, DisableLineWrap, EnableLineWrap, EndSynchronizedUpdate,
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::app::App;
use crate::event::{handle_key_event, handle_mouse_event, handle_paste_event};
use crate::ui::layout::render_ui;

fn main() -> Result<()> {
    // ── Panic hook: restore the terminal, then log the backtrace ──────
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Restore FIRST, before any I/O that could itself fail: a panic
            // leaves the tty in whatever state `enter_tui` put it, and
            // `leave_tui` only runs when `run_loop` *returns* — an unwind skips
            // it entirely. Without this the user is dropped back to a shell
            // with no visible caret (ratatui emits `\x1b[?25l` every frame),
            // mouse tracking still on (`\x1b[?1003h`, so selection and the
            // pointer misbehave), the alternate screen still active, and raw
            // mode still set — all of which persist until they run `reset`.
            //
            // Main thread only. This crate does not set `panic = "abort"`, so a
            // worker (background diff, symbol index, worktree ops) unwinds just
            // itself while the event loop keeps drawing at 60fps. Tearing the
            // terminal down from *that* panic would leave the alternate screen
            // and raw mode off underneath a still-running TUI — frames would
            // start scribbling over the user's actual shell. A worker dying is
            // survivable; the log below is the right response on its own.
            // `execute!` flushes internally, so no extra flush is needed here.
            if std::thread::current().name() == Some("main") {
                let _ = restore_terminal_modes(&mut io::stdout());
            }

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
                    r#"conductor {}

Usage: conductor [REPO_PATH]
       conductor mcp-serve [--db <PATH>]

  REPO_PATH    Git repository to open (defaults to the current directory)

Commands:
  mcp-serve    Serve the review database to Claude Code over stdio (MCP).
               Started automatically by conductor and by the Claude Code
               plugin; not usually run by hand.

    --db <PATH>    Review database to serve. Defaults to $CONDUCTOR_DB_PATH,
                   then .conductor/conductor.db in the surrounding repository.

Options:
  -V, --version    Print version and exit
  -h, --help       Print this help and exit"#,
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            // Must return before any terminal setup below: this subcommand
            // speaks JSON-RPC on stdout, so entering the alternate screen or
            // probing terminal capabilities would corrupt the protocol.
            "mcp-serve" => return mcp_serve::run(),
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
    let result = event_loop::run_loop(&mut terminal, &mut app);

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
        // A TUI positions every cell explicitly (ratatui MoveTo's each row), so
        // auto-wrap must be OFF: otherwise a glyph that the terminal renders
        // wider than we counted can push a line's tail past the last column,
        // where auto-wrap kicks it onto the *next* row's first column — bleeding
        // one panel's overflow into the left edge of another. With wrap off the
        // overflow is harmlessly clamped at the right edge instead.
        DisableLineWrap,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        // D7(b): crossterm never reports the mouse leaving the terminal
        // window, so `Event::FocusLost` (the terminal losing focus entirely,
        // e.g. an alt-tab) is the one reliable signal the event loop has for
        // "the mouse definitely isn't resting on anything drawn right now" —
        // used to clear stale hover state (viewer underline, popup, tree/diff
        // row highlights).
        crossterm::event::EnableFocusChange,
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
    restore_terminal_modes(w)?;
    Ok(())
}

/// Write every mode-reset [`leave_tui`] depends on, and leave raw mode.
///
/// Split out of `leave_tui` so the panic hook can reuse it: the hook has no
/// access to the `Terminal` (or to `keyboard_enhanced`), but it must still undo
/// the modes `enter_tui` set, or an unwind strands the user's tty. Taking a
/// generic writer also makes the emitted sequence assertable in a test without
/// touching the real terminal.
///
/// `cursor::Show` is included because ratatui hides the caret on **every**
/// frame (`Terminal::draw` → `hide_cursor` unless a widget requested a cursor
/// position), so `\x1b[?25l` is essentially always the last caret state we set.
/// The normal exit path re-shows it via `terminal.show_cursor()`; the panic
/// path has no `Terminal`, so it has to be done here.
fn restore_terminal_modes<W: io::Write>(w: &mut W) -> io::Result<()> {
    // `disable_raw_mode` is a libc/termios call, not an escape sequence, so it
    // writes nothing to `w` — harmless (and idempotent) when `leave_tui` has
    // already called it.
    let _ = disable_raw_mode();
    execute!(
        w,
        EnableLineWrap,
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        crossterm::cursor::Show,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::restore_terminal_modes;

    /// The panic hook's whole purpose is that these specific modes get undone
    /// even when `leave_tui` never runs. Assert on the raw bytes rather than
    /// trusting the `execute!` list to stay complete: the two symptoms users
    /// actually report after an abnormal exit are an invisible caret and a
    /// misbehaving mouse, which map to exactly these two resets.
    #[test]
    fn panic_hook_restores_terminal() {
        let mut buf: Vec<u8> = Vec::new();
        restore_terminal_modes(&mut buf).expect("writing to a Vec cannot fail");
        let seq = String::from_utf8(buf).expect("escape sequences are ASCII");

        assert!(
            seq.contains("\x1b[?25h"),
            "caret must be shown again (ratatui hides it every frame); got {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1003l"),
            "any-event mouse tracking must be turned off; got {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1049l"),
            "the alternate screen must be left; got {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?2004l"),
            "bracketed paste must be turned off; got {seq:?}"
        );
    }
}
