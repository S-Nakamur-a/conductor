//! 押した場所の直下に開く、コマンドを 1 つ選ぶポップアップ。
//!
//! 画面中央ではなく押した場所の隣に出すため、[rect] が矩形を決める。中央に置くと
//! 「どの区画の話か」が消えて、押した手応えが返らない。

use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::command::CommandId;
use crate::effect::Effect;
use crate::workspace::Ctx;

pub type Choices = &'static [(&'static str, CommandId)];

#[derive(Debug)]
pub struct Popup {
    title: &'static str,
    choices: Choices,
    /// 枠の左上。
    at: (u16, u16),
    selected: usize,
}

impl Popup {
    /// current は開いた時点で合わせておく行。
    pub fn new(title: &'static str, choices: Choices, at: (u16, u16), current: usize) -> Self {
        Self {
            title,
            choices,
            at,
            selected: current.min(choices.len() - 1),
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
    pub fn click(&self, x: u16, y: u16, area: Rect) -> Vec<Effect> {
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

    fn choose(&self, row: usize) -> Vec<Effect> {
        match self.choices.get(row) {
            Some((_, command)) => vec![Effect::PopModal, Effect::Command(*command)],
            None => vec![Effect::PopModal],
        }
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

/// 押した見出しの直下。はみ出す辺だけ画面の内側へ寄せる。
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
    let (x, y) = popup.at;
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

    const CHOICES: Choices = &[
        ("Changes", CommandId::ShowDiffList),
        ("Commit Log", CommandId::ShowCommitLog),
    ];

    fn at(x: u16, y: u16) -> Popup {
        Popup::new("Show", CHOICES, (x, y), 0)
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
        let popup = at(10, 10);
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
        let popup = at(10, 10);
        assert!(
            matches!(popup.click(0, 0, SCREEN).as_slice(), [Effect::PopModal]),
            "枠の外"
        );
    }
}
