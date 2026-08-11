//! ヘルプオーバーレイ: keymap から自動生成される、パネルごとのキーバインド
//! チートシート。

use crate::app::App;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// 現在のコンテキストのキーバインドを表示するヘルプオーバーレイを描画する。
pub fn render_help_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::app::Focus;

    let theme = &app.theme;
    let popup_width = 72_u16.min(area.width.saturating_sub(4));
    let popup_height = 30_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // どのパネルのヘルプを表示しているかを示すタブバー。
    let tabs = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).split(popup_area);

    let tab_labels = [
        ("1:Worktree", Focus::Worktree),
        ("2:Explorer", Focus::Explorer),
        ("3:Viewer", Focus::Viewer),
        ("4:Terminal", Focus::TerminalClaude),
    ];

    let tab_spans: Vec<Span> = tab_labels
        .iter()
        .flat_map(|(label, focus)| {
            let style = if *focus == app.overlays.help.context
                || (*focus == Focus::TerminalClaude
                    && app.overlays.help.context == Focus::TerminalShell)
            {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.fg)
            };
            vec![
                Span::styled(format!(" {label} "), style),
                Span::styled(" ", Style::default()),
            ]
        })
        .collect();

    let tab_line =
        Paragraph::new(Line::from(tab_spans)).style(Style::default().bg(theme.titlebar_bg));
    frame.render_widget(tab_line, tabs[0]);

    // メインの内容ブロック。
    let block = Block::default()
        .title(" Help (?/Esc: close, 1-4: switch panel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(tabs[1]);
    frame.render_widget(block, tabs[1]);

    let lines = help_lines_for(app, app.overlays.help.context, theme);
    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// セクション見出しの行を追加する。
fn help_section(lines: &mut Vec<Line<'static>>, title: &'static str, theme: &Theme) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        title,
        Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
    )));
}

/// キーバインドの行を追加する（動的: キーは KeyMap から取得する）。
fn help_key_dyn(lines: &mut Vec<Line<'static>>, keys: String, desc: &'static str, theme: &Theme) {
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {keys:<18}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc, Style::default().fg(theme.fg)),
    ]));
}

/// ヘルプタブのチートシート行を組み立てる。keymap から自動生成しているので、
/// そのパネルで発火するすべてのバインドを常に漏れなく列挙できる — 手作業での
/// 選別は一切していないので、アクションが黙って抜け落ちることがない（以前の
/// 手作りリストは一部しか表示できていなかった）。レイヤーごとに1セクションで、
/// そのレイヤー自身のバインドを列挙する（グローバルなコード進行は「Global」の
/// 下に1回だけ表示する）。
fn help_lines_for(app: &App, focus: crate::app::Focus, theme: &Theme) -> Vec<Line<'static>> {
    use crate::app::Focus;
    use crate::keymap::{Action, KeyContext};

    let mut lines = Vec::new();

    let section = |lines: &mut Vec<Line<'static>>, title: &'static str, ctx: KeyContext| {
        let mut entries: Vec<(String, &'static str)> = Vec::new();
        for &action in Action::ALL {
            let keys = app.keymap.keys_in_layer(ctx, action);
            if !keys.is_empty() {
                entries.push((keys.join(" / "), action.label()));
            }
        }
        if entries.is_empty() {
            return;
        }
        help_section(lines, title, theme);
        for (keys, desc) in entries {
            help_key_dyn(lines, keys, desc, theme);
        }
    };

    // まずパネル固有のレイヤー（今いる場所に一番関係が深い）、その後に
    // 常に使えるグローバルなコード進行を並べる。
    let panel_ctxs: &[(&'static str, KeyContext)] = match focus {
        Focus::Worktree => &[("Worktree panel", KeyContext::Worktree)],
        Focus::Explorer => &[
            ("Explorer — file tree", KeyContext::Explorer),
            ("Explorer — changed files", KeyContext::ExplorerDiffList),
            ("Explorer — comment list", KeyContext::ExplorerCommentList),
        ],
        Focus::Viewer => &[
            ("Viewer", KeyContext::Viewer),
            ("Viewer — diff mode", KeyContext::ViewerDiffMode),
        ],
        Focus::TerminalClaude | Focus::TerminalShell => &[("Terminal panel", KeyContext::Terminal)],
        Focus::Editor => &[("Editor panel", KeyContext::Editor)],
        Focus::Revidere => &[("Review — sections + diff", KeyContext::Revidere)],
    };
    for (title, ctx) in panel_ctxs {
        section(&mut lines, title, *ctx);
    }
    if matches!(focus, Focus::TerminalClaude | Focus::TerminalShell) {
        help_transcript_section(&mut lines, theme);
    }
    help_review_commands_section(&mut lines, theme);
    section(&mut lines, "Global — works anywhere", KeyContext::Global);

    lines
}

/// ライブの末尾から上にスクロールして入る、Claude のトランスクリプトのキー。
///
/// 下のセクションと同じ理由で手書きにしている: handle_reflow_key が
/// app.keymap を経由せずこれらを直接扱っているため、keymap を辿る section()
/// からは1つも見えない。これができるまでは help に一切表示されておらず、
/// G も例外ではなかった。
fn help_transcript_section(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    help_section(lines, "Claude transcript (scroll up to enter)", theme);
    for (keys, desc) in [
        ("j / k", "Scroll one line"),
        ("ctrl+d / ctrl+u", "Scroll half a page"),
        ("g / Home", "Oldest turn"),
        ("G / End", "Newest turn, and resume following it"),
        ("ctrl+o", "Expand / collapse tool results and thinking"),
        ("Esc", "Back to the live prompt"),
    ] {
        help_key_dyn(lines, keys.to_string(), desc, theme);
    }
}

/// PR取り込み、ウォークスルー生成、公開の各コマンドにはデフォルトのキーバインドが
/// ない（default_keybinds.toml 参照） — コマンドパレット経由でしか到達できないため、
/// app.keymap を辿る上の section() では見つからない。それでも help 画面に
/// 表示されるよう、ここに列挙している。
fn help_review_commands_section(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    help_section(lines, "Review (via command palette)", theme);
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Review Pull Request…",
        theme,
    );
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Publish Comments to GitHub",
        theme,
    );
}
