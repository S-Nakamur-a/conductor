//! パネルの Claude が MCP 越しに頼んだ画面操作を Effect に直す。
//!
//! 開く・作る頼みは、頼んだ Claude の worktree を画面に出してから操作する。人が別の
//! worktree を見ていても、頼まれたものが見えないまま終わるよりは移るほうを選んだ。

use std::path::Path;

use conductor_core::cc_hook::UiRequest;

use crate::effect::Effect;
use crate::workspace::{StatusLevel, Workspace};

pub fn effects(ws: &mut Workspace, cwd: &Path, request: UiRequest) -> Vec<Effect> {
    let Some((index, root)) = owning_worktree(ws, cwd) else {
        return vec![Effect::Status(
            StatusLevel::Warning,
            format!(
                "Claude asked for {}, which this window does not show.",
                cwd.display()
            ),
        )];
    };
    let shown = ws.worktree_path() == root;
    match request {
        UiRequest::OpenFile { path, line, diff } => {
            let mut effects = match show(ws, index, &root, shown) {
                Ok(effects) => effects,
                Err(refusal) => return refusal,
            };
            effects.push(match (diff, relative(&root, &path)) {
                (true, Some(rel)) => Effect::OpenChangedFile { path: rel, line },
                _ => Effect::OpenFile {
                    path: path.into(),
                    line,
                    diff: None,
                    preview: false,
                },
            });
            effects
        }
        // 閉じるために画面を移ると、閉じた結果ではなく移った先が見える。
        UiRequest::CloseFile { path } => shown
            .then(|| relative(&root, &path))
            .flatten()
            .and_then(|rel| ws.panels.viewer.close_path(&rel))
            .unwrap_or_else(|| {
                vec![Effect::Status(
                    StatusLevel::Warning,
                    format!("Claude asked to close {path}, which is not open here."),
                )]
            }),
        // 移った後の worktree で確認の文言を組むので、移るのはここで済ませておく。
        UiRequest::AnalyzeReview => {
            let mut effects = match show(ws, index, &root, shown) {
                Ok(effects) => effects,
                Err(refusal) => return refusal,
            };
            effects.extend(crate::command::confirm_requested_analyze(ws));
            effects
        }
    }
}

/// worktree を移るとエディタは閉じられる。保存していない編集を Claude の都合で
/// 失わせないよう、開いている間は移らずに断る。
fn show(
    ws: &mut Workspace,
    index: usize,
    root: &Path,
    shown: bool,
) -> Result<Vec<Effect>, Vec<Effect>> {
    if shown {
        return Ok(Vec::new());
    }
    if let Some(editor) = ws.panels.terminal.editor_name() {
        return Err(vec![Effect::Status(
            StatusLevel::Warning,
            format!(
                "Claude asked to show {}; close {editor} first.",
                root.display()
            ),
        )]);
    }
    Ok(crate::effect::select_worktree(ws, index))
}

/// worktree はネストしうるので、cwd を含むうち最も深いもの。
fn owning_worktree(ws: &Workspace, cwd: &Path) -> Option<(usize, std::path::PathBuf)> {
    ws.panels
        .worktree
        .list()
        .iter()
        .enumerate()
        .filter(|(_, w)| cwd.starts_with(&w.path))
        .max_by_key(|(_, w)| w.path.components().count())
        .map(|(i, w)| (i, w.path.clone()))
}

fn relative(root: &Path, path: &str) -> Option<String> {
    Path::new(path)
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::diff_state::{DiffSource, DiffState};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::effect::apply;
    use crate::modal::Modal;
    use crate::task::TaskResult;
    use crate::testing::{TestRepo, pump, workspace_for};

    fn open_diff(path: &Path) -> UiRequest {
        UiRequest::OpenFile {
            path: path.to_string_lossy().into_owned(),
            line: None,
            diff: true,
        }
    }

    #[test]
    fn 別のworktreeから頼まれたら移ってから差分を開く() {
        let repo = TestRepo::new();
        let feat = repo.worktree("feat");
        std::fs::write(feat.join("a.txt"), "changed\n").unwrap();
        let (mut ws, mut svc) = workspace_for(&repo);
        assert_ne!(ws.worktree_path(), feat);

        let asked = effects(&mut ws, &feat, open_diff(&feat.join("a.txt")));
        apply(&mut ws, &mut svc, asked);
        pump(&mut ws, &mut svc);

        assert_eq!(ws.worktree_path(), feat);
        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));
    }

    #[test]
    fn 開いたタブを閉じられる() {
        let repo = TestRepo::new();
        let (mut ws, mut svc) = workspace_for(&repo);
        let root = repo.root();
        let file = root.join("a.txt").to_string_lossy().into_owned();
        let open = UiRequest::OpenFile {
            path: file.clone(),
            line: None,
            diff: false,
        };
        let asked = effects(&mut ws, &root, open);
        apply(&mut ws, &mut svc, asked);
        pump(&mut ws, &mut svc);
        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));

        let asked = effects(&mut ws, &root, UiRequest::CloseFile { path: file });
        apply(&mut ws, &mut svc, asked);

        assert!(ws.panels.viewer.tabs().is_empty());
    }

    #[test]
    fn 移る前に投げた差分が届いても移った先で開く() {
        let repo = TestRepo::new();
        let feat = repo.worktree("feat");
        std::fs::write(feat.join("a.txt"), "feat change\n").unwrap();
        let (mut ws, mut svc) = workspace_for(&repo);
        let previous = ws.worktree_path();
        let mut stale = DiffState::new(DiffSource::working_tree("main"));
        stale.load(&previous, false, 4);

        let asked = effects(&mut ws, &feat, open_diff(&feat.join("a.txt")));
        apply(&mut ws, &mut svc, asked);
        let follow = ws.accept(TaskResult::Diff {
            root: previous,
            diff: Box::new(stale),
        });
        apply(&mut ws, &mut svc, follow);
        pump(&mut ws, &mut svc);

        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));
    }

    #[test]
    fn エディタを開いている間は別のworktreeへ移らない() {
        let repo = TestRepo::new();
        let feat = repo.worktree("feat");
        let (mut ws, mut svc) = workspace_for(&repo);
        let root = ws.worktree_path();
        let argv = ["/bin/sh", "-c", "sleep 30"].map(String::from);
        ws.panels
            .terminal
            .open_editor(&root.join("a.txt"), &root, &argv)
            .unwrap();

        let asked = effects(&mut ws, &feat, UiRequest::AnalyzeReview);
        apply(&mut ws, &mut svc, asked);

        assert!(ws.panels.terminal.editor_name().is_some());
        assert_eq!(ws.worktree_path(), root);
    }

    #[test]
    fn 頼まれた解析の確認は打ちかけの文字とenterで承認されない() {
        let repo = TestRepo::new();
        let feat = repo.worktree("feat");
        let (mut ws, mut svc) = workspace_for(&repo);

        let asked = effects(&mut ws, &feat, UiRequest::AnalyzeReview);
        apply(&mut ws, &mut svc, asked);
        let Some(Modal::Confirm(confirm)) = ws.modals.last_mut() else {
            panic!("確認が出ていない");
        };
        let typed = "cargo build".chars().map(KeyCode::Char);
        let answered: Vec<Effect> = typed
            .chain([KeyCode::Enter])
            .flat_map(|code| confirm.update(KeyEvent::new(code, KeyModifiers::NONE)))
            .collect();

        assert!(
            !answered.iter().any(|e| matches!(e, Effect::Spawn(_))),
            "{answered:?}"
        );
    }

    #[test]
    fn 画面に無いworktreeの頼みは断る() {
        let repo = TestRepo::new();
        let (mut ws, _svc) = workspace_for(&repo);
        let elsewhere = Path::new("/nowhere/else");

        let asked = effects(&mut ws, elsewhere, open_diff(&elsewhere.join("a.txt")));

        assert!(
            matches!(asked.as_slice(), [Effect::Status(StatusLevel::Warning, _)]),
            "{asked:?}"
        );
    }
}
