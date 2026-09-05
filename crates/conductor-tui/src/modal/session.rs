//! Claude セッションの再開。一覧はディスクを走るので svc が取ってくる。

use conductor_core::claude_sessions::ResumableSession;
use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::{Cursor, Filtered, filtered_key, scroll_for};
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
                let id = self
                    .matching()
                    .get(self.cursor.selected)
                    .map(|s| s.session_id.clone());
                return match id {
                    Some(id) => vec![
                        Effect::PopModal,
                        Effect::ResumeSession { id, worktree: None },
                    ],
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
    fn 選ぶと閉じてそのidで開き直す() {
        let (mut picker, ws) = picker();
        picker.update(key(KeyCode::Down), &ws.ctx());
        let effects = picker.update(key(KeyCode::Enter), &ws.ctx());
        let [Effect::PopModal, Effect::ResumeSession { id, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(id, "s2");
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
