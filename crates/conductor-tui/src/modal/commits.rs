//! cherry-pick: 他のブランチのコミットを 1 つ選んで、いまの worktree へ積む。

use std::path::PathBuf;

use conductor_core::git_engine::CommitInfo;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::{Cursor, scroll_for};
use crate::task::Task;
use crate::workspace::Ctx;

#[derive(Debug)]
pub struct CherryPick {
    pub cursor: Cursor,
    /// 取り出し元。tab で候補を順に回る。
    source: String,
    others: Vec<String>,
    target: PathBuf,
    commits: Vec<CommitInfo>,
    loading: bool,
}

impl CherryPick {
    pub fn open(others: Vec<String>, target: PathBuf) -> Option<(Self, Effect)> {
        let source = others.first()?.clone();
        let load = Effect::Spawn(Task::ListBranchCommits {
            branch: source.clone(),
        });
        Some((
            Self {
                cursor: Cursor::default(),
                source,
                others,
                target,
                commits: Vec::new(),
                loading: true,
            },
            load,
        ))
    }

    pub fn install(&mut self, commits: Vec<CommitInfo>) {
        self.commits = commits;
        self.loading = false;
        self.cursor.selected = 0;
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        if self.cursor.navigate(ctx.keymap, key, self.commits.len()) {
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => vec![Effect::PopModal],
            KeyCode::Tab => self.next_source(),
            KeyCode::Enter => match self.commits.get(self.cursor.selected) {
                Some(commit) => vec![
                    Effect::PopModal,
                    Effect::Spawn(Task::CherryPick {
                        worktree: self.target.clone(),
                        commit: commit.oid.clone(),
                    }),
                ],
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn next_source(&mut self) -> Vec<Effect> {
        let at = self
            .others
            .iter()
            .position(|b| *b == self.source)
            .unwrap_or(0);
        self.source = self.others[(at + 1) % self.others.len()].clone();
        self.commits.clear();
        self.loading = true;
        vec![Effect::Spawn(Task::ListBranchCommits {
            branch: self.source.clone(),
        })]
    }
}

pub fn title(picker: &CherryPick) -> String {
    format!("Cherry-pick from {} (tab: switch)", picker.source)
}

pub fn lines(picker: &CherryPick, ctx: &Ctx, area: Rect) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    if picker.commits.is_empty() {
        return vec![Line::styled(
            if picker.loading {
                "  loading\u{2026}"
            } else {
                "  no commits on this branch"
            },
            Style::default().fg(theme.muted),
        )];
    }
    let height = (area.height as usize).saturating_sub(2);
    let start = scroll_for(picker.cursor.selected, picker.commits.len(), height);
    picker
        .commits
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, commit)| {
            crate::list::row_line(
                vec![
                    Span::styled(
                        format!(" {} ", commit.short_oid),
                        Style::default().fg(theme.accent),
                    ),
                    Span::styled(
                        format!("{:<10}", commit.time_ago),
                        Style::default().fg(theme.hint),
                    ),
                    Span::styled(commit.message.clone(), Style::default().fg(theme.fg)),
                ],
                theme,
                i == picker.cursor.selected,
                true,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn commit(oid: &str) -> CommitInfo {
        CommitInfo {
            short_oid: oid[..8.min(oid.len())].to_string(),
            oid: oid.to_string(),
            message: format!("message of {oid}"),
            author: "someone".into(),
            time_ago: "2h ago".into(),
        }
    }

    fn picker() -> (CherryPick, Workspace) {
        let (mut picker, _) = CherryPick::open(
            vec!["feature/a".into(), "feature/b".into()],
            PathBuf::from("/tmp/wt/here"),
        )
        .unwrap();
        picker.install(vec![commit("aaaaaaaaaa"), commit("bbbbbbbbbb")]);
        (picker, Workspace::for_test())
    }

    #[test]
    fn 候補が無ければ開かない() {
        assert!(CherryPick::open(Vec::new(), PathBuf::from("/tmp/wt")).is_none());
    }

    #[test]
    fn 選んだコミットを今のworktreeへ積む() {
        let (mut picker, ws) = picker();
        picker.update(key(KeyCode::Down), &ws.ctx());
        let effects = picker.update(key(KeyCode::Enter), &ws.ctx());
        let [
            Effect::PopModal,
            Effect::Spawn(Task::CherryPick { worktree, commit }),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(commit, "bbbbbbbbbb");
        assert_eq!(worktree, &PathBuf::from("/tmp/wt/here"));
    }

    #[test]
    fn tabは取り出し元を回して読み直す() {
        let (mut picker, ws) = picker();
        let effects = picker.update(key(KeyCode::Tab), &ws.ctx());
        assert!(title(&picker).contains("feature/b"), "{}", title(&picker));
        assert!(
            picker.commits.is_empty(),
            "前のブランチのコミットが残っている"
        );
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Spawn(Task::ListBranchCommits { branch })] if branch == "feature/b"
            ),
            "{effects:?}"
        );

        picker.update(key(KeyCode::Tab), &ws.ctx());
        assert!(title(&picker).contains("feature/a"), "一周して戻る");
    }

    #[test]
    fn コミットが無ければ何も積まない() {
        let (mut picker, ws) = picker();
        picker.install(Vec::new());
        assert!(picker.update(key(KeyCode::Enter), &ws.ctx()).is_empty());
    }
}
