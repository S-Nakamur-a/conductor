//! ブランチを 1 つ選ぶ一覧。switch-branch と grab が形を共有する。

use std::collections::HashMap;
use std::path::PathBuf;

use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::{Cursor, Filtered, filtered_key, scroll_for};
use crate::task::Task;
use crate::workspace::Ctx;

/// 選んだブランチで何が起きるか。
#[derive(Debug)]
pub enum Pick {
    /// リモートブランチから worktree を作る。
    Checkout,
    /// ブランチを main worktree へ持ってくる。値はブランチごとの元の worktree。
    Grab(HashMap<String, PathBuf>),
}

#[derive(Debug)]
pub struct BranchPicker {
    pub cursor: Cursor,
    pub filter: TextInput,
    title: &'static str,
    pick: Pick,
    branches: Vec<String>,
    loading: bool,
}

impl BranchPicker {
    /// リモートブランチから選ぶ。手元の ref をすぐ出しつつ、fetch した結果で
    /// 上書きする。fetch を待たせると数秒間まっさらな一覧を見ることになる。
    pub fn remote() -> (Self, Vec<Effect>) {
        let picker = Self {
            cursor: Cursor::default(),
            filter: TextInput::new(),
            title: "Switch Branch (Remote)",
            pick: Pick::Checkout,
            branches: Vec::new(),
            loading: true,
        };
        let effects = [false, true]
            .into_iter()
            .map(|fetch| Effect::Spawn(Task::ListRemoteBranches { fetch }))
            .collect();
        (picker, effects)
    }

    /// main 以外の worktree のブランチから選ぶ。一覧は手元にあるので待ちがない。
    pub fn grab(sources: HashMap<String, PathBuf>) -> Self {
        let mut branches: Vec<String> = sources.keys().cloned().collect();
        branches.sort();
        Self {
            cursor: Cursor::default(),
            filter: TextInput::new(),
            title: "Grab Branch",
            pick: Pick::Grab(sources),
            branches,
            loading: false,
        }
    }

    /// 一覧を差し替える。選んでいたブランチは名前で追いかける。
    pub fn install(&mut self, branches: Vec<String>) {
        let selected = self.matching().get(self.cursor.selected).cloned();
        self.branches = branches;
        self.loading = false;
        self.cursor.selected = selected
            .and_then(|name| self.matching().iter().position(|b| *b == name))
            .unwrap_or(self.cursor.selected);
        self.cursor.clamp(self.matching().len());
    }

    fn matching(&self) -> Vec<String> {
        let needle = self.filter.text().to_lowercase();
        self.branches
            .iter()
            .filter(|b| b.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let matching = self.matching();
        match key.code {
            KeyCode::Esc => return vec![Effect::PopModal],
            KeyCode::Enter => {
                let Some(branch) = matching.get(self.cursor.selected).cloned() else {
                    return Vec::new();
                };
                return self.chosen(branch);
            }
            _ => {}
        }
        if let Filtered::Typed = filtered_key(
            &mut self.cursor,
            &mut self.filter,
            ctx.keymap,
            key,
            matching.len(),
        ) {
            self.cursor.selected = 0;
        }
        Vec::new()
    }

    fn chosen(&self, branch: String) -> Vec<Effect> {
        let task = match &self.pick {
            Pick::Checkout => Task::CreateWorktreeFromRemote {
                remote_branch: branch,
            },
            Pick::Grab(sources) => match sources.get(&branch) {
                Some(source) => Task::Grab {
                    source: source.clone(),
                    branch,
                },
                None => return vec![Effect::PopModal],
            },
        };
        vec![Effect::PopModal, Effect::Spawn(task)]
    }
}

pub fn title(picker: &BranchPicker) -> String {
    picker.title.to_string()
}

pub fn lines(picker: &BranchPicker, ctx: &Ctx, area: Rect) -> Vec<Line<'static>> {
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
    let matching = picker.matching();
    if matching.is_empty() {
        lines.push(Line::styled(
            if picker.loading {
                "  loading\u{2026}"
            } else {
                "  no branches"
            },
            Style::default().fg(theme.muted),
        ));
        return lines;
    }
    let height = (area.height as usize).saturating_sub(4);
    let start = scroll_for(picker.cursor.selected, matching.len(), height);
    for (i, branch) in matching.iter().enumerate().skip(start).take(height) {
        lines.push(crate::list::row_line(
            vec![Span::styled(
                format!(" {branch}"),
                Style::default().fg(theme.fg),
            )],
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

    fn type_text(picker: &mut BranchPicker, ws: &Workspace, text: &str) {
        for c in text.chars() {
            picker.update(key(KeyCode::Char(c)), &ws.ctx());
        }
    }

    #[test]
    fn リモートは手元の一覧とfetchの両方を頼む() {
        let (picker, effects) = BranchPicker::remote();
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::Spawn(Task::ListRemoteBranches { fetch: false }),
                    Effect::Spawn(Task::ListRemoteBranches { fetch: true }),
                ]
            ),
            "{effects:?}"
        );
        assert!(picker.loading);
    }

    #[test]
    fn 選んだリモートブランチから_worktreeを作る() {
        let ws = Workspace::for_test();
        let (mut picker, _) = BranchPicker::remote();
        picker.install(vec!["origin/main".into(), "origin/feature/a".into()]);
        type_text(&mut picker, &ws, "feat");
        let effects = picker.update(key(KeyCode::Enter), &ws.ctx());
        let [
            Effect::PopModal,
            Effect::Spawn(Task::CreateWorktreeFromRemote { remote_branch }),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(remote_branch, "origin/feature/a");
    }

    #[test]
    fn 一覧が入れ替わっても選んでいたブランチを追いかける() {
        let (mut picker, _) = BranchPicker::remote();
        picker.install(vec!["a".into(), "b".into()]);
        picker.cursor.selected = 1;
        picker.install(vec!["z".into(), "a".into(), "b".into()]);
        assert_eq!(picker.matching()[picker.cursor.selected], "b");
    }

    #[test]
    fn grabは選んだブランチの元のworktreeを渡す() {
        let ws = Workspace::for_test();
        let sources = HashMap::from([
            ("feature/a".to_string(), PathBuf::from("/tmp/wt/a")),
            ("feature/b".to_string(), PathBuf::from("/tmp/wt/b")),
        ]);
        let mut picker = BranchPicker::grab(sources);
        picker.cursor.selected = 1;
        let effects = picker.update(key(KeyCode::Enter), &ws.ctx());
        let [
            Effect::PopModal,
            Effect::Spawn(Task::Grab { branch, source }),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(
            (branch.as_str(), source.as_path()),
            ("feature/b", Path::new("/tmp/wt/b"))
        );
    }

    use std::path::Path;

    #[test]
    fn escは何もせず閉じる() {
        let ws = Workspace::for_test();
        let mut picker = BranchPicker::grab(HashMap::new());
        assert_eq!(
            picker.update(key(KeyCode::Esc), &ws.ctx()),
            vec![Effect::PopModal]
        );
    }
}
