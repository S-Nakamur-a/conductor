//! Command palette overlay: fuzzy-searchable list of all keymap actions.

use super::input::{format_input_with_cursor, set_cursor_for_input};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

/// Render the command palette overlay with search bar and command list.
pub fn render_command_palette_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::command_palette;

    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 24_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let chunks = Layout::vertical([
        Constraint::Length(3), // Search bar
        Constraint::Min(3),    // Command list
    ])
    .split(popup_area);

    // Search bar
    let search_block = Block::default()
        .title(" Command Palette (Enter: run, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let search_inner = search_block.inner(chunks[0]);
    frame.render_widget(search_block, chunks[0]);

    let search_text = format_input_with_cursor(&app.overlays.command_palette.filter);
    frame.render_widget(
        Paragraph::new(Span::styled(search_text, Style::default().fg(theme.fg))),
        search_inner,
    );
    set_cursor_for_input(frame, search_inner, &app.overlays.command_palette.filter);

    // Command list
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let context = app.focus.key_context();
    let filtered = command_palette::filter_commands(
        &app.overlays.command_palette.filter,
        &app.keymap,
        context,
    );
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("  No matching commands.").style(Style::default().fg(theme.muted)),
            list_inner,
        );
        return;
    }

    let current_label = match app.focus {
        crate::app::Focus::Worktree => "Worktree",
        crate::app::Focus::Explorer => "Explorer",
        crate::app::Focus::Viewer => "Viewer",
        crate::app::Focus::TerminalClaude => "Claude Code",
        crate::app::Focus::TerminalShell => "Shell",
        crate::app::Focus::Editor => "Editor",
    };
    let scope_header = |scope: command_palette::CommandScope| match scope {
        command_palette::CommandScope::Current => current_label,
        command_palette::CommandScope::Global => "Global",
        command_palette::CommandScope::Other => "Other",
    };

    // Interleave non-selectable scope headers between the (selectable) command
    // rows. `selected` indexes the command rows only, so track the visual row
    // index of the selected command to drive the highlight.
    let selected = app.overlays.command_palette.selected;
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_row: Option<usize> = None;
    let mut last_scope: Option<command_palette::CommandScope> = None;

    for (cmd_idx, scored) in filtered.iter().enumerate() {
        if last_scope != Some(scored.scope) {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {}", scope_header(scored.scope)),
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD | Modifier::DIM),
            ))));
            last_scope = Some(scored.scope);
        }

        let cmd = &command_palette::COMMANDS[scored.index];
        let is_selected = cmd_idx == selected;
        if is_selected {
            selected_row = Some(items.len());
        }
        let style = if is_selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        // Live keybinding from the keymap for the focused context (blank for
        // palette-only commands and commands not bound in this context).
        let kb = cmd
            .action
            .and_then(|a| crate::ui::common::representative_chord(&app.keymap, context, a))
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(
                if is_selected { " > " } else { "   " },
                Style::default().fg(theme.accent),
            ),
            Span::styled(cmd.label, style),
            Span::styled(format!("  {kb:>12}"), Style::default().fg(theme.muted)),
        ]);
        items.push(ListItem::new(line));
    }

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(selected_row);
    frame.render_stateful_widget(list, list_inner, &mut state);
}
