//! メニューバー。タイトルバーの直下に常時 1 行あり、F10 で入る。
//!
//! 項目のラベルがメニューローカルなのは、トップレベルのタイトルが既に文脈を
//! 与えているから。パレットはフラットな一覧なので自己説明的な別のラベルを持つ。

use conductor_core::icons::{self, Glyph, IconSet};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::command::{self, CommandId, Enabled};
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::workspace::Workspace;

pub enum Item {
    Command {
        id: CommandId,
        label: &'static str,
    },
    /// 関連する行の間に置く、選べない区切り。
    Separator,
}

impl Item {
    pub fn command(&self) -> Option<CommandId> {
        match self {
            Item::Command { id, .. } => Some(*id),
            Item::Separator => None,
        }
    }
}

pub struct Menu {
    pub title: &'static str,
    /// Nerd Font が無ければ描かれない。バーは横幅が厳しく、記号で埋めても伝わらない。
    pub icon: Glyph,
    pub items: &'static [Item],
}

const fn cmd(id: CommandId, label: &'static str) -> Item {
    Item::Command { id, label }
}

const SEP: Item = Item::Separator;

/// メニューに載せないと決めたコマンドと、その理由。到達性のテストがここを読むので、
/// メニューに足さずにこの行だけ消すとテストが落ちる。
#[cfg(test)]
pub const UNLISTED: &[(CommandId, &str)] = &[
    (
        CommandId::AddReviewComment,
        "コメントは Viewer で行を選んでから書くもので、メニューから始めても宛先が無い",
    ),
    (
        CommandId::EditComment,
        "対象は一覧で選択中のコメント。選択を持たないメニューからは指せない",
    ),
    (CommandId::ReplyToComment, "EditComment と同じ"),
    (CommandId::DeleteComment, "EditComment と同じ"),
    (CommandId::ToggleCommentResolve, "EditComment と同じ"),
    (CommandId::ViewCommentDetail, "EditComment と同じ"),
    (
        CommandId::ForceAnalyzeRevidere,
        "作り直しは Review current branch の確認が兼ねる。こちらは確認を飛ばす近道",
    ),
];

pub const MENUS: &[Menu] = &[
    Menu {
        title: "Repo",
        icon: icons::MENU_REPO,
        items: &[
            cmd(CommandId::OpenRepo, "Open Repository\u{2026}"),
            cmd(CommandId::SwitchRepo, "Switch Repository\u{2026}"),
            SEP,
            cmd(CommandId::RefreshDiff, "Refresh Diff"),
            cmd(CommandId::RebuildCodeIndex, "Rebuild Code Index"),
            SEP,
            cmd(CommandId::Quit, "Quit Conductor"),
        ],
    },
    Menu {
        title: "Worktree",
        icon: icons::MENU_WORKTREE,
        items: &[
            cmd(CommandId::CreateWorktree, "New Worktree\u{2026}"),
            cmd(CommandId::DeleteWorktree, "Delete Worktree\u{2026}"),
            SEP,
            cmd(CommandId::NextWorktree, "Next Worktree"),
            cmd(CommandId::PrevWorktree, "Previous Worktree"),
            SEP,
            cmd(CommandId::SwitchBranch, "Switch Branch (Remote)\u{2026}"),
            cmd(CommandId::GrabBranch, "Grab Branch\u{2026}"),
            cmd(CommandId::UngrabBranch, "Ungrab Branch"),
            SEP,
            cmd(CommandId::PullWorktree, "Pull (fast-forward)"),
            cmd(CommandId::MergeToMain, "Merge into Main"),
            cmd(CommandId::CherryPick, "Cherry-pick\u{2026}"),
            cmd(CommandId::ResetMainToOrigin, "Reset Main to Origin"),
            SEP,
            cmd(CommandId::PruneWorktrees, "Prune Stale Worktrees"),
            cmd(CommandId::RefreshWorktrees, "Refresh Worktree List"),
            SEP,
            cmd(CommandId::OpenPullRequest, "Open Pull Request in Browser"),
        ],
    },
    // レビューを作る → 読む → コメントを書く → 公開する、の順。
    Menu {
        title: "Review",
        icon: icons::MENU_REVIEW,
        items: &[
            cmd(CommandId::AnalyzeRevidere, "Review current branch"),
            cmd(CommandId::ReviewPullRequest, "Review Pull Request\u{2026}"),
            SEP,
            cmd(CommandId::ShowRevidere, "Show Review"),
            SEP,
            cmd(CommandId::ShowReviewComments, "Show Comments"),
            SEP,
            cmd(
                CommandId::PublishReview,
                "Publish Comments to GitHub\u{2026}",
            ),
        ],
    },
    Menu {
        title: "View",
        icon: icons::MENU_VIEW,
        items: &[
            cmd(CommandId::ShowDiffList, "Changed Files"),
            cmd(CommandId::ShowCommentList, "Comment List"),
            SEP,
            cmd(CommandId::ToggleMarkdownRender, "Markdown: Raw / Rendered"),
            SEP,
            cmd(CommandId::FoldOneLevel, "Fold One Level (zm)"),
            cmd(CommandId::UnfoldOneLevel, "Unfold One Level (zr)"),
            cmd(CommandId::FoldAll, "Fold All (zM)"),
            cmd(CommandId::UnfoldAll, "Unfold All (zR)"),
            SEP,
            cmd(CommandId::SwitchTheme, "Switch Theme\u{2026}"),
            cmd(CommandId::ToggleHighContrast, "Toggle High Contrast"),
        ],
    },
    Menu {
        title: "Panel",
        icon: icons::MENU_PANEL,
        items: &[
            cmd(CommandId::FocusWorktree, "Focus Worktree"),
            cmd(CommandId::FocusExplorer, "Focus Explorer"),
            cmd(CommandId::FocusViewer, "Focus Viewer"),
            cmd(CommandId::FocusTerminalClaude, "Focus Claude Code"),
            cmd(CommandId::FocusTerminalShell, "Focus Shell"),
            SEP,
            cmd(CommandId::TogglePanelExpand, "Maximize / Restore Panel"),
            SEP,
            cmd(CommandId::ResizePaneLeft, "Resize Pane Left"),
            cmd(CommandId::ResizePaneRight, "Resize Pane Right"),
            cmd(CommandId::ResizePaneUp, "Resize Pane Up"),
            cmd(CommandId::ResizePaneDown, "Resize Pane Down"),
        ],
    },
    Menu {
        title: "Search",
        icon: icons::MENU_SEARCH,
        items: &[
            cmd(CommandId::SearchInFile, "Search in File\u{2026}"),
            cmd(CommandId::SearchFullText, "Full-text Search (Grep)\u{2026}"),
        ],
    },
    Menu {
        title: "Terminal",
        icon: icons::MENU_TERMINAL,
        items: &[
            cmd(CommandId::NewClaudeCode, "New Claude Code Session"),
            cmd(CommandId::NewShell, "New Shell Session"),
            cmd(
                CommandId::ResumeClaudeSession,
                "Resume Claude Session\u{2026}",
            ),
            SEP,
            cmd(CommandId::SaveSessionHistory, "Save Terminal Output"),
            cmd(CommandId::SessionHistory, "Saved Terminal Output\u{2026}"),
        ],
    },
    Menu {
        title: "Help",
        icon: icons::MENU_HELP,
        items: &[
            cmd(CommandId::ToggleHelp, "Keyboard Shortcuts"),
            SEP,
            cmd(CommandId::CheckForUpdate, "Check for Updates"),
            cmd(CommandId::UpdateAndRestart, "Update and Restart"),
        ],
    },
];

/// バーは 3 状態。F10 はメニューを開かずバーにフォーカスするだけで、そこから
/// 矢印でタイトルを眺め Down/Enter で開く。GTK や Windows と同じ作法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuBar {
    #[default]
    Closed,
    Bar {
        index: usize,
    },
    Open {
        index: usize,
        selected: usize,
    },
}

impl MenuBar {
    /// 入力を握っているか。true の間は全てのキーがメニューのものになる。
    pub fn is_active(self) -> bool {
        self != MenuBar::Closed
    }

    /// ハイライトされているトップレベル。
    pub fn index(self) -> Option<usize> {
        match self {
            MenuBar::Closed => None,
            MenuBar::Bar { index } | MenuBar::Open { index, .. } => Some(index),
        }
    }

    pub fn open_index(self) -> Option<usize> {
        match self {
            MenuBar::Open { index, .. } => Some(index),
            _ => None,
        }
    }

    fn open(index: usize) -> Self {
        MenuBar::Open {
            index,
            selected: first_selectable(items(index)),
        }
    }
}

fn items(index: usize) -> &'static [Item] {
    MENUS.get(index).map_or(&[], |menu| menu.items)
}

pub fn first_selectable(items: &[Item]) -> usize {
    items
        .iter()
        .position(|i| i.command().is_some())
        .unwrap_or(0)
}

pub fn last_selectable(items: &[Item]) -> usize {
    items
        .iter()
        .rposition(|i| i.command().is_some())
        .unwrap_or(0)
}

/// 区切りを飛ばして 1 行動かす。両端では回り込む。
///
/// 使えない行もあえて選べるままにしてある。飛ばすと行の存在ごと隠れてしまい、
/// それは灰色にする目的の逆になる。
pub fn step_item(items: &[Item], from: usize, down: bool) -> usize {
    let n = items.len();
    if n == 0 {
        return 0;
    }
    let mut index = from.min(n - 1);
    for _ in 0..n {
        index = if down {
            (index + 1) % n
        } else {
            (index + n - 1) % n
        };
        if items[index].command().is_some() {
            return index;
        }
    }
    from
}

pub fn step_menu(from: usize, down: bool) -> usize {
    let n = MENUS.len();
    if n == 0 {
        return 0;
    }
    if down {
        (from + 1) % n
    } else {
        (from + n - 1) % n
    }
}

pub fn find_by_initial(items: &[Item], from: usize, ch: char) -> Option<usize> {
    let n = items.len();
    let target = ch.to_ascii_lowercase();
    (1..=n)
        .map(|off| (from + off) % n)
        .find(|&i| match &items[i] {
            Item::Command { label, .. } => label
                .chars()
                .next()
                .is_some_and(|c| c.to_ascii_lowercase() == target),
            Item::Separator => false,
        })
}

fn menu_by_initial(ch: char) -> Option<usize> {
    let target = ch.to_ascii_lowercase();
    MENUS.iter().position(|menu| {
        menu.title
            .chars()
            .next()
            .is_some_and(|c| c.to_ascii_lowercase() == target)
    })
}

/// メニューが入力を握っている間のキー。
///
/// Esc は 1 段ずつ (ドロップダウン → バー → アプリ) 戻る。誤って Down を押しても
/// 分かりやすく引き返せる。
pub fn key(ws: &mut Workspace, key: KeyEvent) -> Vec<Effect> {
    match ws.chrome.menu {
        MenuBar::Closed => Vec::new(),
        MenuBar::Bar { index } => {
            ws.chrome.menu = match key.code {
                KeyCode::Esc => MenuBar::Closed,
                KeyCode::Left => MenuBar::Bar {
                    index: step_menu(index, false),
                },
                KeyCode::Right => MenuBar::Bar {
                    index: step_menu(index, true),
                },
                KeyCode::Home => MenuBar::Bar { index: 0 },
                KeyCode::End => MenuBar::Bar {
                    index: MENUS.len() - 1,
                },
                KeyCode::Down | KeyCode::Enter | KeyCode::Char(' ') => MenuBar::open(index),
                // 行き先を知っている人には頭文字が最速の経路。
                KeyCode::Char(c) => match menu_by_initial(c) {
                    Some(index) => MenuBar::open(index),
                    None => ws.chrome.menu,
                },
                _ => ws.chrome.menu,
            };
            Vec::new()
        }
        MenuBar::Open { index, selected } => {
            let items = items(index);
            let step = |selected| MenuBar::Open { index, selected };
            ws.chrome.menu = match key.code {
                KeyCode::Esc => MenuBar::Bar { index },
                KeyCode::Left => MenuBar::open(step_menu(index, false)),
                KeyCode::Right => MenuBar::open(step_menu(index, true)),
                KeyCode::Up => step(step_item(items, selected, false)),
                KeyCode::Down => step(step_item(items, selected, true)),
                KeyCode::Home => step(first_selectable(items)),
                KeyCode::End => step(last_selectable(items)),
                KeyCode::Enter => return activate(ws, index, selected),
                KeyCode::Char(c) => match find_by_initial(items, selected, c) {
                    Some(next) => step(next),
                    None => ws.chrome.menu,
                },
                _ => ws.chrome.menu,
            };
            Vec::new()
        }
    }
}

/// 行のコマンドを実行する。灰色の行は閉じるだけで何も起きない。
///
/// 実行の前にメニューを閉じる。コマンドはモーダルを積むので、後から閉じると
/// そのコマンドが立てた状態まで一緒に落ちる。
fn activate(ws: &mut Workspace, index: usize, selected: usize) -> Vec<Effect> {
    let Some(id) = items(index).get(selected).and_then(Item::command) else {
        return Vec::new();
    };
    ws.chrome.menu = MenuBar::Closed;
    if command::enabled(ws, id).is_yes() {
        vec![Effect::Command(id)]
    } else {
        Vec::new()
    }
}

/// クリックが何を意味するか。記録した座標ではなく [MENUS] と区画から決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Activate {
        index: usize,
        item: usize,
    },
    Open(usize),
    Close,
    /// 消費するが何も起きない。惜しいクリックで閉じないよう、開いたままにする。
    Inert,
    /// メニューへのクリックではない。呼び出し側が続きを捌く。
    Pass,
}

pub fn classify_click(ws: &Workspace, layout: &Layout, col: u16, row: u16) -> Click {
    let Some(bar) = layout.rect(Region::MenuBar) else {
        return Click::Pass;
    };
    if let Some(index) = ws.chrome.menu.open_index() {
        let area = dropdown_rect(ws, layout, index);
        if contains(area, col, row) {
            return match item_at(ws, layout, index, row) {
                Some(item) => Click::Activate { index, item },
                None => Click::Inert,
            };
        }
    }
    if row == bar.y {
        return match title_at(ws.config.ui.icon_set(), bar, col) {
            Some(index) if ws.chrome.menu.open_index() == Some(index) => Click::Close,
            Some(index) => Click::Open(index),
            None => Click::Close,
        };
    }
    // 閉じるためのクリックは飲み込む。下にあったものまで押してはいけない。
    if ws.chrome.menu.is_active() {
        Click::Close
    } else {
        Click::Pass
    }
}

pub fn click(ws: &mut Workspace, layout: &Layout, col: u16, row: u16) -> Option<Vec<Effect>> {
    match classify_click(ws, layout, col, row) {
        Click::Activate { index, item } => Some(activate(ws, index, item)),
        Click::Open(index) => {
            ws.chrome.menu = MenuBar::open(index);
            Some(Vec::new())
        }
        Click::Close => {
            ws.chrome.menu = MenuBar::Closed;
            Some(Vec::new())
        }
        Click::Inert => Some(Vec::new()),
        Click::Pass => None,
    }
}

fn contains(rect: Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn width(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

fn title_text(menu: &Menu, icons: IconSet) -> String {
    format!(" {}{} ", menu.icon.labeled(icons), menu.title)
}

/// バー上でのタイトルの位置。描画とヒット判定の両方がここから引く。
pub fn title_spans(icons: IconSet, bar: Rect) -> Vec<(usize, Rect)> {
    let mut x = bar.x;
    let mut out = Vec::new();
    for (index, menu) in MENUS.iter().enumerate() {
        let w = width(&title_text(menu, icons));
        // はみ出す前に止める。途中で切れたタイトルは画面と当たり判定がずれる。
        if x + w > bar.x + bar.width {
            break;
        }
        out.push((index, Rect::new(x, bar.y, w, 1)));
        x += w;
    }
    out
}

pub fn title_at(icons: IconSet, bar: Rect, col: u16) -> Option<usize> {
    title_spans(icons, bar)
        .into_iter()
        .find(|(_, rect)| col >= rect.x && col < rect.x + rect.width)
        .map(|(index, _)| index)
}

/// 開いているドロップダウンの矩形 (枠込み)。バーの下に垂れ下がり、画面の下端で切れる。
pub fn dropdown_rect(ws: &Workspace, layout: &Layout, index: usize) -> Rect {
    let Some(bar) = layout.rect(Region::MenuBar) else {
        return Rect::default();
    };
    let icons = ws.config.ui.icon_set();
    let anchor = title_spans(icons, bar)
        .into_iter()
        .find(|(i, _)| *i == index)
        .map_or(bar.x, |(_, rect)| rect.x);
    let items = items(index);
    let w = items
        .iter()
        .filter_map(|item| match item {
            Item::Command { label, id } => Some(width(label) + width(&chord_for(ws, *id)) + 6),
            Item::Separator => None,
        })
        .max()
        .unwrap_or(10)
        .min(layout.area.width);
    let y = bar.y + bar.height;
    let h = (items.len() as u16 + 2).min((layout.area.y + layout.area.height).saturating_sub(y));
    Rect::new(anchor.min(layout.area.width.saturating_sub(w)), y, w, h)
}

fn chord_for(ws: &Workspace, id: CommandId) -> String {
    command::find(id)
        .action
        .and_then(|a| crate::render::representative_chord(&ws.keymap, ws.key_context(), a))
        .unwrap_or_default()
}

/// ドロップダウンの画面行 row にある項目。枠と区切りは None。
pub fn item_at(ws: &Workspace, layout: &Layout, index: usize, row: u16) -> Option<usize> {
    let area = dropdown_rect(ws, layout, index);
    let offset = row.checked_sub(area.y + 1)? as usize;
    let item = visible_from(ws, index, area) + offset;
    (offset + 2 < area.height as usize)
        .then(|| items(index).get(item))
        .flatten()
        .and_then(Item::command)
        .map(|_| item)
}

fn visible_from(ws: &Workspace, index: usize, area: Rect) -> usize {
    let MenuBar::Open { selected, .. } = ws.chrome.menu else {
        return 0;
    };
    crate::modal::picker::scroll_for(
        selected,
        items(index).len(),
        (area.height as usize).saturating_sub(2),
    )
}

pub fn bar_line(ws: &Workspace, bar: Rect) -> Line<'static> {
    let theme = &ws.theme;
    let icons = ws.config.ui.icon_set();
    let active = ws.chrome.menu.index();
    Line::from(
        title_spans(icons, bar)
            .into_iter()
            .map(|(index, _)| {
                let style = if active == Some(index) {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if ws.chrome.menu.is_active() {
                    Style::default().fg(theme.fg)
                } else {
                    Style::default().fg(theme.muted)
                };
                Span::styled(title_text(&MENUS[index], icons), style)
            })
            .collect::<Vec<_>>(),
    )
}

pub fn dropdown_lines(ws: &Workspace, area: Rect, index: usize) -> Vec<Line<'static>> {
    let theme = &ws.theme;
    let MenuBar::Open { selected, .. } = ws.chrome.menu else {
        return Vec::new();
    };
    let inner = area.width.saturating_sub(2) as usize;
    let height = (area.height as usize).saturating_sub(2);
    let start = visible_from(ws, index, area);
    items(index)
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, item)| match item {
            Item::Separator => Line::styled(
                "\u{2500}".repeat(inner),
                Style::default().fg(theme.border_unfocused),
            ),
            Item::Command { id, label } => {
                let on = matches!(command::enabled(ws, *id), Enabled::Yes);
                // 使えない行のチョードは出さない。今は何もしないキーの案内は嘘になる。
                let chord = if on {
                    chord_for(ws, *id)
                } else {
                    String::new()
                };
                let gap = inner.saturating_sub(width(label) as usize + width(&chord) as usize + 2);
                let label_style = if on {
                    Style::default().fg(theme.fg)
                } else {
                    // muted は同梱テーマのいくつかで背景に近く、行ごと消える。
                    Style::default().fg(theme.fg).add_modifier(Modifier::DIM)
                };
                crate::list::row_line(
                    vec![
                        Span::styled(format!(" {label}"), label_style),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(format!("{chord} "), Style::default().fg(theme.hint)),
                    ],
                    theme,
                    i == selected,
                    true,
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::COMMANDS;

    fn listed() -> Vec<CommandId> {
        MENUS
            .iter()
            .flat_map(|menu| menu.items.iter())
            .filter_map(Item::command)
            .collect()
    }

    fn sample() -> Vec<Item> {
        vec![
            cmd(CommandId::Quit, "Alpha"),
            SEP,
            cmd(CommandId::OpenRepo, "Beta"),
            cmd(CommandId::SwitchRepo, "Alto"),
        ]
    }

    #[test]
    fn 全コマンドがメニューから辿れる() {
        let listed = listed();
        let excused: Vec<CommandId> = UNLISTED.iter().map(|(id, _)| *id).collect();
        let missing: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| !listed.contains(&c.id) && !excused.contains(&c.id))
            .map(|c| c.label)
            .collect();
        assert!(
            missing.is_empty(),
            "these commands are on no menu and are not excused in UNLISTED: {missing:?}"
        );
    }

    #[test]
    fn 載せない決めのコマンドは実際に載っていない() {
        let listed = listed();
        for (id, reason) in UNLISTED {
            assert!(!listed.contains(id), "{id:?} is on a menu ({reason})");
        }
    }

    #[test]
    fn 同じコマンドが2つのメニューに出ない() {
        let listed = listed();
        for (i, id) in listed.iter().enumerate() {
            assert!(!listed[i + 1..].contains(id), "{id:?} is on two menus");
        }
    }

    #[test]
    fn メニューの行は全部コマンド表にある() {
        for id in listed() {
            assert!(
                COMMANDS.iter().any(|c| c.id == id),
                "{id:?} is not a command"
            );
        }
    }

    #[test]
    fn どのメニューも空でなく区切りで始まらず終わらない() {
        for menu in MENUS {
            let selectable: Vec<bool> = menu.items.iter().map(|i| i.command().is_some()).collect();
            assert_eq!(selectable.first(), Some(&true), "{}", menu.title);
            assert_eq!(selectable.last(), Some(&true), "{}", menu.title);
            assert!(
                !selectable.windows(2).any(|w| !w[0] && !w[1]),
                "{} has two separators in a row",
                menu.title
            );
        }
    }

    #[test]
    fn 選択の移動は区切りを飛ばし両端で回り込む() {
        let items = sample();
        assert_eq!(step_item(&items, 0, true), 2);
        assert_eq!(step_item(&items, 2, false), 0);
        assert_eq!(step_item(&items, 3, true), 0);
        assert_eq!(step_item(&items, 0, false), 3);
    }

    #[test]
    fn 退化した入力では選択が動かない() {
        let separators = vec![SEP, SEP];
        assert_eq!(step_item(&separators, 0, true), 0);
        assert_eq!(step_item(&separators, 1, false), 1);
        assert_eq!(step_item(&[], 0, true), 0);
        assert!(step_item(&sample(), 99, true) < sample().len());
    }

    #[test]
    fn 最初と最後の選べる行を見つける() {
        let items = sample();
        assert_eq!((first_selectable(&items), last_selectable(&items)), (0, 3));
        let leading = vec![SEP, cmd(CommandId::Quit, "Only")];
        assert_eq!(
            (first_selectable(&leading), last_selectable(&leading)),
            (1, 1)
        );
    }

    #[test]
    fn 頭文字検索は大小を無視して回り込み区切りに着地しない() {
        let items = sample();
        assert_eq!(
            find_by_initial(&items, 0, 'a'),
            Some(3),
            "自分自身は選ばない"
        );
        assert_eq!(find_by_initial(&items, 3, 'A'), Some(0));
        assert_eq!(find_by_initial(&items, 0, 'b'), Some(2));
        assert_eq!(find_by_initial(&items, 0, 'z'), None);
        for ch in ['a', 'b', 'z'] {
            if let Some(i) = find_by_initial(&items, 0, ch) {
                assert!(items[i].command().is_some());
            }
        }
    }

    #[test]
    fn メニューの移動は両方向に回り込む() {
        assert_eq!(step_menu(MENUS.len() - 1, true), 0);
        assert_eq!(step_menu(0, false), MENUS.len() - 1);
    }

    // 幾何 — 描いた場所とクリックの解釈が一致すること。

    fn workspace() -> (Workspace, Layout) {
        let ws = Workspace::for_test();
        let layout = crate::layout::layout(&ws, Rect::new(0, 0, 120, 40));
        (ws, layout)
    }

    #[test]
    fn バーのタイトルは自分の位置で引ける() {
        let (ws, layout) = workspace();
        let bar = layout.rect(Region::MenuBar).unwrap();
        let icons = ws.config.ui.icon_set();
        let spans = title_spans(icons, bar);
        assert_eq!(spans.len(), MENUS.len(), "120 桁なら全部載る");
        for (index, rect) in spans {
            for col in [rect.x, rect.x + rect.width - 1] {
                assert_eq!(title_at(icons, bar, col), Some(index), "{index} at {col}");
            }
        }
    }

    #[test]
    fn 狭いバーは載らないタイトルを当たり判定に残さない() {
        let ws = Workspace::for_test();
        let bar = Rect::new(0, 1, 12, 1);
        let icons = ws.config.ui.icon_set();
        let spans = title_spans(icons, bar);
        assert!(spans.len() < MENUS.len());
        assert_eq!(title_at(icons, bar, 119), None);
    }

    #[test]
    fn ドロップダウンの行はクリックで同じ項目に戻る() {
        let (mut ws, layout) = workspace();
        ws.chrome.menu = MenuBar::open(1);
        let area = dropdown_rect(&ws, &layout, 1);
        assert!(area.height >= 3);
        for (index, item) in items(1).iter().enumerate() {
            let row = area.y + 1 + index as u16;
            let hit = item_at(&ws, &layout, 1, row);
            assert_eq!(hit, item.command().map(|_| index), "row {row}");
        }
        assert_eq!(item_at(&ws, &layout, 1, area.y), None, "上の枠線");
    }

    #[test]
    fn クリックの意味は開いている場所で決まる() {
        let (mut ws, layout) = workspace();
        let bar = layout.rect(Region::MenuBar).unwrap();
        assert_eq!(classify_click(&ws, &layout, 0, 30), Click::Pass);
        assert_eq!(classify_click(&ws, &layout, 1, bar.y), Click::Open(0));

        ws.chrome.menu = MenuBar::open(0);
        assert_eq!(
            classify_click(&ws, &layout, 1, bar.y),
            Click::Close,
            "同じタイトルの再クリックは閉じる"
        );
        assert_eq!(classify_click(&ws, &layout, 0, 30), Click::Close);
        let area = dropdown_rect(&ws, &layout, 0);
        assert_eq!(
            classify_click(&ws, &layout, area.x + 1, area.y + 1),
            Click::Activate { index: 0, item: 0 }
        );
        assert_eq!(
            classify_click(&ws, &layout, area.x, area.y),
            Click::Inert,
            "枠は消費するだけ"
        );
    }

    #[test]
    fn 灰色の行は選べるが実行されない() {
        let mut ws = Workspace::for_test();
        // Repo メニューの Switch Repository は既知が 1 つのとき使えない。
        ws.chrome.menu = MenuBar::Open {
            index: 0,
            selected: 1,
        };
        assert!(!command::enabled(&ws, CommandId::SwitchRepo).is_yes());
        let effects = key(&mut ws, KeyEvent::from(KeyCode::Enter));
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(ws.chrome.menu, MenuBar::Closed, "閉じるところまでは同じ");
    }
}
