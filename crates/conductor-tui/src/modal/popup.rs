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
    /// 数字でだけ答え、ほかのキーでは何もせずに閉じる。
    numbered: bool,
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
        }
    }

    pub fn numbered(title: &'static str, choices: Choices, at: Option<(u16, u16)>) -> Self {
        Self {
            numbered: true,
            ..Self::new(title, choices, at, 0)
        }
    }

    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        if self.numbered {
            return match key.code {
                KeyCode::Char(c @ '1'..='9') => self.choose(usize::from(c as u8 - b'1')),
                _ => vec![Effect::PopModal],
            };
        }
        let last = self.choices.len() - 1;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1) % self.choices.len();
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.checked_sub(1).unwrap_or(last);
                Vec::new()
            }
            KeyCode::Enter => self.choose(self.selected),
            KeyCode::Esc => vec![Effect::PopModal],
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
            let chosen = i == popup.selected && !popup.numbered;
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
    fn 数字の_popup_は数字でだけ選び_ほかのキーでは何もせずに閉じる() {
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
                ],
                None,
            )
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(matches!(
            numbered()
                .update(key(KeyCode::Char('2')), &ws.ctx())
                .as_slice(),
            [Effect::PopModal, Effect::Command(CommandId::ShowCommitLog)]
        ));
        for code in [
            KeyCode::Enter,
            KeyCode::Char('n'),
            KeyCode::Char('o'),
            KeyCode::Char('j'),
            KeyCode::Down,
            KeyCode::Esc,
            KeyCode::Char('3'),
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
