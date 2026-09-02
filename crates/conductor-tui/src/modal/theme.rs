//! テーマピッカー。移動のたびにライブプレビューし、Esc で開いた時点へ戻す。

use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::Cursor;
use crate::workspace::{Ctx, StatusLevel};

#[derive(Debug)]
pub struct ThemePicker {
    pub cursor: Cursor,
    /// 開いた時点のテーマ名。Esc の戻り先。
    original: String,
}

impl ThemePicker {
    pub fn open(current: &str) -> Self {
        let selected = names().iter().position(|n| *n == current).unwrap_or(0);
        Self {
            cursor: Cursor { selected },
            original: current.to_string(),
        }
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let names = names();
        if self.cursor.navigate(ctx.keymap, key, names.len()) {
            return vec![preview(names[self.cursor.selected])];
        }
        match key.code {
            KeyCode::Enter => {
                let name = names[self.cursor.selected];
                vec![
                    Effect::PopModal,
                    Effect::SetTheme {
                        name: name.to_string(),
                        persist: true,
                    },
                    Effect::Status(StatusLevel::Success, format!("Theme: {name}")),
                ]
            }
            KeyCode::Esc => vec![Effect::PopModal, preview(&self.original)],
            _ => Vec::new(),
        }
    }
}

fn preview(name: &str) -> Effect {
    Effect::SetTheme {
        name: name.to_string(),
        persist: false,
    }
}

fn names() -> &'static [&'static str] {
    Theme::all_names()
}

pub fn title() -> String {
    "Switch Theme  \u{2191}/\u{2193} preview  enter confirm  esc revert".into()
}

pub fn lines(picker: &ThemePicker, ctx: &Ctx) -> Vec<Line<'static>> {
    names()
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let tag = if Theme::from_name(name).light {
                "light"
            } else {
                "dark"
            };
            crate::list::row_line(
                vec![
                    Span::styled(format!(" {name} "), Style::default().fg(ctx.theme.fg)),
                    Span::styled(tag, Style::default().fg(ctx.theme.hint)),
                ],
                ctx.theme,
                i == picker.cursor.selected,
                true,
            )
        })
        .collect()
}
