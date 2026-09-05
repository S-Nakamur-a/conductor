//! PR 番号か URL を受け取って、レビューできる worktree にする入口。

use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::Line;

use crate::effect::Effect;
use crate::task::Task;
use crate::workspace::Ctx;

#[derive(Debug, Default)]
pub struct PrInput {
    pub input: TextInput,
    loading: bool,
    /// 直前の取り込みが失敗した理由。
    error: Option<String>,
}

impl PrInput {
    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        // 走っている間は入力を凍らせる。打った文字が、いま取り込んでいるものと
        // 食い違う入力欄を作ってしまう。
        if self.loading {
            return match key.code {
                KeyCode::Esc => vec![Effect::PopModal],
                _ => Vec::new(),
            };
        }
        match key.code {
            KeyCode::Esc => vec![Effect::PopModal],
            KeyCode::Enter => {
                let input = self.input.text().trim().to_string();
                if input.is_empty() {
                    return Vec::new();
                }
                self.loading = true;
                self.error = None;
                vec![Effect::Spawn(Task::IntakePr { input })]
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                self.error = None;
                self.input.delete_to_line_start();
                Vec::new()
            }
            _ => {
                self.error = None;
                self.input.handle_key(key);
                Vec::new()
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.loading {
            return;
        }
        self.error = None;
        self.input.insert_str(text);
    }

    pub fn failed(&mut self, error: String) {
        self.loading = false;
        self.error = Some(error);
    }
}

pub fn title() -> String {
    "Review Pull Request (number or URL)".into()
}

pub fn lines(prompt: &PrInput, ctx: &Ctx, width: u16) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let mut lines: Vec<Line<'static>> =
        crate::modal::input::with_caret(&prompt.input, width as usize)
            .into_iter()
            .map(|line| Line::styled(format!("> {line}"), Style::default().fg(theme.fg)))
            .collect();
    if prompt.loading {
        lines.push(Line::styled(
            "fetching\u{2026}",
            Style::default().fg(theme.info),
        ));
    }
    if let Some(error) = &prompt.error {
        lines.push(Line::styled(
            error.clone(),
            Style::default().fg(theme.error),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(text: &str) -> (PrInput, Workspace) {
        let ws = Workspace::for_test();
        let mut prompt = PrInput::default();
        for c in text.chars() {
            prompt.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        (prompt, ws)
    }

    #[test]
    fn 空の入力では何も始めない() {
        let (mut prompt, ws) = typed("  ");
        assert!(prompt.update(key(KeyCode::Enter), &ws.ctx()).is_empty());
        assert!(!prompt.loading);
    }

    #[test]
    fn 失敗しても入力は残り編集で理由が消える() {
        let (mut prompt, ws) = typed("123");
        let effects = prompt.update(key(KeyCode::Enter), &ws.ctx());
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Spawn(Task::IntakePr { input })] if input == "123"
            ),
            "{effects:?}"
        );

        prompt.update(key(KeyCode::Char('4')), &ws.ctx());
        assert_eq!(prompt.input.text(), "123");

        prompt.failed("Pull request #123 not found.".into());
        assert_eq!(prompt.input.text(), "123", "打ち直させない");
        assert!(prompt.error.is_some());

        prompt.update(key(KeyCode::Backspace), &ws.ctx());
        assert_eq!(prompt.input.text(), "12");
        assert!(prompt.error.is_none(), "直した入力の隣に古い理由が残る");
    }

    #[test]
    fn 走っている間もescで閉じられる() {
        let (mut prompt, ws) = typed("123");
        prompt.update(key(KeyCode::Enter), &ws.ctx());
        assert_eq!(
            prompt.update(key(KeyCode::Esc), &ws.ctx()),
            vec![Effect::PopModal]
        );
    }
}
