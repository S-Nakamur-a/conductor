//! 下区画に何を出すかを選ぶポップアップ。押した見出しの直下に開く。
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

const CHOICES: [(&str, CommandId); 3] = [
    ("Changes", CommandId::ShowDiffList),
    ("Commit Log", CommandId::ShowCommitLog),
    ("Comments", CommandId::ShowCommentList),
];

/// 枠を含む幅。一番長い綴りと選択の印が収まる。
const WIDTH: u16 = 16;

#[derive(Debug)]
pub struct ListingPicker {
    /// 枠の左上。
    at: (u16, u16),
    selected: usize,
}

impl ListingPicker {
    /// current はいま出ている一覧の添字。開いた時点でそこに合わせる。
    pub fn new(x: u16, y: u16, current: usize) -> Self {
        Self {
            at: (x, y),
            selected: current.min(CHOICES.len() - 1),
        }
    }

    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        let last = CHOICES.len() - 1;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1) % CHOICES.len();
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
            Some(row) if row < CHOICES.len() => self.choose(row),
            _ => vec![Effect::PopModal],
        }
    }

    fn choose(&self, row: usize) -> Vec<Effect> {
        match CHOICES.get(row) {
            Some((_, command)) => vec![Effect::PopModal, Effect::Command(*command)],
            None => vec![Effect::PopModal],
        }
    }
}

pub fn title() -> String {
    "Show".to_string()
}

pub fn lines(picker: &ListingPicker, theme: &Theme) -> Vec<Line<'static>> {
    CHOICES
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let chosen = i == picker.selected;
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
pub fn rect(picker: &ListingPicker, area: Rect) -> Rect {
    let height = CHOICES.len() as u16 + 2;
    let (x, y) = picker.at;
    Rect {
        x: x.min(area.right().saturating_sub(WIDTH)),
        y: y.min(area.bottom().saturating_sub(height)),
        width: WIDTH.min(area.width),
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

    #[test]
    fn 画面の外へはみ出さない() {
        let picker = ListingPicker::new(95, 38, 0);
        let rect = rect(&picker, SCREEN);
        assert!(rect.right() <= SCREEN.right(), "{rect:?}");
        assert!(rect.bottom() <= SCREEN.bottom(), "{rect:?}");
    }

    #[test]
    fn 行を押すとその行のコマンドを送る() {
        let picker = ListingPicker::new(10, 10, 0);
        let inner = crate::list::inner(rect(&picker, SCREEN));
        let effects = picker.click(inner.x, inner.y + 1, SCREEN);
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
        let picker = ListingPicker::new(10, 10, 0);
        assert!(
            matches!(picker.click(0, 0, SCREEN).as_slice(), [Effect::PopModal]),
            "枠の外"
        );
    }
}
