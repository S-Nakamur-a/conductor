//! 既知リポジトリの切替。開く方はパス入力の [crate::modal::Prompt] が兼ねる。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::Cursor;
use crate::workspace::Ctx;

#[derive(Debug)]
pub struct RepoPicker {
    pub cursor: Cursor,
}

impl RepoPicker {
    pub fn open(current: usize) -> Self {
        Self {
            cursor: Cursor { selected: current },
        }
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let known = &ctx.repo.known;
        if self.cursor.navigate(ctx.keymap, key, known.len()) {
            return Vec::new();
        }
        match key.code {
            KeyCode::Enter => match known.get(self.cursor.selected) {
                Some(path) => vec![Effect::PopModal, Effect::SwitchRepo(path.clone())],
                None => vec![Effect::PopModal],
            },
            KeyCode::Esc => vec![Effect::PopModal],
            _ => Vec::new(),
        }
    }
}

/// パス入力で開くときの入口。`~` はホームに展開する。
pub fn expand_home(input: &str) -> PathBuf {
    let Some(rest) = input.strip_prefix('~') else {
        return PathBuf::from(input);
    };
    match dirs::home_dir() {
        Some(home) => home.join(rest.strip_prefix('/').unwrap_or(rest)),
        None => PathBuf::from(input),
    }
}

pub fn title() -> String {
    "Switch Repository".into()
}

pub fn lines(picker: &RepoPicker, ctx: &Ctx) -> Vec<Line<'static>> {
    ctx.repo
        .known
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let marker = if *path == ctx.repo.root { "* " } else { "  " };
            crate::list::row_line(
                vec![
                    Span::styled(marker, Style::default().fg(ctx.theme.accent)),
                    Span::styled(
                        path.display().to_string(),
                        Style::default().fg(ctx.theme.fg),
                    ),
                ],
                ctx.theme,
                i == picker.cursor.selected,
                true,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn チルダはホームに展開する() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/src"), home.join("src"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
    }
}
