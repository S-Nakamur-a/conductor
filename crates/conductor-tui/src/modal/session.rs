//! Claude セッションの再開。一覧はディスクを走るので svc が取ってくる。

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use conductor_core::claude_sessions::ResumableSession;
use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::Modal;
use crate::modal::picker::{Cursor, Filtered, filtered_key, scroll_for};
use crate::modal::popup::Popup;
use crate::task::Task;
use crate::workspace::Ctx;

#[derive(Debug, Default)]
pub struct ResumePicker {
    pub cursor: Cursor,
    pub filter: TextInput,
    /// 今のリポジトリだけか、全プロジェクトか。tab で入れ替わる。
    pub all_projects: bool,
    sessions: Vec<ResumableSession>,
    loading: bool,
}

impl ResumePicker {
    pub fn open() -> (Self, Effect) {
        let picker = Self {
            loading: true,
            ..Self::default()
        };
        (picker, Effect::Spawn(Task::ListSessions { all: false }))
    }

    pub fn install(&mut self, sessions: Vec<ResumableSession>) {
        self.sessions = sessions;
        self.loading = false;
        self.cursor.clamp(self.matching().len());
    }

    /// 絞り込みを通ったセッション。表示も選択もこの並びを見る。
    fn matching(&self) -> Vec<&ResumableSession> {
        let needle = self.filter.text().to_lowercase();
        self.sessions
            .iter()
            .filter(|s| {
                needle.is_empty()
                    || s.display.to_lowercase().contains(&needle)
                    || s.project_name.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let len = self.matching().len();
        match key.code {
            KeyCode::Esc => return vec![Effect::PopModal],
            KeyCode::Enter => {
                let chosen = self
                    .matching()
                    .get(self.cursor.selected)
                    .map(|s| launch_choice(s.session_id.clone(), s.config_dir.clone(), ctx.config));
                return match chosen {
                    Some(popup) => vec![Effect::PopModal, Effect::PushModal(Modal::Popup(popup))],
                    None => Vec::new(),
                };
            }
            KeyCode::Tab => {
                self.all_projects = !self.all_projects;
                self.loading = true;
                return vec![Effect::Spawn(Task::ListSessions {
                    all: self.all_projects,
                })];
            }
            _ => {}
        }
        if let Filtered::Typed =
            filtered_key(&mut self.cursor, &mut self.filter, ctx.keymap, key, len)
        {
            self.cursor.selected = 0;
        }
        Vec::new()
    }
}

/// 既定の行は会話を最後に走らせたアカウントで開き直す。
fn launch_choice(id: String, ran_in: PathBuf, config: &conductor_core::config::Config) -> Popup {
    let resume = |chrome, account: Option<&str>| Effect::ResumeSession {
        id: id.clone(),
        worktree: None,
        chrome,
        account: account.map(str::to_string),
        ran_in: Some(ran_in.clone()),
    };
    let mut choices = vec![
        ("Resume".into(), resume(false, None)),
        ("Resume + Chrome".into(), resume(true, None)),
    ];
    choices.extend(
        crate::panels::fleet::account_names(config)
            .map(|name| (format!("Resume as {name}"), resume(false, Some(name)))),
    );
    Popup::new("Resume", choices, None, 0)
}

pub fn title(picker: &ResumePicker) -> String {
    let scope = if picker.all_projects {
        "all projects"
    } else {
        "this repository"
    };
    format!("Resume Claude Session \u{b7} {scope} (tab: switch)")
}

pub fn lines(picker: &ResumePicker, ctx: &Ctx, area: Rect) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.accent)),
            Span::styled(
                crate::modal::input::with_caret(&picker.filter, area.width as usize).join(""),
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(""),
    ];
    let sessions = picker.matching();
    if sessions.is_empty() {
        lines.push(Line::styled(
            if picker.loading {
                "  loading\u{2026}"
            } else {
                "  no resumable sessions"
            },
            Style::default().fg(theme.muted),
        ));
        return lines;
    }
    let height = (area.height as usize).saturating_sub(4);
    let start = scroll_for(picker.cursor.selected, sessions.len(), height);
    for (i, session) in sessions.iter().enumerate().skip(start).take(height) {
        lines.push(crate::list::row_line(
            vec![
                Span::styled(
                    format!(" {:<10}", session.time_ago),
                    Style::default().fg(theme.hint),
                ),
                Span::styled(
                    format!("{:<16}", session.project_name),
                    Style::default().fg(theme.info),
                ),
                Span::styled(session.display.clone(), Style::default().fg(theme.fg)),
            ],
            theme,
            i == picker.cursor.selected,
            true,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn session(id: &str, display: &str, project: &str) -> ResumableSession {
        ResumableSession {
            session_id: id.into(),
            display: display.into(),
            project_name: project.into(),
            time_ago: "3h ago".into(),
            config_dir: PathBuf::from("/c"),
        }
    }

    fn picker() -> (ResumePicker, Workspace) {
        let (mut picker, _) = ResumePicker::open();
        picker.install(vec![
            session("s1", "fix the parser", "conductor"),
            session("s2", "write docs", "sheaf"),
        ]);
        (picker, Workspace::for_test())
    }

    #[test]
    fn 絞り込みは本文とプロジェクト名の両方に効く() {
        let (mut picker, ws) = picker();
        for c in "sheaf".chars() {
            picker.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        assert_eq!(
            picker
                .matching()
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            ["s2"]
        );
    }

    #[test]
    fn 選ぶとchromeで開き直すかを聞いてからそのidを走らせたアカウントで開き直す() {
        let (mut picker, ws) = picker();
        picker.update(key(KeyCode::Down), &ws.ctx());
        let mut effects = picker.update(key(KeyCode::Enter), &ws.ctx());
        let [Effect::PopModal, Effect::PushModal(Modal::Popup(popup))] = effects.as_mut_slice()
        else {
            panic!("{effects:?}");
        };
        popup.update(key(KeyCode::Down), &ws.ctx());
        let effects = popup.update(key(KeyCode::Enter), &ws.ctx());
        let [
            Effect::PopModal,
            Effect::ResumeSession {
                id,
                chrome: true,
                account: None,
                ran_in,
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(id, "s2");
        assert_eq!(
            ran_in.as_deref(),
            Some(Path::new("/c")),
            "会話を走らせたアカウントで開き直す"
        );
    }

    #[test]
    fn アカウントが2つ以上なら選んだアカウントで開き直す行がchromeの後に並ぶ() {
        let (mut picker, mut ws) = picker();
        ws.config.claude.accounts = ["work", "personal"]
            .map(|name| conductor_core::config::ClaudeAccount {
                name: name.into(),
                ..Default::default()
            })
            .to_vec();
        let mut effects = picker.update(key(KeyCode::Enter), &ws.ctx());
        let [Effect::PopModal, Effect::PushModal(Modal::Popup(popup))] = effects.as_mut_slice()
        else {
            panic!("{effects:?}");
        };
        for _ in 0..3 {
            popup.update(key(KeyCode::Down), &ws.ctx());
        }
        let effects = popup.update(key(KeyCode::Enter), &ws.ctx());
        let [
            Effect::PopModal,
            Effect::ResumeSession {
                id,
                chrome: false,
                account: Some(account),
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!((id.as_str(), account.as_str()), ("s1", "personal"));
    }

    #[test]
    fn tabで範囲を切り替えて読み直す() {
        let (mut picker, ws) = picker();
        let effects = picker.update(key(KeyCode::Tab), &ws.ctx());
        assert!(picker.all_projects);
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Spawn(Task::ListSessions { all: true })]
            ),
            "{effects:?}"
        );
    }

    #[test]
    fn 絞り込みで選択が範囲外に出ない() {
        let (mut picker, ws) = picker();
        picker.update(key(KeyCode::Down), &ws.ctx());
        for c in "sheaf".chars() {
            picker.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        assert_eq!(picker.cursor.selected, 0);
    }
}
