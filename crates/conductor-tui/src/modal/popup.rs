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

pub type Choices = Vec<(&'static str, Effect)>;

#[derive(Debug)]
pub struct Popup {
    title: &'static str,
    choices: Choices,
    /// 枠の左上。None は画面の中央。
    at: Option<(u16, u16)>,
    selected: usize,
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
        }
    }

    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
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
            let chosen = i == popup.selected;
            let style = Style::default().fg(if chosen { theme.accent } else { theme.fg });
            Line::from(vec![
                Span::styled(if chosen { " \u{203a} " } else { "   " }, style),
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
            ("Changes", Effect::Command(CommandId::ShowDiffList)),
            ("Commit Log", Effect::Command(CommandId::ShowCommitLog)),
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
}
