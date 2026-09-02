//! 画面を描く唯一の入口。`&Workspace` しか取らないので、描画が次フレームの入力の
//! 前提を作ることがない。区画は [crate::layout] が先に決めている。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use conductor_core::keymap::{Action, KeyContext, KeyMap};

use crate::layout::{Layout, Region};
use crate::modal::Modal;
use crate::workspace::{Focus, StatusLevel, Workspace};

/// メニューバーの見出し (左から右)。項目はフェーズ 4。
const MENUS: [&str; 8] = [
    "Repo", "Worktree", "Review", "View", "Panel", "Search", "Terminal", "Help",
];

pub fn render(frame: &mut Frame, ws: &Workspace, layout: &Layout) {
    for (region, rect) in &layout.regions {
        let rect = *rect;
        match region {
            Region::TitleBar => frame.render_widget(Paragraph::new(title_line(ws)), rect),
            Region::MenuBar => frame.render_widget(Paragraph::new(menu_line(ws)), rect),
            Region::WorktreeStrip => {
                frame.render_widget(Paragraph::new(worktree_strip_line(ws)), rect)
            }
            Region::StatusBar => frame.render_widget(Paragraph::new(status_line(ws)), rect),
            panel => frame.render_widget(panel_block(ws, *panel), rect),
        }
    }

    if let Some(modal) = ws.modals.last() {
        render_modal(frame, ws, modal, frame.area());
    }
}

fn title_line(ws: &Workspace) -> Line<'_> {
    let theme = &ws.theme;
    Line::from(vec![
        Span::styled(
            format!(" {} ", ws.repo.name),
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            &ws.repo.branch,
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(theme.muted)),
        Span::styled(
            ws.repo.root.display().to_string(),
            Style::default().fg(theme.dir_fg),
        ),
    ])
}

fn menu_line(ws: &Workspace) -> Line<'static> {
    let style = if ws.chrome.menu_open {
        Style::default().fg(ws.theme.fg)
    } else {
        Style::default().fg(ws.theme.muted)
    };
    Line::from(
        MENUS
            .iter()
            .map(|title| Span::styled(format!(" {title} "), style))
            .collect::<Vec<_>>(),
    )
}

fn worktree_strip_line(ws: &Workspace) -> Line<'static> {
    let style = Style::default().fg(if ws.focus == Focus::Worktree {
        ws.theme.border_focused
    } else {
        ws.theme.muted
    });
    Line::styled("", style)
}

fn panel_block(ws: &Workspace, region: Region) -> Block<'static> {
    let focused = focused_region(ws.focus) == region;
    let border = if focused {
        Style::default().fg(ws.theme.border_focused)
    } else {
        Style::default().fg(ws.theme.border_unfocused)
    };
    let title = match region {
        Region::Explorer => "Explorer",
        Region::Viewer => "Viewer",
        Region::TerminalClaude => "Claude Code",
        Region::TerminalShell => "Shell",
        _ => "",
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                border.add_modifier(Modifier::BOLD)
            } else {
                border
            },
        ))
}

/// フォーカスが枠を光らせる区画。Editor と Revidere は Viewer の場所を借りる。
fn focused_region(focus: Focus) -> Region {
    match focus {
        Focus::Worktree => Region::WorktreeStrip,
        Focus::Explorer => Region::Explorer,
        Focus::Viewer | Focus::Editor | Focus::Revidere => Region::Viewer,
        Focus::TerminalClaude => Region::TerminalClaude,
        Focus::TerminalShell => Region::TerminalShell,
    }
}

/// フラッシュメッセージ、無ければフォーカス中パネルのキーヒント。
pub fn status_line(ws: &Workspace) -> Line<'static> {
    let theme = &ws.theme;
    match &ws.chrome.status {
        Some(msg) => Line::styled(
            msg.text.clone(),
            Style::default().fg(match msg.level {
                StatusLevel::Success => theme.success,
                StatusLevel::Error => theme.error,
                StatusLevel::Warning => theme.warning,
                StatusLevel::Info => theme.info,
            }),
        ),
        None => Line::styled(
            key_hint(ws.focus, &ws.keymap),
            Style::default().fg(theme.hint),
        ),
    }
}

/// フッターのキーヒントを今のキーマップから組み立てる。
///
/// 未割り当てのアクションだけのエントリは落ちるので、発火しないキーを案内しない。
fn key_hint(focus: Focus, keymap: &KeyMap) -> String {
    let entries: &[(&str, &[Action])] = match focus {
        Focus::Worktree => &[
            ("nav", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("open", &[Action::Select]),
            ("new", &[Action::CreateWorktree]),
            ("switch", &[Action::SwitchBranch]),
        ],
        Focus::Explorer => &[
            ("nav", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("open", &[Action::Select]),
            ("fold", &[Action::CollapseOrLeft, Action::ExpandOrRight]),
            ("diff", &[Action::ShowDiffList]),
            ("search", &[Action::SearchFilename]),
        ],
        Focus::Viewer => &[
            ("scroll", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("search", &[Action::SearchInFile]),
            ("tab", &[Action::NextViewerTab, Action::CloseViewerTab]),
            ("back", &[Action::ExitToExplorer]),
        ],
        Focus::TerminalClaude => &[
            ("leave", &[Action::LeaveTerminal]),
            ("panel", &[Action::CycleFocusForward]),
            ("new CC", &[Action::NewClaudeCode]),
            ("session", &[Action::NextSession]),
        ],
        Focus::TerminalShell => &[
            ("leave", &[Action::LeaveTerminal]),
            ("panel", &[Action::CycleFocusForward]),
            ("new shell", &[Action::NewShell]),
            ("session", &[Action::NextSession]),
        ],
        Focus::Editor => &[
            ("leave", &[Action::LeaveTerminal]),
            ("zoom", &[Action::TogglePanelExpand]),
            ("panel", &[Action::CycleFocusForward]),
        ],
        Focus::Revidere => &[
            ("scroll", &[Action::NavigateDown, Action::NavigateUp]),
            (
                "section",
                &[Action::RevidereNextSection, Action::RevidererPrevSection],
            ),
            ("close", &[Action::ExitSubPanel]),
        ],
    };

    let context = focus.key_context();
    let mut parts: Vec<String> = Vec::new();
    for (label, actions) in entries {
        let chords: Vec<String> = actions
            .iter()
            .filter_map(|a| representative_chord(keymap, context, *a))
            .collect();
        if !chords.is_empty() {
            parts.push(format!("{}: {label}", chords.join("/")));
        }
    }
    // パレットとヘルプは他の全アクションへの入口なので、どのコンテキストでも案内する。
    for (action, label) in [(Action::CommandPalette, "cmds"), (Action::ShowHelp, "keys")] {
        if let Some(chord) = representative_chord(keymap, context, action) {
            parts.push(format!("{chord}: {label}"));
        }
    }
    if focus.is_pty() {
        parts.push("keys → terminal".to_string());
    }
    parts.join(" | ")
}

/// そのアクションを発火させるチョードのうち、案内に載せる 1 つ。
fn representative_chord(keymap: &KeyMap, context: KeyContext, action: Action) -> Option<String> {
    keymap
        .keys_for_action(context, action)
        .into_iter()
        .filter(|c| c.is_ascii())
        .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

fn render_modal(frame: &mut Frame, ws: &Workspace, modal: &Modal, area: Rect) {
    let (title, body) = match modal {
        Modal::Help => ("Keys".to_string(), help_lines(ws)),
        Modal::Prompt(prompt) => (
            prompt.title.clone(),
            vec![Line::from(format!("> {}", prompt.input.text()))],
        ),
        Modal::Confirm(confirm) => (
            "Confirm".to_string(),
            vec![
                Line::from(confirm.question.clone()),
                Line::from(""),
                Line::styled("y / n", Style::default().fg(ws.theme.hint)),
            ],
        ),
    };

    let height = (body.len() as u16 + 2).min(area.height);
    let rect = centered(area, 60, height);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ws.theme.border_focused))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(ws.theme.fg)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Reset));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(body).block(block), rect);
}

/// フォーカス中のコンテキストで発火するチョードとアクション名。ヘルプが実挙動から
/// ずれないよう、表ではなくキーマップを引く。
fn help_lines(ws: &Workspace) -> Vec<Line<'static>> {
    let context = ws.focus.key_context();
    Action::ALL
        .iter()
        .filter_map(|action| {
            let chords = ws.keymap.keys_for_action(context, *action);
            if chords.is_empty() {
                return None;
            }
            Some(Line::from(vec![
                Span::styled(
                    format!("{:<16}", chords.join(" / ")),
                    Style::default().fg(ws.theme.accent),
                ),
                Span::styled(action.label(), Style::default().fg(ws.theme.fg)),
            ]))
        })
        .collect()
}

fn centered(area: Rect, width_pct: u16, height: u16) -> Rect {
    let width = (area.width * width_pct / 100).max(1).min(area.width);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height.min(area.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::layout;
    use crate::modal::Modal;
    use conductor_core::keymap::KeyMap;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn draw(ws: &Workspace) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| render(frame, ws, &layout(ws, frame.area())))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text_of(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, row)].symbol())
            .collect()
    }

    #[test]
    fn ステータスバーのヒントはキーマップから来る() {
        let mut ws = Workspace::for_test();
        assert!(status_line(&ws).to_string().contains("j/k: nav"));

        let user: toml::Table = toml::from_str(
            "[layers.explorer]\n\"j\" = false\n\"k\" = false\n\"down\" = false\n\"up\" = false\n",
        )
        .unwrap();
        ws.keymap = KeyMap::with_warnings(&user).0;
        let hint = status_line(&ws).to_string();
        assert!(!hint.contains("nav"), "外した束縛が残っている: {hint}");
        assert!(hint.contains("cmds"), "残りの案内まで消えている: {hint}");
    }

    #[test]
    fn ステータスメッセージはヒントより優先する() {
        let mut ws = Workspace::for_test();
        crate::effect::apply(
            &mut ws,
            &mut conductor_svc::Services::new(),
            vec![crate::effect::Effect::Status(
                StatusLevel::Error,
                "cannot open".into(),
            )],
        );
        assert_eq!(status_line(&ws).to_string(), "cannot open");
    }

    #[test]
    fn 枠と全幅の行が描かれる() {
        let ws = Workspace::for_test();
        let buffer = draw(&ws);
        assert!(text_of(&buffer, 0).contains("repo"));
        assert!(text_of(&buffer, 1).contains("Worktree"));
        let panels = (3..40).map(|y| text_of(&buffer, y)).collect::<String>();
        for title in ["Explorer", "Viewer", "Claude Code", "Shell"] {
            assert!(panels.contains(title), "{title} の枠が無い");
        }
        assert!(text_of(&buffer, 39).contains("nav"));
    }

    #[test]
    fn モーダルは他の全てに重ねて描かれる() {
        let mut ws = Workspace::for_test();
        let without = draw(&ws);
        ws.modals.push(Modal::Help);
        let with = draw(&ws);

        let middle = with.area.height / 2;
        assert_ne!(text_of(&without, middle), text_of(&with, middle));
        let all: String = (0..with.area.height).map(|y| text_of(&with, y)).collect();
        assert!(all.contains("Keys"));
        assert!(all.contains("Focus next panel"), "キー一覧が空");
    }
}
