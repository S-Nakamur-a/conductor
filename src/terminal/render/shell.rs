//! ターミナルシェルパネル — 右下エリアに shell の PTY セッションを表示する。
//!
//! セッションタブとアクティブな shell セッションの PTY 出力を表示する。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, Focus};

/// 指定した領域にシェルターミナルパネルを描画する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let focused = app.focus.current() == Focus::TerminalShell;
    let border_color = app.animated_border_color(Focus::TerminalShell);

    let is_grabbed = app.is_selected_worktree_grabbed();
    let icon_set = app.config.ui.icon_set();
    let panel_icon = crate::icons::PANEL_TERMINAL.labeled(icon_set);
    let locked = format!(" {}", crate::icons::LOCKED.get(icon_set));

    let sessions = app.current_worktree_sessions(crate::pty_manager::SessionKind::Shell);

    let is_expanded = matches!(
        app.expanded_panel,
        Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
    );

    // 選択中の worktree が grab されている場合は、セッションの代わりにロック表示を出す。
    if is_grabbed {
        let block = if is_expanded {
            Block::default().title(format!(" {panel_icon}Shell{locked} "))
        } else {
            Block::default()
                .title(Span::styled(
                    format!(" {panel_icon}Shell{locked} "),
                    Style::default().fg(theme.muted),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.muted))
        };
        let msg = Paragraph::new(Span::styled(
            "  Locked — worktree is grabbed",
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ))
        .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    if sessions.is_empty() {
        let title_style = if focused {
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        let block = if is_expanded {
            Block::default().title(Span::styled(format!(" {panel_icon}Shell "), title_style))
        } else {
            Block::default()
                .title(Span::styled(format!(" {panel_icon}Shell "), title_style))
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(Style::default().fg(border_color))
        };
        let msg = Paragraph::new(" Enter / Click / Ctrl+t: new session")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    // レイアウト: セッションタブ（1行） + PTY出力（残り全部）。
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);

    // セッションタブ — [+]/展開を固定表示するスクロール可能なストリップ（tab_bar 参照）。
    let tab_items: Vec<crate::ui::tab_bar::TabItem> = sessions
        .iter()
        .map(|(global_idx, session)| crate::ui::tab_bar::TabItem {
            global_idx: *global_idx,
            label: format!("[{}]", session.label),
            is_active: Some(*global_idx) == app.terminal.shell.active_session,
            label_style: Style::default(),
        })
        .collect();
    let (hits, scroll) = crate::ui::tab_bar::render(
        frame,
        chunks[0],
        theme,
        &tab_items,
        app.terminal.shell.tab_scroll,
        app.terminal.shell.tab_reveal,
        is_expanded,
        app.terminal.shell.tab_hover,
    );
    app.terminal.shell.tab_hits = hits;
    app.terminal.shell.tab_scroll = scroll;
    app.terminal.shell.tab_reveal = false;

    // PTY出力。
    let output_area = chunks[1];
    let output_block = if is_expanded {
        Block::default()
    } else {
        Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color))
    };

    if let Some(active_idx) = app.terminal.shell.active_session {
        if let Some(screen_arc) = app.terminal.pty_manager.get_screen(active_idx) {
            let inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            // 新しい出力が届いた（dirtyフラグ）か、キャッシュが空のときだけ PTY
            // スナップショットを再構築する。PTYリーダースレッドが vt100 の mutex を
            // 保持している間にブロックしないよう try_lock を使う。
            let scroll_changed =
                app.terminal.shell.cache.effective_offset != app.terminal.shell.scroll;
            if (app.terminal.shell.cache.lines.is_empty()
                || (focused && app.terminal.shell.dirty)
                || scroll_changed)
                && let Some(cache) = crate::terminal::render::pty::build_pty_lines(
                    &screen_arc,
                    app.terminal.shell.scroll,
                    inner.height,
                    inner.width,
                )
            {
                // スクロールオフセットを vt100 側で実際にクランプされた位置と同期させ、
                // スクロールがスクロールバックバッファを超えたときに無限に再構築される
                // のを防ぐ。
                app.terminal.shell.scroll = cache.effective_offset;
                app.terminal.shell.cache = cache;
                app.terminal.shell.dirty = false;
            }
            crate::terminal::render::pty::render_pty_cached(
                frame,
                inner,
                &app.terminal.shell.cache,
                &app.theme,
            );

            // フォーカスがあり、スクロールバックしておらず、オーバーレイがこのパネルを
            // 覆っていないときに IME 用のカーソル位置を設定する。
            if focused
                && !app.is_any_overlay_active()
                && let Some((row, col)) = app.terminal.shell.cache.cursor_position
            {
                let cursor_x = inner.x + col;
                let cursor_y = inner.y + row;
                if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
                    frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
                }
            }
        } else {
            frame.render_widget(output_block, output_area);
        }
    } else {
        frame.render_widget(output_block, output_area);
    }
}
