//! パネルの Claude が MCP 越しに頼んだ画面操作を Effect に直す。
//!
//! 開く頼みは、頼んだ Claude の worktree を画面に出してから操作する。人が別の
//! worktree を見ていても、頼まれたものが見えないまま終わるよりは移るほうを選んだ。
//!
//! 一覧が開いているときは、一覧で選んでいる会話の頼みだけを左下で受ける。ほかの会話の
//! 都合で、人が見ている一覧の選択や画面を動かさない。

use std::path::{Path, PathBuf};

use conductor_core::cc_hook::UiRequest;

use crate::effect::Effect;
use crate::workspace::{Focus, StatusLevel, Workspace};

pub fn effects(
    ws: &mut Workspace,
    cwd: &Path,
    panel: Option<&str>,
    request: UiRequest,
) -> Vec<Effect> {
    if ws.chrome.fleet_open {
        return under_list(ws, cwd, panel, request)
            .unwrap_or_else(|text| vec![Effect::Status(StatusLevel::Warning, text)]);
    }
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
            let mut effects = if shown {
                Vec::new()
            } else {
                crate::effect::select_worktree(ws, index)
            };
            effects.push(match (diff, relative(&root, &path)) {
                (true, Some(rel)) => Effect::ShowChange { path: rel, line },
                _ => Effect::OpenFile {
                    path: path.into(),
                    line,
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
    }
}

fn under_list(
    ws: &mut Workspace,
    cwd: &Path,
    panel: Option<&str>,
    request: UiRequest,
) -> Result<Vec<Effect>, String> {
    let selected = ws.fleet_session();
    if panel.is_none() || panel != selected.as_deref() {
        return Err(format!(
            "Claude in {} asked to show a file; select that session in the list to see it.",
            cwd.display()
        ));
    }
    let fleet = &mut ws.panels.fleet;
    if !fleet.lower_shown() {
        return Err(
            "Claude asked to show a file, but this window has no room under the list.".into(),
        );
    }
    let dir = fleet
        .selected_dir()
        .filter(|dir| canonical(cwd) == *dir)
        .ok_or_else(|| {
            format!(
                "Claude asked from {}, which is not the worktree under the list.",
                cwd.display()
            )
        })?;
    let (path, opening) = match &request {
        UiRequest::OpenFile { path, .. } => (path, true),
        UiRequest::CloseFile { path } => (path, false),
    };
    if opening && !Path::new(path).is_file() {
        return Err(format!("Claude asked for {path}, which does not exist."));
    }
    let rel = canonical(Path::new(path))
        .strip_prefix(&dir)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|rel| !rel.is_empty())
        .ok_or_else(|| {
            format!(
                "Claude asked for {path}, which is outside {}.",
                dir.display()
            )
        })?;
    match request {
        UiRequest::OpenFile { line, diff, .. } => {
            fleet.changes.open_asked(&dir, rel.clone(), line, diff);
            // 左下のシェルに打っている最中は、打ち先を消さない。
            if ws.focus == Focus::FleetShell {
                return Ok(vec![Effect::Status(
                    StatusLevel::Info,
                    format!("Claude opened {rel}; ctrl+g shows it under the list."),
                )]);
            }
            fleet.put_changes();
            Ok(Vec::new())
        }
        UiRequest::CloseFile { .. } => fleet
            .changes
            .close_asked(&dir, &rel)
            .then(Vec::new)
            .ok_or_else(|| {
                format!("Claude asked to close {rel}, which is not shown under the list.")
            }),
    }
}

/// 一覧の木は正規化したパスで並ぶ。読めなければそのまま比べる。
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

    use crate::effect::apply;
    use crate::task::TaskResult;
    use crate::testing::{TestRepo, pump, workspace_for};

    fn open_diff(path: &Path) -> UiRequest {
        UiRequest::OpenFile {
            path: path.to_string_lossy().into_owned(),
            line: None,
            diff: true,
        }
    }

    fn assert_shows_change(ws: &Workspace, path: &str) {
        use crate::panels::changes::Tab;
        assert_eq!(ws.center_tab(), Tab::Files);
        let changes = &ws.panels.changes;
        let top = changes
            .diff()
            .map(|d| d.files[changes.top().file].path.as_str());
        assert_eq!(top, Some(path));
    }

    #[test]
    fn 別のworktreeから頼まれたら移ってから差分を開く() {
        let repo = TestRepo::new();
        let feat = repo.worktree("feat");
        std::fs::write(feat.join("a.txt"), "changed\n").unwrap();
        let (mut ws, mut svc) = workspace_for(&repo);
        assert_ne!(ws.worktree_path(), feat);

        let asked = effects(&mut ws, &feat, None, open_diff(&feat.join("a.txt")));
        apply(&mut ws, &mut svc, asked);
        pump(&mut ws, &mut svc);

        assert_eq!(ws.worktree_path(), feat);
        assert_shows_change(&ws, "a.txt");
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
        let asked = effects(&mut ws, &root, None, open);
        apply(&mut ws, &mut svc, asked);
        pump(&mut ws, &mut svc);
        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));

        let asked = effects(&mut ws, &root, None, UiRequest::CloseFile { path: file });
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

        let asked = effects(&mut ws, &feat, None, open_diff(&feat.join("a.txt")));
        apply(&mut ws, &mut svc, asked);
        let follow = ws.accept(TaskResult::Diff {
            root: previous,
            diff: Box::new(stale),
        });
        apply(&mut ws, &mut svc, follow);
        pump(&mut ws, &mut svc);

        assert_shows_change(&ws, "a.txt");
    }

    #[test]
    fn 画面に無いworktreeの頼みは断る() {
        let repo = TestRepo::new();
        let (mut ws, _svc) = workspace_for(&repo);
        let elsewhere = Path::new("/nowhere/else");

        let asked = effects(
            &mut ws,
            elsewhere,
            None,
            open_diff(&elsewhere.join("a.txt")),
        );

        assert!(
            matches!(asked.as_slice(), [Effect::Status(StatusLevel::Warning, _)]),
            "{asked:?}"
        );
    }

    mod under_list {
        use std::path::PathBuf;

        use conductor_core::diff_state::{DiffHunk, DiffLine, DiffLineTag, FileDiff, FileStatus};
        use conductor_core::git_engine::{Head, WorktreeHead};
        use conductor_core::keymap::Action;
        use conductor_svc::Services;
        use ratatui::layout::Rect;

        use super::*;
        use crate::panels::fleet::{FleetStatus, OwnSession, RepoTree};

        struct Fixture {
            ws: Workspace,
            dir: PathBuf,
            panel: String,
            _tmp: tempfile::TempDir,
        }

        fn changed(path: &str) -> FileDiff {
            let lines = (1..=30)
                .map(|n| DiffLine {
                    tag: DiffLineTag::Insert,
                    old_line_no: None,
                    new_line_no: Some(n),
                    inline_segments: Vec::new(),
                    content: format!("{path} {n}"),
                })
                .collect();
            FileDiff {
                path: path.into(),
                status: FileStatus::Modified,
                added_lines: 30,
                deleted_lines: 0,
                hunks: vec![DiffHunk {
                    lines,
                    func_header: None,
                }],
            }
        }

        fn land(ws: &mut Workspace, dir: &Path, paths: &[&str]) {
            let mut diff = DiffState::new(DiffSource::working_tree("main"));
            diff.files = paths.iter().map(|p| changed(p)).collect();
            diff.rebuild_display_list();
            ws.accept(TaskResult::FleetChanges {
                dir: dir.to_path_buf(),
                diff: Box::new(diff),
                status: Default::default(),
            });
        }

        /// 一覧でこの窓の会話を 1 つ選び、その worktree の変更が左下に据わったところ。
        fn fixture(height: u16) -> Fixture {
            let tmp = tempfile::tempdir().unwrap();
            let dir = std::fs::canonicalize(tmp.path()).unwrap();
            for file in ["a.rs", "b.rs", "plain.rs"] {
                std::fs::write(dir.join(file), "x\n").unwrap();
            }
            let mut ws = Workspace::for_test();
            let mut svc = Services::new();
            apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
            ws.panels.terminal.show_shell_as_claude_for_test(&dir);
            let panel = ws.panels.terminal.visible_claude().unwrap().to_string();
            ws.apply_fleet(
                FleetStatus {
                    trees: vec![RepoTree {
                        root: dir.clone(),
                        name: "t".into(),
                        worktrees: vec![WorktreeHead {
                            path: dir.clone(),
                            head: Head::Branch("main".into()),
                        }],
                    }],
                    ..Default::default()
                },
                vec![OwnSession {
                    panel_id: panel.clone(),
                    working_dir: dir.clone(),
                    claude_session_id: None,
                }],
            );
            for _ in 0..5 {
                if ws.fleet_session().as_deref() == Some(panel.as_str()) {
                    break;
                }
                ws.dispatch_to(Focus::Fleet, Action::NavigateDown);
            }
            assert_eq!(ws.fleet_session().as_deref(), Some(panel.as_str()));
            let l = crate::layout::layout(&ws, Rect::new(0, 0, 160, height));
            ws.sync_layout(&l);
            land(&mut ws, &dir, &["a.rs", "b.rs"]);
            Fixture {
                ws,
                dir,
                panel,
                _tmp: tmp,
            }
        }

        fn open(dir: &Path, file: &str, diff: bool) -> UiRequest {
            UiRequest::OpenFile {
                path: dir.join(file).to_string_lossy().into_owned(),
                line: None,
                diff,
            }
        }

        fn ask(f: &mut Fixture, request: UiRequest) -> Vec<Effect> {
            let panel = f.panel.clone();
            effects(&mut f.ws, &f.dir.clone(), Some(&panel), request)
        }

        fn ask_open(f: &mut Fixture, file: &str, diff: bool) -> Vec<Effect> {
            let request = open(&f.dir, file, diff);
            ask(f, request)
        }

        fn picked(f: &Fixture) -> Option<&str> {
            f.ws.panels.fleet.changes.selected_path()
        }

        fn warned(effects: &[Effect]) -> bool {
            matches!(effects, [Effect::Status(StatusLevel::Warning, _)])
        }

        #[test]
        fn 選んでいる会話が_diff_で頼むと左下でそのファイルを選びフォーカスは動かさない() {
            let mut f = fixture(40);
            let asked = ask_open(&mut f, "b.rs", true);
            assert!(asked.is_empty(), "{asked:?}");
            assert_eq!(picked(&f), Some("b.rs"));
            assert_eq!(f.ws.focus, Focus::Fleet);
        }

        #[test]
        fn 左下に入っている間に頼まれても左下に居たまま切り替わる() {
            let mut f = fixture(40);
            f.ws.focus = Focus::FleetChanges;
            ask_open(&mut f, "b.rs", true);
            assert_eq!(picked(&f), Some("b.rs"));
            assert_eq!(f.ws.focus, Focus::FleetChanges);
        }

        #[test]
        fn 左下がシェルか_artifact_なら_git_changes_に替え_右の_claude_に居るフォーカスは動かさない()
         {
            for artifacts in [false, true] {
                let mut f = fixture(40);
                match artifacts {
                    true => {
                        f.ws.dispatch_to(Focus::Fleet, Action::ShowFleetArtifacts);
                    }
                    false => f.ws.panels.fleet.show_shell(),
                }
                assert!(!f.ws.panels.fleet.shows_changes());
                f.ws.focus = Focus::FleetSession;
                ask_open(&mut f, "b.rs", true);
                assert!(f.ws.panels.fleet.shows_changes(), "artifacts={artifacts}");
                assert_eq!(f.ws.focus, Focus::FleetSession);
                assert_eq!(picked(&f), Some("b.rs"));
            }
        }

        #[test]
        fn 左下のシェルに打っている最中は左下を替えず_ctrl_g_を案内する() {
            let mut f = fixture(40);
            f.ws.panels.fleet.show_shell();
            f.ws.focus = Focus::FleetShell;
            let asked = ask_open(&mut f, "b.rs", true);
            assert!(
                matches!(asked.as_slice(), [Effect::Status(StatusLevel::Info, t)] if t.contains("ctrl+g")),
                "{asked:?}"
            );
            assert!(f.ws.panels.fleet.shows_shell());
            assert_eq!(f.ws.focus, Focus::FleetShell);
            assert_eq!(picked(&f), Some("b.rs"), "頼みを預けていない");
        }

        #[test]
        fn 左下の区画が無い窓では警告だけ出す() {
            let mut f = fixture(15);
            assert!(!f.ws.panels.fleet.lower_shown());
            let asked = ask_open(&mut f, "b.rs", true);
            assert!(warned(&asked), "{asked:?}");
            assert_ne!(picked(&f), Some("b.rs"));
        }

        #[test]
        fn 無いファイルの頼みは警告だけで左下を変えない() {
            let mut f = fixture(40);
            let asked = ask_open(&mut f, "gone.rs", false);
            assert!(warned(&asked), "{asked:?}");
            assert_eq!(picked(&f), Some("a.rs"));
        }

        #[test]
        fn 左下の_worktree_と違う場所からの頼みと外のファイルは警告だけ() {
            let mut f = fixture(40);
            let sub = f.dir.join("sub");
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("c.rs"), "x\n").unwrap();
            let panel = f.panel.clone();
            let asked = effects(&mut f.ws, &sub, Some(&panel), open(&sub, "c.rs", true));
            assert!(warned(&asked), "{asked:?}");
            let outside = tempfile::NamedTempFile::new().unwrap();
            let far = UiRequest::OpenFile {
                path: outside.path().to_string_lossy().into_owned(),
                line: None,
                diff: false,
            };
            assert!(warned(&ask(&mut f, far)));
            let sibling = outside.path().file_name().unwrap().to_owned();
            let dotted = UiRequest::OpenFile {
                path: f
                    .dir
                    .join("sub/../..")
                    .join(sibling)
                    .to_string_lossy()
                    .into_owned(),
                line: None,
                diff: false,
            };
            assert!(warned(&ask(&mut f, dotted)));
            assert_eq!(picked(&f), Some("a.rs"));
        }

        #[test]
        fn 選んでいない会話とパネルの分からない頼みは警告だけ() {
            let mut f = fixture(40);
            let dir = f.dir.clone();
            for panel in [Some("someone-else"), None] {
                let asked = effects(&mut f.ws, &dir, panel, open(&dir, "b.rs", true));
                assert!(warned(&asked), "{panel:?} {asked:?}");
            }
            assert_eq!(picked(&f), Some("a.rs"));
        }

        #[test]
        fn 一覧で_worktree_の行を選んでいる間はその会話の頼みでも警告だけ() {
            let mut f = fixture(40);
            for _ in 0..5 {
                if f.ws.fleet_session().is_none() {
                    break;
                }
                f.ws.dispatch_to(Focus::Fleet, Action::NavigateUp);
            }
            assert_eq!(f.ws.fleet_session(), None);
            let asked = ask_open(&mut f, "b.rs", true);
            assert!(warned(&asked), "{asked:?}");
        }

        #[test]
        fn 閉じる頼みは左下に出しているファイルだけを閉じ_それ以外は警告() {
            let mut f = fixture(40);
            ask_open(&mut f, "plain.rs", false);
            assert_eq!(f.ws.panels.fleet.changes.provisional(), Some("plain.rs"));
            let close = |dir: &Path, file: &str| UiRequest::CloseFile {
                path: dir.join(file).to_string_lossy().into_owned(),
            };
            let dir = f.dir.clone();
            assert!(warned(&ask(&mut f, close(&dir, "b.rs"))));
            assert!(ask(&mut f, close(&dir, "plain.rs")).is_empty());
            assert_eq!(f.ws.panels.fleet.changes.provisional(), None);
            assert!(warned(&ask(&mut f, close(&dir, "plain.rs"))));
        }
    }
}
