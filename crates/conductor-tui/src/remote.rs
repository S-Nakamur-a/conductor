//! パネルの Claude が MCP 越しに頼んだ画面操作を Effect に直す。
//!
//! 開く・作る頼みは、頼んだ Claude の worktree を画面に出してから操作する。人が別の
//! worktree を見ていても、頼まれたものが見えないまま終わるよりは移るほうを選んだ。

use std::path::Path;

use conductor_core::cc_hook::UiRequest;

use crate::command::CommandId;
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
    let mut effects = Vec::new();
    match request {
        UiRequest::OpenFile { path, line, diff } => {
            if !shown {
                effects.push(Effect::SelectWorktree(index));
            }
            effects.push(match (diff, relative(&root, &path)) {
                (true, Some(rel)) => Effect::OpenChangedFile { path: rel, line },
                _ => Effect::OpenFile {
                    path: path.into(),
                    line,
                    diff: None,
                    preview: false,
                },
            });
        }
        // 閉じるために画面を移ると、閉じた結果ではなく移った先が見える。
        UiRequest::CloseFile { path } => {
            let closed = shown
                .then(|| relative(&root, &path))
                .flatten()
                .and_then(|rel| ws.panels.viewer.close_path(&rel));
            effects.extend(closed.unwrap_or_else(|| {
                vec![Effect::Status(
                    StatusLevel::Warning,
                    format!("Claude asked to close {path}, which is not open here."),
                )]
            }));
        }
        UiRequest::AnalyzeReview => {
            if !shown {
                effects.push(Effect::SelectWorktree(index));
            }
            effects.push(Effect::Command(CommandId::AnalyzeRevidere));
        }
    }
    effects
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
    use crate::effect::apply;
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
