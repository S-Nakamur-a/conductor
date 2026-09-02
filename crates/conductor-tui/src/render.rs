//! 画面を描く唯一の入口。`&Workspace` しか取らないので、描画が次フレームの入力の
//! 前提を作ることがない。区画は [crate::layout] が先に決めている。

use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use conductor_core::keymap::{Action, KeyContext, KeyMap};

use crate::layout::{Layout, Region};
use crate::modal::Modal;
use crate::workspace::{Focus, StatusLevel, Workspace};

pub fn render(frame: &mut Frame, ws: &Workspace, layout: &Layout) {
    for (region, rect) in &layout.regions {
        let rect = *rect;
        match region {
            Region::TitleBar => frame.render_widget(Paragraph::new(title_line(ws)), rect),
            Region::MenuBar => {
                frame.render_widget(Paragraph::new(crate::menu::bar_line(ws, rect)), rect)
            }
            Region::WorktreeStrip => frame.render_widget(
                Paragraph::new(crate::panels::worktree::render::strip(ws, rect)),
                rect,
            ),
            Region::StatusBar => frame.render_widget(Paragraph::new(status_line(ws)), rect),
            // Explorer の 2 区画は枠だけ先に描き、中身は 2 つ揃ったところで 1 回描く。
            Region::ExplorerTree | Region::ExplorerChanges => {
                frame.render_widget(panel_block(ws, *region), rect)
            }
            panel => {
                frame.render_widget(panel_block(ws, *panel), rect);
                match panel {
                    Region::TerminalClaude | Region::TerminalShell => {
                        crate::panels::terminal::render::pane(frame, rect, ws, *panel)
                    }
                    Region::Viewer => crate::panels::viewer::render::render(frame, rect, ws),
                    _ => {}
                }
            }
        }
    }

    if let (Some(tree), Some(changes)) = (
        layout.rect(Region::ExplorerTree),
        layout.rect(Region::ExplorerChanges),
    ) {
        crate::panels::explorer::render::render(frame, tree, changes, ws);
    }

    // worktree は全幅のストリップに収まらないので、フォーカス中だけ一覧を重ねる。
    if ws.focus == Focus::Worktree {
        crate::panels::worktree::render::list(frame, frame.area(), ws);
    }
    if let Some(modal) = ws.modals.last() {
        render_modal(frame, ws, modal, frame.area());
    }
    // メニューは全ての上。モーダルの上でもあるのは、開いたままモーダルへ
    // 進む経路がないので、見えているなら必ずそれが最前面だから。
    if let Some(index) = ws.chrome.menu.open_index() {
        let rect = crate::menu::dropdown_rect(ws, layout, index);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(crate::menu::dropdown_lines(ws, rect, index))
                .block(modal_block(ws, crate::menu::MENUS[index].title)),
            rect,
        );
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
            ws.branch().to_string(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(theme.muted)),
        Span::styled(
            ws.repo.root.display().to_string(),
            Style::default().fg(theme.dir_fg),
        ),
    ])
}

fn panel_block(ws: &Workspace, region: Region) -> Block<'static> {
    let focused = focused_region(ws) == region;
    let border = if focused {
        Style::default().fg(ws.theme.border_focused)
    } else {
        Style::default().fg(ws.theme.border_unfocused)
    };
    let explorer = &ws.panels.explorer;
    let title = match region {
        Region::ExplorerTree => {
            crate::panels::explorer::render::tree_title(explorer, explorer.tree_viewport().height)
        }
        Region::ExplorerChanges => crate::panels::explorer::render::bottom_title(explorer, ws),
        Region::Viewer => " Viewer ".to_string(),
        Region::TerminalClaude => " Claude Code ".to_string(),
        Region::TerminalShell => " Shell ".to_string(),
        _ => String::new(),
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(
            title,
            if focused {
                border.add_modifier(Modifier::BOLD)
            } else {
                border
            },
        ))
}

/// フォーカスが枠を光らせる区画。Editor と Revidere は Viewer の場所を借りる。
fn focused_region(ws: &Workspace) -> Region {
    match ws.focus {
        Focus::Worktree => Region::WorktreeStrip,
        Focus::Explorer => match ws.panels.explorer.pane() {
            crate::panels::explorer::Pane::Tree => Region::ExplorerTree,
            crate::panels::explorer::Pane::Bottom => Region::ExplorerChanges,
        },
        Focus::Viewer | Focus::Editor | Focus::Revidere => Region::Viewer,
        Focus::TerminalClaude => Region::TerminalClaude,
        Focus::TerminalShell => Region::TerminalShell,
    }
}

/// 消える前に色を落とし始めるまで。読み終えた合図が最後まで目を引かないようにする。
const STATUS_FADE: std::time::Duration = std::time::Duration::from_millis(2500);

/// フラッシュメッセージ、無ければフォーカス中パネルのキーヒント。
pub fn status_line(ws: &Workspace) -> Line<'static> {
    let theme = &ws.theme;
    match &ws.chrome.status {
        Some(msg) if msg.shown_at.elapsed() >= STATUS_FADE => {
            Line::styled(msg.text.clone(), Style::default().fg(theme.muted))
        }
        Some(msg) => Line::styled(
            msg.text.clone(),
            Style::default()
                .fg(match msg.level {
                    StatusLevel::Success => theme.success,
                    StatusLevel::Error => theme.error,
                    StatusLevel::Warning => theme.warning,
                    StatusLevel::Info => theme.info,
                })
                .add_modifier(Modifier::BOLD),
        ),
        None => Line::styled(
            key_hint(ws.focus, ws.key_context(), &ws.keymap),
            Style::default().fg(theme.hint),
        ),
    }
}

/// フッターのキーヒントを今のキーマップから組み立てる。
///
/// 未割り当てのアクションだけのエントリは落ちるので、発火しないキーを案内しない。
fn key_hint(focus: Focus, context: KeyContext, keymap: &KeyMap) -> String {
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
        // diff を見ている間はハンク送りと文脈の展開が主役になる。案内も入れ替える。
        Focus::Viewer if context == KeyContext::ViewerDiffMode => &[
            ("scroll", &[Action::NavigateDown, Action::NavigateUp]),
            ("hunk", &[Action::NextHunk, Action::PrevHunk]),
            ("expand", &[Action::ExpandContext]),
            ("split", &[Action::ToggleDiffView]),
            ("viewed", &[Action::ToggleViewed]),
            ("file", &[Action::NextChangedFile]),
            ("back", &[Action::ExitToExplorer]),
        ],
        Focus::Viewer => &[
            ("scroll", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("search", &[Action::SearchInFile]),
            ("fold", &[Action::FoldPrefix]),
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
pub fn representative_chord(
    keymap: &KeyMap,
    context: KeyContext,
    action: Action,
) -> Option<String> {
    keymap
        .keys_for_action(context, action)
        .into_iter()
        .filter(|c| c.is_ascii())
        .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

/// 全画面のコメント一覧が占める矩形。ヒットジオメトリを描画の副産物にしないよう、
/// [crate::workspace::Workspace::sync_layout] も同じ関数を引く。
pub fn comment_list_rect(area: Rect) -> Rect {
    let height = (area.height * 80 / 100).max(3);
    centered(area, 70, height)
}

fn render_modal(frame: &mut Frame, ws: &Workspace, modal: &Modal, area: Rect) {
    let big = centered(area, 76, (area.height * 84 / 100).max(3));
    if let Modal::CommentList(list) = modal {
        let rect = comment_list_rect(area);
        let inner = crate::list::inner(rect);
        let lines = crate::comment_list::lines(
            list,
            &ws.review,
            &ws.theme,
            ws.config.ui.icon_set(),
            inner.height as usize,
            true,
        );
        let block = modal_block(
            ws,
            &crate::comment_list::title(&ws.review, ws.config.ui.icon_set()),
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
        return;
    }
    let ctx = ws.ctx();
    let full = |body: Vec<Line<'static>>, title: String| (title, body);
    let (title, body) = match modal {
        Modal::Help(help) => full(
            crate::modal::help::lines(help, &ctx, big),
            crate::modal::help::title(),
        ),
        Modal::Palette(palette) => full(
            crate::modal::palette::lines(palette, ws, big),
            crate::modal::palette::title(),
        ),
        Modal::ThemePicker(picker) => full(
            crate::modal::theme::lines(picker, &ctx),
            crate::modal::theme::title(),
        ),
        Modal::RepoPicker(picker) => full(
            crate::modal::repo::lines(picker, &ctx),
            crate::modal::repo::title(),
        ),
        Modal::Resume(picker) => full(
            crate::modal::session::lines(picker, &ctx, big),
            crate::modal::session::title(picker),
        ),
        Modal::History(browser) => full(
            crate::modal::history::lines(browser, &ctx, big),
            crate::modal::history::title(),
        ),
        Modal::Grep(grep) => full(
            crate::modal::grep::lines(grep, ws, big),
            crate::modal::grep::title(grep),
        ),
        Modal::BranchPicker(picker) => full(
            crate::modal::branch::lines(picker, &ctx, big),
            crate::modal::branch::title(picker),
        ),
        Modal::CherryPick(picker) => full(
            crate::modal::commits::lines(picker, &ctx, big),
            crate::modal::commits::title(picker),
        ),
        Modal::Publish(confirm) => full(
            crate::modal::publish::lines(confirm, &ctx),
            crate::modal::publish::title(confirm),
        ),
        Modal::PrInput(prompt) => (
            crate::modal::pr::title(),
            crate::modal::pr::lines(prompt, &ctx, area.width * 60 / 100),
        ),
        Modal::Prompt(prompt) => (
            prompt.title.clone(),
            crate::modal::input::with_caret(&prompt.input, area.width as usize * 60 / 100)
                .into_iter()
                .map(|line| Line::from(format!("> {line}")))
                .collect(),
        ),
        Modal::Confirm(confirm) => (
            "Confirm".to_string(),
            vec![
                Line::from(confirm.question.clone()),
                Line::from(""),
                Line::styled("y / n", Style::default().fg(ws.theme.hint)),
            ],
        ),
        Modal::CommentEditor(editor) => (
            editor.title(),
            editor_lines(editor, &ws.theme, area.width * 60 / 100),
        ),
        Modal::CommentList(_) => unreachable!("上で返している"),
    };

    // 一覧を持つモーダルは画面を大きく取る。中身の丈で決めると、絞り込むたびに
    // 枠が伸び縮みして読みにくい。
    let rect = if wide(modal) {
        big
    } else {
        centered(area, 60, (body.len() as u16 + 2).min(area.height))
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(body).block(modal_block(ws, &title)), rect);
}

fn wide(modal: &Modal) -> bool {
    matches!(
        modal,
        Modal::Help(_)
            | Modal::Palette(_)
            | Modal::ThemePicker(_)
            | Modal::RepoPicker(_)
            | Modal::Resume(_)
            | Modal::History(_)
            | Modal::Grep(_)
            | Modal::BranchPicker(_)
            | Modal::CherryPick(_)
            | Modal::Publish(_)
    )
}

fn modal_block(ws: &Workspace, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ws.theme.border_focused))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(ws.theme.fg)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Reset))
}

/// 本文とキーの案内。
fn editor_lines(
    editor: &crate::modal::CommentEditor,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> =
        crate::modal::input::with_caret(&editor.input, width as usize)
            .into_iter()
            .map(|line| Line::styled(line, Style::default().fg(theme.fg)))
            .collect();
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "shift+enter: newline  \u{b7}  enter: save  \u{b7}  esc: cancel",
        Style::default().fg(theme.hint),
    ));
    lines
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

    /// 案内は ASCII だけ。macOS の alt グリフ (˙ や †) は端末で読めないことがある。
    #[test]
    fn 代表キーはunicodeより短いasciiを選ぶ() {
        let ws = Workspace::for_test();
        assert_eq!(
            representative_chord(&ws.keymap, KeyContext::Global, Action::CycleFocusBackward),
            Some("alt+h".to_string())
        );
        assert_eq!(
            representative_chord(&ws.keymap, KeyContext::Worktree, Action::NavigateDown),
            Some("j".to_string()),
            "別名の down より素の j"
        );
    }

    #[test]
    fn パネルごとのヒントは実際のキーだけを出す() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::Worktree;
        let hint = status_line(&ws).to_string();
        assert!(
            hint.contains("j/k: nav") && hint.contains("tab: panel"),
            "{hint}"
        );
        assert!(hint.contains("w: new"), "{hint}");
        assert!(hint.is_ascii(), "{hint}");

        ws.focus = Focus::TerminalClaude;
        let hint = status_line(&ws).to_string();
        assert!(hint.contains("keys \u{2192} terminal"), "{hint}");
        assert!(
            hint.contains("ctrl+esc: leave"),
            "esc ではなく ctrl+esc: {hint}"
        );
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
        ws.modals
            .push(Modal::Help(crate::modal::help::Help::open(ws.focus)));
        let with = draw(&ws);

        let middle = with.area.height / 2;
        assert_ne!(text_of(&without, middle), text_of(&with, middle));
        let all: String = (0..with.area.height).map(|y| text_of(&with, y)).collect();
        assert!(all.contains("Keys"));
        assert!(all.contains("Focus next panel"), "キー一覧が空");
    }
}
