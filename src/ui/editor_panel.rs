//! Embedded editor panel — renders the `$EDITOR` PTY (vim/emacs) in the merged
//! Explorer+Viewer region while the user edits a file inline.
//!
//! A single-session sibling of [`terminal_claude`](super::terminal_claude): no
//! session tabs, no scrollback (full-screen editors own their own scrolling) —
//! just a title row with exit hints and the live PTY output.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, Focus};

/// Render the embedded editor panel into `area`. No-op if no editor is open.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some((session_idx, title)) = app.editor.as_ref().map(|e| {
        let name = e
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| e.path.display().to_string());
        (e.session_idx, name)
    }) else {
        return;
    };

    let focused = app.focus == Focus::Editor;
    let border_focused = app.theme.border_focused;
    let border_unfocused = app.theme.border_unfocused;
    let fg = app.theme.fg;
    let muted = app.theme.muted;
    let accent = app.theme.accent;

    let border_color = if focused {
        border_focused
    } else {
        border_unfocused
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let is_expanded = app.expanded_panel == Some(Focus::Editor);

    // Title row: filename + exit hints. `:q` always works (it ends the process,
    // which closes the panel); Ctrl+Esc needs the kitty keyboard protocol.
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let title_style = if focused {
        Style::default().fg(fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(muted)
    };
    let title_line = ratatui::text::Line::from(vec![
        Span::styled(format!(" EDIT — {title} "), title_style),
        Span::styled(
            ":q close · Ctrl+Esc Claude · alt+m zoom",
            Style::default().fg(muted),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title_line).style(Style::default().fg(accent)),
        chunks[0],
    );

    let output_area = chunks[1];
    let output_block = if is_expanded {
        Block::default()
    } else {
        Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color))
    };

    let Some(screen_arc) = app.terminal.pty_manager.get_screen(session_idx) else {
        frame.render_widget(output_block, output_area);
        return;
    };
    let inner = output_block.inner(output_area);
    frame.render_widget(output_block, output_area);

    // Rebuild the PTY snapshot only when new output arrived (dirty) or the
    // cache is empty. Editors run on the alternate screen, so there is no
    // scrollback offset — always render the live view (offset 0).
    if let Some(editor) = app.editor.as_mut()
        && (editor.cache.lines.is_empty() || editor.dirty)
        && let Some(cache) =
            crate::ui::common::build_pty_lines(&screen_arc, 0, inner.height, inner.width)
    {
        editor.cache = cache;
        editor.dirty = false;
    }

    if let Some(editor) = app.editor.as_ref() {
        crate::ui::common::render_pty_cached(frame, inner, &editor.cache, &app.theme);

        // Place the hardware cursor for IME when focused and unobscured.
        if focused
            && !app.is_any_overlay_active()
            && let Some((row, col)) = editor.cache.cursor_position
        {
            let cursor_x = inner.x + col;
            let cursor_y = inner.y + row;
            if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
                frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
            }
        }
    }
}
