//! 組み込みエディタパネル — Explorer+Viewer を統合した領域に $EDITOR の PTY（vim/emacs）を
//! 描画し、ユーザーがファイルをインラインで編集できるようにする。
//!
//! [terminal_claude](super::terminal_claude) の単一セッション版の兄弟モジュール。
//! セッションタブもスクロールバックもない（全画面エディタは自前でスクロールを管理する）。
//! あるのは終了方法のヒントを示すタイトル行と、ライブの PTY 出力だけ。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, Focus};

/// area に組み込みエディタパネルを描画する。エディタが開いていなければ何もしない。
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
    let fg = app.theme.fg;
    let muted = app.theme.muted;
    let accent = app.theme.accent;

    let border_color = app.animated_border_color(Focus::Editor);
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let is_expanded = app.expanded_panel == Some(Focus::Editor);

    // タイトル行: ファイル名 + 終了方法のヒント。:q は常に有効（プロセスを終了させ、
    // それによってパネルが閉じる）。Ctrl+Esc は kitty keyboard protocol が必要。
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let title_style = if focused {
        Style::default().fg(fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(muted)
    };
    let title_line = ratatui::text::Line::from(vec![
        Span::styled(format!(" EDIT — {title} "), title_style),
        Span::styled(
            ":q close · Ctrl+Esc Claude · ctrl+alt+z zoom",
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

    // 新しい出力が届いた（dirty）か、キャッシュが空のときだけ PTY のスナップショットを
    // 再構築する。エディタは代替スクリーンで動くのでスクロールバックのオフセットはなく、
    // 常にライブビュー（オフセット0）を描画する。
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

        // フォーカスがあり隠れていないときは、IME 用にハードウェアカーソルを配置する。
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
