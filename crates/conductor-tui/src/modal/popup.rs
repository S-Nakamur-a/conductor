//! 押した場所の直下に開く、行き先を 1 つ選ぶポップアップ。
//!
//! 画面中央ではなく押した場所の隣に出すため、[rect] が矩形を決める。中央に置くと
//! 「どの区画の話か」が消えて、押した手応えが返らない。押した場所の無いキー操作から
//! 開くときだけ中央に置く。

use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::workspace::Ctx;

pub type Choices = Vec<(String, Effect)>;

#[derive(Debug)]
pub struct Popup {
    title: &'static str,
    choices: Choices,
    /// 枠の左上。None は画面の中央。
    at: Option<(u16, u16)>,
    selected: usize,
    /// 数字か、矢印の後の Enter でだけ答え、ほかのキーでは何もせずに閉じる。
    numbered: bool,
    /// 一覧に誤って打った文は矢印を含まないので、その改行では選ばせない。
    moved: bool,
}

impl Popup {
    /// current は開いた時点で合わせておく行。
    pub fn new(
        title: &'static str,
        choices: Choices,
        at: Option<(u16, u16)>,
        current: usize,
    ) -> Self {
        let selected = current.min(choices.len() - 1);
        Self {
            title,
            choices,
            at,
            selected,
            numbered: false,
            moved: false,
        }
    }

    pub fn numbered(title: &'static str, choices: Choices, at: Option<(u16, u16)>) -> Self {
        Self {
            numbered: true,
            ..Self::new(title, choices, at, 0)
        }
    }

    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        let last = self.choices.len() - 1;
        match key.code {
            KeyCode::Down => self.select((self.selected + 1) % self.choices.len()),
            KeyCode::Up => self.select(self.selected.checked_sub(1).unwrap_or(last)),
            KeyCode::Char('j') if !self.numbered => {
                self.select((self.selected + 1) % self.choices.len())
            }
            KeyCode::Char('k') if !self.numbered => {
                self.select(self.selected.checked_sub(1).unwrap_or(last))
            }
            KeyCode::Enter if self.numbered && !self.moved => vec![Effect::PopModal],
            KeyCode::Enter => self.choose(self.selected),
            KeyCode::Char(c @ '1'..='9') if self.numbered => {
                self.choose(usize::from(c as u8 - b'1'))
            }
            KeyCode::Esc => vec![Effect::PopModal],
            _ if self.numbered => vec![Effect::PopModal],
            _ => Vec::new(),
        }
    }

    /// 枠の内側の行をクリックして選ぶ。外なら閉じるだけ。
    pub fn click(&mut self, x: u16, y: u16, area: Rect) -> Vec<Effect> {
        let rect = rect(self, area);
        let inner = crate::list::inner(rect);
        if !(inner.x..inner.right()).contains(&x) {
            return vec![Effect::PopModal];
        }
        match y.checked_sub(inner.y).map(usize::from) {
            Some(row) if row < self.choices.len() => self.choose(row),
            _ => vec![Effect::PopModal],
        }
    }

    fn select(&mut self, row: usize) -> Vec<Effect> {
        self.selected = row;
        self.moved = true;
        Vec::new()
    }

    /// 選んだら閉じるので、行き先は取り出してしまってよい。
    fn choose(&mut self, row: usize) -> Vec<Effect> {
        if row >= self.choices.len() {
            return vec![Effect::PopModal];
        }
        vec![Effect::PopModal, self.choices.swap_remove(row).1]
    }
}

pub fn title(popup: &Popup) -> String {
    popup.title.to_string()
}

pub fn lines(popup: &Popup, theme: &Theme) -> Vec<Line<'static>> {
    popup
        .choices
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let chosen = i == popup.selected && (!popup.numbered || popup.moved);
            let style = Style::default().fg(if chosen { theme.accent } else { theme.fg });
            let mark = match (popup.numbered, chosen) {
                (true, _) => format!(" {} ", i + 1),
                (false, true) => " \u{203a} ".to_string(),
                (false, false) => "   ".to_string(),
            };
            Line::from(vec![
                Span::styled(mark, style),
                Span::styled(
                    (*label).to_string(),
                    match chosen {
                        true => style.add_modifier(Modifier::BOLD),
                        false => style,
                    },
                ),
            ])
        })
        .collect()
}

/// at があればその直下、無ければ中央。はみ出す辺だけ画面の内側へ寄せる。
pub fn rect(popup: &Popup, area: Rect) -> Rect {
    let height = popup.choices.len() as u16 + 2;
    // 一番長い綴りに、選択の印と両側の枠を足す。
    let width = popup
        .choices
        .iter()
        .map(|(label, _)| label.len() as u16)
        .max()
        .unwrap_or(0)
        + 6;
    let (x, y) = popup.at.unwrap_or((
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
    ));
    Rect {
        x: x.min(area.right().saturating_sub(width)),
        y: y.min(area.bottom().saturating_sub(height)),
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 40,
    };

    use crate::command::CommandId;

    fn at(x: u16, y: u16) -> Popup {
        let choices = vec![
            ("Changes".into(), Effect::Command(CommandId::ShowDiffList)),
            (
                "Commit Log".into(),
                Effect::Command(CommandId::ShowCommitLog),
            ),
        ];
        Popup::new("Show", choices, Some((x, y)), 0)
    }

    #[test]
    fn 画面の外へはみ出さない() {
        let popup = at(95, 38);
        let rect = rect(&popup, SCREEN);
        assert!(rect.right() <= SCREEN.right(), "{rect:?}");
        assert!(rect.bottom() <= SCREEN.bottom(), "{rect:?}");
    }

    #[test]
    fn 行を押すとその行のコマンドを送る() {
        let mut popup = at(10, 10);
        let inner = crate::list::inner(rect(&popup, SCREEN));
        let effects = popup.click(inner.x, inner.y + 1, SCREEN);
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::PopModal, Effect::Command(CommandId::ShowCommitLog)]
            ),
            "{effects:?}"
        );
    }

    #[test]
    fn 枠の外を押したら閉じるだけ() {
        let mut popup = at(10, 10);
        assert!(
            matches!(popup.click(0, 0, SCREEN).as_slice(), [Effect::PopModal]),
            "枠の外"
        );
    }

    #[test]
    fn 数字の_popup_は矢印を押すまで選択行を強調しない() {
        use crossterm::event::KeyModifiers;
        let ws = crate::workspace::Workspace::for_test();
        let mut popup = Popup::numbered(
            "New",
            vec![
                ("Changes".into(), Effect::Command(CommandId::ShowDiffList)),
                (
                    "Commit Log".into(),
                    Effect::Command(CommandId::ShowCommitLog),
                ),
            ],
            None,
        );
        let label_fg = |popup: &Popup| {
            lines(popup, &ws.theme)
                .iter()
                .map(|line| line.spans[1].style.fg)
                .collect::<Vec<_>>()
        };
        assert_eq!(label_fg(&popup), [Some(ws.theme.fg), Some(ws.theme.fg)]);
        popup.update(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &ws.ctx());
        assert_eq!(label_fg(&popup), [Some(ws.theme.fg), Some(ws.theme.accent)]);
    }

    #[test]
    fn 数字の_popup_は数字か矢印の後の_enter_でだけ選び_ほかのキーでは何もせずに閉じる() {
        use crossterm::event::KeyModifiers;
        let ws = crate::workspace::Workspace::for_test();
        let numbered = || {
            Popup::numbered(
                "New",
                vec![
                    ("Changes".into(), Effect::Command(CommandId::ShowDiffList)),
                    (
                        "Commit Log".into(),
                        Effect::Command(CommandId::ShowCommitLog),
                    ),
                    ("Claude".into(), Effect::Command(CommandId::NewClaudeCode)),
                ],
                None,
            )
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let chosen = |codes: &[KeyCode]| {
            let mut popup = numbered();
            let (last, moves) = codes.split_last().unwrap();
            for code in moves {
                assert!(popup.update(key(*code), &ws.ctx()).is_empty(), "{code:?}");
            }
            match popup.update(key(*last), &ws.ctx()).as_slice() {
                [Effect::PopModal, Effect::Command(id)] => Some(*id),
                [Effect::PopModal] => None,
                other => panic!("{other:?}"),
            }
        };
        use KeyCode::{Char, Down, Enter, Up};
        assert_eq!(chosen(&[Char('2')]), Some(CommandId::ShowCommitLog));
        assert_eq!(chosen(&[Down, Down, Enter]), Some(CommandId::NewClaudeCode));
        assert_eq!(chosen(&[Up, Enter]), Some(CommandId::NewClaudeCode));
        assert_eq!(
            chosen(&[Down, Down, Down, Enter]),
            Some(CommandId::ShowDiffList)
        );
        assert_eq!(
            chosen(&[Enter]),
            None,
            "誤って一覧に打った文の改行で選ばない"
        );
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('o'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Esc,
            KeyCode::Char('4'),
            KeyCode::Char('0'),
        ] {
            assert!(
                matches!(
                    numbered().update(key(code), &ws.ctx()).as_slice(),
                    [Effect::PopModal]
                ),
                "{code:?}"
            );
        }
    }
}
