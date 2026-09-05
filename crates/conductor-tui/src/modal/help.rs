//! キーのチートシート。表ではなくキーマップを引くので、ユーザの上書きも含めて
//! 実際に発火するキーだけが並ぶ。

use conductor_core::keymap::{Action, KeyContext};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::workspace::{Ctx, Focus};

/// どのパネルのページを見ているか。数字キーで行き来する。
#[derive(Debug)]
pub struct Help {
    pub page: Focus,
    /// 先頭から落とす行数。層が 3 つあるページは 1 画面に収まらない。
    scroll: usize,
}

/// 数字キーとページの対応。ヘルプの見出しの並びでもある。
const PAGES: [(char, &str, Focus); 5] = [
    ('1', "Worktree", Focus::Worktree),
    ('2', "Explorer", Focus::Explorer),
    ('3', "Viewer", Focus::Viewer),
    ('4', "Terminal", Focus::TerminalClaude),
    ('5', "Review", Focus::Revidere),
];

impl Help {
    pub fn open(focus: Focus) -> Self {
        Self {
            page: focus,
            scroll: 0,
        }
    }

    pub fn update(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                return vec![Effect::PopModal];
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char(c) => {
                if let Some((_, _, focus)) = PAGES.iter().find(|(digit, ..)| *digit == c) {
                    self.page = *focus;
                    self.scroll = 0;
                }
            }
            _ => {}
        }
        Vec::new()
    }
}

pub fn title() -> String {
    "Keys  1-5 page  j/k scroll  esc close".into()
}

/// パネルの層を先に、どこでも効くグローバルを最後に。
fn layers(page: Focus) -> &'static [(&'static str, KeyContext)] {
    match page {
        Focus::Worktree => &[("Worktree panel", KeyContext::Worktree)],
        Focus::Explorer => &[
            ("Explorer \u{2014} file tree", KeyContext::Explorer),
            (
                "Explorer \u{2014} Git Changes",
                KeyContext::ExplorerDiffList,
            ),
            (
                "Explorer \u{2014} comment list",
                KeyContext::ExplorerCommentList,
            ),
        ],
        Focus::Viewer => &[
            ("Viewer", KeyContext::Viewer),
            ("Viewer \u{2014} diff mode", KeyContext::ViewerDiffMode),
        ],
        Focus::TerminalClaude | Focus::TerminalShell => &[("Terminal panel", KeyContext::Terminal)],
        Focus::Editor => &[("Editor panel", KeyContext::Editor)],
        Focus::Revidere => &[
            ("Review \u{2014} sections + diff", KeyContext::Revidere),
            ("Pickers and pop-ups", KeyContext::Overlay),
        ],
    }
}

pub fn lines(help: &Help, ctx: &Ctx, area: Rect) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let mut lines = vec![Line::from(
        PAGES
            .iter()
            .map(|(digit, label, focus)| {
                let on = *focus == help.page
                    || (*focus == Focus::TerminalClaude && help.page == Focus::TerminalShell);
                Span::styled(
                    format!(" {digit}:{label} "),
                    if on {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        Style::default().fg(theme.muted)
                    },
                )
            })
            .collect::<Vec<_>>(),
    )];

    let section = |lines: &mut Vec<Line<'static>>, title: &str, context: KeyContext| {
        let rows: Vec<Line<'static>> = Action::ALL
            .iter()
            .filter_map(|action| {
                let chords = ctx.keymap.keys_in_layer(context, *action);
                (!chords.is_empty()).then(|| {
                    Line::from(vec![
                        Span::styled(
                            format!("  {:<18}", chords.join(" / ")),
                            Style::default().fg(theme.accent),
                        ),
                        Span::styled(action.label(), Style::default().fg(theme.fg)),
                    ])
                })
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        lines.push(Line::from(""));
        lines.push(Line::styled(
            title.to_string(),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ));
        lines.extend(rows);
    };

    for (title, context) in layers(help.page) {
        section(&mut lines, title, *context);
    }
    section(
        &mut lines,
        "Global \u{2014} works anywhere",
        KeyContext::Global,
    );

    // 見出しの行は残したまま、その下だけを送る。枠の 2 行と見出しの 1 行が本文を削る。
    let tabs = lines.remove(0);
    let height = (area.height as usize).saturating_sub(3);
    let skip = help.scroll.min(lines.len().saturating_sub(height));
    std::iter::once(tabs)
        .chain(lines.into_iter().skip(skip))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn text(help: &Help, ws: &Workspace, area: Rect) -> Vec<String> {
        lines(help, &ws.ctx(), area)
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect()
    }

    #[test]
    fn ページごとにその層のキーだけを出す() {
        let ws = Workspace::for_test();
        let area = Rect::new(0, 0, 80, 200);
        let viewer = text(&Help::open(Focus::Viewer), &ws, area).join("\n");
        assert!(viewer.contains("Viewer \u{2014} diff mode"));
        assert!(!viewer.contains("Worktree panel"));
        // 表ではなくキーマップを引くので、案内と実挙動がずれない。
        assert!(viewer.contains("Search in file"));
        assert!(
            viewer.contains("Global \u{2014} works anywhere"),
            "グローバルは常に最後に付く"
        );
    }

    #[test]
    fn 収まらないページは_jで送れる() {
        let ws = Workspace::for_test();
        let area = Rect::new(0, 0, 80, 20);
        let mut help = Help::open(Focus::Viewer);
        let top = text(&help, &ws, area);
        for _ in 0..200 {
            help.update(key(KeyCode::Char('j')));
        }
        let bottom = text(&help, &ws, area);
        assert_ne!(top[1..], bottom[1..], "送っていない");
        assert_eq!(top[0], bottom[0], "見出しの行は残る");
        let whole = text(&Help::open(Focus::Viewer), &ws, Rect::new(0, 0, 80, 500));
        assert_eq!(bottom.last(), whole.last(), "末尾まで届かない");
        assert_eq!(bottom.len(), 18, "窓より多く出さない");
    }

    #[test]
    fn ページを変えると先頭に戻る() {
        let ws = Workspace::for_test();
        let area = Rect::new(0, 0, 80, 20);
        let mut help = Help::open(Focus::Viewer);
        help.update(key(KeyCode::Char('j')));
        help.update(key(KeyCode::Char('1')));
        assert_eq!(help.page, Focus::Worktree);
        assert_eq!(
            text(&help, &ws, area),
            text(&Help::open(Focus::Worktree), &ws, area)
        );
    }
}
