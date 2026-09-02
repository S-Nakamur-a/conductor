//! GitHub へコメントを投稿する前の確認。取り消せないので、何が飛ぶかを並べる。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::task::{Publishable, Task};
use crate::workspace::{Ctx, StatusLevel};

#[derive(Debug)]
pub struct Publish {
    request: Box<Publishable>,
}

impl Publish {
    pub fn new(request: Box<Publishable>) -> Self {
        Self { request }
    }

    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => vec![
                Effect::PopModal,
                Effect::Status(
                    StatusLevel::Info,
                    format!(
                        "Publishing {} comment(s) to GitHub\u{2026}",
                        self.request.comments.len()
                    ),
                ),
                Effect::Spawn(Task::Publish(self.request.clone())),
            ],
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => vec![
                Effect::PopModal,
                Effect::Status(StatusLevel::Warning, "Publish cancelled.".into()),
            ],
            _ => Vec::new(),
        }
    }
}

pub fn title(modal: &Publish) -> String {
    let request = &modal.request;
    format!(
        "Publish {} comment(s) to {}/{} #{}",
        request.comments.len(),
        request.owner,
        request.repo,
        request.pr_number
    )
}

pub fn lines(modal: &Publish, ctx: &Ctx) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let request = &modal.request;
    let mut lines: Vec<Line<'static>> = request
        .comments
        .iter()
        .map(|c| {
            Line::from(vec![
                Span::styled(
                    format!(" {}:{}  ", c.file_path, c.line_start),
                    Style::default().fg(theme.accent),
                ),
                Span::styled(
                    c.body.lines().next().unwrap_or("").to_string(),
                    Style::default().fg(theme.fg),
                ),
            ])
        })
        .collect();
    if request.skipped > 0 {
        lines.push(Line::styled(
            format!(
                " {} comment(s) outside the current diff will be skipped.",
                request.skipped
            ),
            Style::default().fg(theme.warning),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        " y: publish  \u{b7}  n: cancel",
        Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use conductor_core::review_publish::PublishComment;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn modal() -> (Publish, Workspace) {
        let request = Publishable {
            owner: "acme".into(),
            repo: "widget".into(),
            pr_number: 7,
            comments: vec![PublishComment {
                id: "c1".into(),
                file_path: "src/a.rs".into(),
                line_start: 3,
                line_end: None,
                body: "off by one".into(),
            }],
            skipped: 2,
        };
        (Publish::new(Box::new(request)), Workspace::for_test())
    }

    #[test]
    fn 何が飛ぶかと落とす件数を出す() {
        let (modal, ws) = modal();
        let text: String = lines(&modal, &ws.ctx())
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(text.contains("src/a.rs:3"), "{text}");
        assert!(text.contains("2 comment(s) outside"), "{text}");
        assert!(
            title(&modal).contains("acme/widget #7"),
            "{}",
            title(&modal)
        );
    }

    #[test]
    fn nは何も飛ばさず閉じる() {
        let (mut modal, ws) = modal();
        for code in [KeyCode::Char('n'), KeyCode::Esc] {
            let effects = modal.update(key(code), &ws.ctx());
            assert!(
                matches!(
                    effects.as_slice(),
                    [Effect::PopModal, Effect::Status(StatusLevel::Warning, _)]
                ),
                "{code:?}: {effects:?}"
            );
        }
    }

    #[test]
    fn yで投稿のタスクが飛ぶ() {
        let (mut modal, ws) = modal();
        let effects = modal.update(key(KeyCode::Char('y')), &ws.ctx());
        let [
            Effect::PopModal,
            Effect::Status(..),
            Effect::Spawn(Task::Publish(request)),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(request.comments.len(), 1);
    }

    #[test]
    fn 関係ないキーでは閉じない() {
        let (mut modal, ws) = modal();
        assert!(modal.update(key(KeyCode::Char('j')), &ws.ctx()).is_empty());
    }
}
