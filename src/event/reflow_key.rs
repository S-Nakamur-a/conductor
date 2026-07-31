//! Key handling for the infinite-scrollback reflow transcript view, layered
//! over the Claude terminal panel.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;

/// Handle a key event while the reflow transcript view is active.
///
/// All keys are consumed here and never forwarded to the PTY — the reflow
/// view is a pure read-only overlay. Navigation:
///
/// * `j` / Down — scroll down one line.
/// * `k` / Up — scroll up one line.
/// * `Ctrl-d` / PageDown — scroll down half a page.
/// * `Ctrl-u` / PageUp — scroll up half a page.
/// * `g` / Home — jump to the oldest turn (top).
/// * `G` / End — jump to the newest turn (bottom) without leaving.
/// * `Esc` — close reflow view and return to live PTY.
/// * `j` / Down / PageDown at the bottom — close reflow (live return).
pub(super) fn handle_reflow_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::KeyModifiers;
    use crate::event::reflow::{at_bottom, clamp_scroll};

    let inner = app.reflow.last_inner_height as usize;
    let total = app.reflow.total_lines;
    let page: usize = (inner / 2).max(1);
    let bottom = at_bottom(app.reflow.scroll, total, inner);
    let old_scroll = app.reflow.scroll;

    match key.code {
        // ── Line scroll ─────────────────────────────────────────────────────
        KeyCode::Char('j') | KeyCode::Down => {
            if bottom {
                // Bottom + discrete down-key → begin exit sweep back to live PTY.
                app.request_close_reflow();
                return;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(1);
        }

        // ── Page scroll ──────────────────────────────────────────────────────
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if bottom {
                app.request_close_reflow();
                return;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(page);
        }
        KeyCode::PageDown => {
            if bottom {
                app.request_close_reflow();
                return;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(page);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(page);
        }
        KeyCode::PageUp => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(page);
        }

        // ── Jump to top / bottom ─────────────────────────────────────────────
        KeyCode::Char('g') | KeyCode::Home => {
            app.reflow.scroll = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            // Snap to the newest turn (logical bottom) without leaving the view.
            app.reflow.scroll = total.saturating_sub(inner);
        }

        // ── Expand / collapse ────────────────────────────────────────────────
        // Claude Code's own transcript folds tool results and thinking blocks
        // and offers `ctrl+o` to expand; conductor reuses the key but expands
        // in place rather than switching to a separate full-screen view. It is
        // a single view-wide toggle: this panel has no per-block cursor to
        // aim a finer-grained one at.
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reflow.expanded = !app.reflow.expanded;
            app.reflow.needs_rebuild = true;
            return;
        }

        // ── Leave ────────────────────────────────────────────────────────────
        KeyCode::Esc => {
            // Play the exit sweep before returning to the live PTY.
            app.request_close_reflow();
            return;
        }

        _ => {} // All other keys are silently consumed.
    }

    // Clamp scroll after any adjustment.  Upper bound is total - inner, not
    // total - 1: aligns with the render path and at_bottom logic.
    app.reflow.scroll = clamp_scroll(app.reflow.scroll, total, inner);

    // On each scroll step, force a hard clear (presented atomically thanks to
    // synchronized output). The transcript is arbitrary Unicode; a glyph the
    // terminal renders wider than counted can drift a line and leave stale cells
    // that ratatui's diff — comparing only its own buffers — never repaints.
    // Re-clearing per step keeps the scrolled view free of that residue.
    if app.reflow.scroll != old_scroll {
        app.terminal.needs_clear = true;
    }
}
