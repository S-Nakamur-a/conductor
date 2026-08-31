//! ターミナル Claude パネル — Claude Code の PTY セッションを表示する右上のエリア。
//!
//! セッションタブと、アクティブな Claude Code セッションの PTY 出力を表示する。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, Focus};

/// Claude Code ターミナルパネルを与えられた area に描画する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.appearance.theme;
    let focused = app.focus.current() == Focus::TerminalClaude;
    // フォーカス変化時、非フォーカス/フォーカスのボーダー色の間を緩やかに遷移する。
    let border_color = app.animated_border_color(Focus::TerminalClaude);

    let is_grabbed = app.is_selected_worktree_grabbed();
    let icon_set = app.config.ui.icon_set();
    let panel_icon = crate::icons::PANEL_TERMINAL.labeled(icon_set);
    let locked = format!(" {}", crate::icons::LOCKED.get(icon_set));

    let sessions = app.current_worktree_sessions(crate::pty_manager::SessionKind::ClaudeCode);

    let is_expanded = matches!(
        app.layout.expanded,
        Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
    );

    // 選択中の worktree が grab されている場合、セッションの代わりにロック表示を出す。
    if is_grabbed {
        let block = if is_expanded {
            Block::default().title(format!(" {panel_icon}Claude Code{locked} "))
        } else {
            Block::default()
                .title(Span::styled(
                    format!(" {panel_icon}Claude Code{locked} "),
                    Style::default().fg(theme.muted),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.muted))
        };
        let msg = Paragraph::new(vec![
            ratatui::text::Line::from(""),
            ratatui::text::Line::from(Span::styled(
                "  This worktree is grabbed.",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            )),
            ratatui::text::Line::from(Span::styled(
                "  Sessions are running on main.",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            )),
        ])
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
            Block::default().title(Span::styled(
                format!(" {panel_icon}Claude Code "),
                title_style,
            ))
        } else {
            Block::default()
                .title(Span::styled(
                    format!(" {panel_icon}Claude Code "),
                    title_style,
                ))
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(Style::default().fg(border_color))
        };
        let msg = Paragraph::new(" Enter / Click / Ctrl+n: new session")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    // レイアウト: セッションタブ（1行）+ PTY出力（残り全体）。
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);

    // セッションタブ — 水平スクロールするストリップで、[+]/展開ボタンは常に
    // 手が届くよう右端に固定されている（tab_bar を参照）。
    let suppress_blink = focused;
    let pulse_on = (app.ticks.ui() / 30).is_multiple_of(2);
    let tab_items: Vec<crate::ui::tab_bar::TabItem> = sessions
        .iter()
        .map(|(global_idx, session)| {
            let is_waiting = app.terminal.pty_manager.is_waiting_for_input(*global_idx);
            let is_active = Some(*global_idx) == app.terminal.claude.active_session;
            let label_style = if is_waiting {
                if suppress_blink {
                    Style::default().fg(theme.waiting_primary)
                } else {
                    Style::default()
                        .fg(if pulse_on {
                            theme.waiting_primary
                        } else {
                            theme.waiting_secondary
                        })
                        .add_modifier(Modifier::BOLD)
                }
            } else {
                Style::default()
            };
            crate::ui::tab_bar::TabItem {
                global_idx: *global_idx,
                label: format!("[{}]", session.label),
                is_active,
                label_style,
            }
        })
        .collect();
    let (hits, scroll) = crate::ui::tab_bar::render(
        frame,
        chunks[0],
        theme,
        &tab_items,
        app.terminal.claude.tab_scroll,
        app.terminal.claude.tab_reveal,
        is_expanded,
        app.terminal.claude.tab_hover,
    );
    app.terminal.claude.tab_hits = hits;
    app.terminal.claude.tab_scroll = scroll;
    app.terminal.claude.tab_reveal = false;

    // PTY出力。
    let output_area = chunks[1];
    let output_block = if is_expanded {
        Block::default()
    } else {
        // reflow がアクティブな間のボーダー色: アクセント色の補色が、読み取り
        // モードであることを常時示す合図になる。入る際の遷移ではボーダーが
        // アクセント色からその補色へ滑らかに（ちらつきのない単一のゆるやかな
        // グラデーションで）遷移する。ビューを抜けるのは瞬時なので抜ける際の
        // グラデーションはない。それ以外は通常のフォーカス/非フォーカス色。
        // 下の reflow 描画ガードに合わせてフォーカスで絞っている: パネルが
        // 非フォーカスの間はライブの PTY を表示する（reflow は保持されるが
        // 描画されない）ので、そこで読み取りモードの補色を出すと誤ったボーダー
        // の合図になってしまう。
        let effective_border = if app.reflow.active && app.focus.current() == Focus::TerminalClaude
        {
            let complement = crate::theme::Theme::complement(theme.accent);
            if let Some(sweep) = &app.reflow.sweep {
                let p = crate::reflow::input::sweep_progress(
                    &sweep.start,
                    crate::reflow::input::TRANSITION_DURATION_MS,
                );
                let t = crate::reflow::input::transition_eased(p);
                // 読み取りモードに入る: アクセント色 → 補色。
                crate::theme::Theme::lerp(theme.accent, complement, t)
            } else {
                // 読み取りモード安定時: 補色のまま静止する。
                complement
            }
        } else {
            border_color
        };
        Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(border_type)
            .border_style(Style::default().fg(effective_border))
    };

    // reflow のトランスクリプトビューがアクティブで、かつこのパネルにフォーカス
    // がある場合は、描画を reflow ビューに委ねる。フォーカスガードにより、
    // close_reflow を経由しなかった worktree 切替やフォーカス移動の後に古い
    // reflow が描画されるのを防ぐ（念のための二重の安全策。F4 は
    // set_focus/on_worktree_changed で reflow を閉じるが、このガードが描画の
    // 安全性を保つ）。
    if app.reflow.active && app.focus.current() == Focus::TerminalClaude {
        let inner = output_block.inner(output_area);
        frame.render_widget(output_block, output_area);
        crate::reflow::render::render(frame, inner, app);
        return;
    }

    if let Some(active_idx) = app.terminal.claude.active_session {
        if let Some(screen_arc) = app.terminal.pty_manager.get_screen(active_idx) {
            let inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            // PTY スナップショットは新しい出力が来た（dirty フラグ）とき、
            // またはキャッシュが空のときだけ再構築する。PTY リーダースレッドが
            // vt100 の mutex を保持している間ブロックしないよう try_lock を
            // 使う — UI の応答性を保つため。
            let scroll_changed =
                app.terminal.claude.cache.effective_offset != app.terminal.claude.scroll;
            if (app.terminal.claude.cache.lines.is_empty()
                || (focused && app.terminal.claude.dirty)
                || scroll_changed)
                && let Some(cache) = crate::terminal::render::pty::build_pty_lines(
                    &screen_arc,
                    app.terminal.claude.scroll,
                    inner.height,
                    inner.width,
                )
            {
                // スクロールがスクロールバックバッファを超えたときの無限再構築を
                // 防ぐため、スクロールオフセットを vt100 側の実際のクランプ済み
                // 位置と同期させる。
                app.terminal.claude.scroll = cache.effective_offset;
                app.terminal.claude.cache = cache;
                app.terminal.claude.dirty = false;
            }
            // try_lock が失敗した場合（リーダースレッドがビジー）、古いキャッシュを使い続ける。
            crate::terminal::render::pty::render_pty_cached(
                frame,
                inner,
                &app.terminal.claude.cache,
                &app.appearance.theme,
            );

            // フォーカスがあり、スクロールバックしておらず、このパネルを覆う
            // オーバーレイもない場合に IME 用のカーソル位置を設定する。
            if focused
                && !app.is_any_overlay_active()
                && let Some((row, col)) = app.terminal.claude.cache.cursor_position
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
