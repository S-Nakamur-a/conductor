//! 自己更新の確認と進み具合。走っているバイナリを置き換えるので、先に一度訊く。

use conductor_core::update_checker::UpdateInfo;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::effect::Effect;
use crate::task::{Task, UpdateStage};
use crate::workspace::Ctx;

#[derive(Debug)]
pub enum Update {
    Confirm(Box<UpdateInfo>),
    Running(&'static str),
    Failed(String),
}

impl Update {
    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        match self {
            Self::Confirm(info) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let task = Task::DownloadUpdate(info.clone());
                    *self = Self::Running("Preparing\u{2026}");
                    vec![Effect::Spawn(task)]
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => vec![Effect::PopModal],
                _ => Vec::new(),
            },
            // 走っている差し替えは止められないので、閉じるのは表示だけ。
            Self::Running(_) => match key.code {
                KeyCode::Esc => vec![Effect::PopModal],
                _ => Vec::new(),
            },
            Self::Failed(_) => vec![Effect::PopModal],
        }
    }

    /// ワーカーからの報告を映す。入れ替えが済んだら再起動へ進む。
    pub fn accept(&mut self, stage: UpdateStage) -> Option<Vec<Effect>> {
        match stage {
            UpdateStage::Step(step) => {
                *self = Self::Running(step.message());
                None
            }
            UpdateStage::Installed => Some(vec![Effect::PopModal, Effect::Quit]),
            UpdateStage::Failed(reason) => {
                *self = Self::Failed(reason);
                None
            }
        }
    }
}

pub fn title(modal: &Update) -> String {
    match modal {
        Update::Confirm(info) => format!("Update to v{}", info.latest_version),
        Update::Running(_) => "Updating Conductor".into(),
        Update::Failed(_) => "Update failed".into(),
    }
}

pub fn lines(modal: &Update, ctx: &Ctx) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let hint = Style::default().fg(theme.hint).add_modifier(Modifier::BOLD);
    match modal {
        Update::Confirm(info) => vec![
            Line::styled(
                format!(
                    " Download v{} and restart? (running v{})",
                    info.latest_version, ctx.version
                ),
                Style::default().fg(theme.fg),
            ),
            Line::from(""),
            Line::styled(" y: update  \u{b7}  n: cancel", hint),
        ],
        Update::Running(message) => vec![
            Line::styled(format!(" {message}"), Style::default().fg(theme.fg)),
            Line::from(""),
            Line::styled(" esc: hide", hint),
        ],
        Update::Failed(reason) => vec![
            Line::styled(format!(" {reason}"), Style::default().fg(theme.error)),
            Line::from(""),
            Line::styled(" any key: dismiss", hint),
        ],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use conductor_core::update_checker::Progress;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(crate) fn info() -> UpdateInfo {
        UpdateInfo {
            latest_version: "9.9.9".into(),
            assets: Vec::new(),
        }
    }

    #[test]
    fn yでダウンロードが始まりnは何もしない() {
        let ws = Workspace::for_test();
        let mut modal = Update::Confirm(Box::new(info()));
        assert!(matches!(
            modal.update(key(KeyCode::Char('n')), &ws.ctx()).as_slice(),
            [Effect::PopModal]
        ));

        let effects = modal.update(key(KeyCode::Char('y')), &ws.ctx());
        assert!(
            matches!(effects.as_slice(), [Effect::Spawn(Task::DownloadUpdate(_))]),
            "{effects:?}"
        );
        assert!(matches!(modal, Update::Running(_)), "{modal:?}");
    }

    #[test]
    fn 確認の文面に新旧のバージョンが出る() {
        let ws = Workspace::for_test();
        let modal = Update::Confirm(Box::new(info()));
        let text: String = lines(&modal, &ws.ctx())
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(text.contains("v9.9.9"), "{text}");
        assert!(text.contains(ws.version), "{text}");
        assert!(title(&modal).contains("v9.9.9"));
    }

    #[test]
    fn 途中経過は文面を差し替え完了で再起動へ進む() {
        let mut modal = Update::Confirm(Box::new(info()));
        assert!(
            modal
                .accept(UpdateStage::Step(Progress::Extracting))
                .is_none()
        );
        let Update::Running(message) = &modal else {
            panic!("{modal:?}");
        };
        assert_eq!(*message, Progress::Extracting.message());

        let effects = modal.accept(UpdateStage::Installed).unwrap();
        assert!(
            matches!(effects.as_slice(), [Effect::PopModal, Effect::Quit]),
            "{effects:?}"
        );
    }

    #[test]
    fn 失敗は理由を出してどのキーでも閉じる() {
        let ws = Workspace::for_test();
        let mut modal = Update::Confirm(Box::new(info()));
        assert!(
            modal
                .accept(UpdateStage::Failed("no route to host".into()))
                .is_none()
        );
        let text: String = lines(&modal, &ws.ctx())
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(text.contains("no route to host"), "{text}");
        assert!(matches!(
            modal.update(key(KeyCode::Char('j')), &ws.ctx()).as_slice(),
            [Effect::PopModal]
        ));
    }
}
