use std::path::{Path, PathBuf};

use conductor_core::diff_state::{
    DiffHunk, DiffLine, DiffLineTag, DiffSource, DiffState, FileDiff, FileStatus,
};
use conductor_core::git_engine::WorktreeInfo;
use conductor_core::keymap::{Action, KeyContext};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::render::{body_lines, list_lines, tab_at, tab_title};
use super::*;
use crate::effect::Effect;
use crate::layout::layout;
use crate::list::Viewport;
use crate::task::{ReviewWrite, TaskResult};
use crate::workspace::{Focus, Workspace};

fn line(tag: DiffLineTag, old: Option<usize>, new: Option<usize>, text: &str) -> DiffLine {
    DiffLine {
        tag,
        old_line_no: old,
        new_line_no: new,
        inline_segments: Vec::new(),
        content: text.into(),
    }
}

/// new 側の start 行目から lines 行の文脈に、1 行の削除と 1 行の追加を挟んだファイル。
fn file(path: &str, status: FileStatus, start: usize, lines: usize) -> FileDiff {
    let mut hunk = Vec::new();
    for n in 0..lines {
        hunk.push(line(
            DiffLineTag::Equal,
            Some(start + n),
            Some(start + n),
            &format!("{path}:{}", start + n),
        ));
    }
    hunk.push(line(
        DiffLineTag::Delete,
        Some(start + lines),
        None,
        "removed",
    ));
    hunk.push(line(
        DiffLineTag::Insert,
        None,
        Some(start + lines),
        "added",
    ));
    FileDiff {
        path: path.into(),
        status,
        added_lines: 1,
        deleted_lines: 1,
        hunks: vec![DiffHunk {
            lines: hunk,
            func_header: None,
        }],
    }
}

fn binary(path: &str) -> FileDiff {
    FileDiff {
        path: path.into(),
        status: FileStatus::Added,
        added_lines: 0,
        deleted_lines: 0,
        hunks: Vec::new(),
    }
}

fn working(files: Vec<FileDiff>) -> DiffState {
    let mut diff = DiffState::new(DiffSource::working_tree("main"));
    diff.files = files;
    diff
}

/// レビューの無い窓の Ctx でパネルだけを動かす。
fn install(panel: &mut ChangesPanel, root: &Path, diff: &DiffState) -> Option<String> {
    let ws = Workspace::for_test();
    panel.install(root, diff, &ws.ctx())
}

fn reveal(panel: &mut ChangesPanel, path: String, line: Option<usize>, loading: bool) -> Reveal {
    let ws = Workspace::for_test();
    panel.reveal(path, line, loading, &ws.ctx())
}

fn body(panel: &mut ChangesPanel, action: Action) -> Option<Vec<Effect>> {
    let ws = Workspace::for_test();
    panel.update_body(action, &ws.ctx())
}

fn loaded(files: Vec<FileDiff>) -> ChangesPanel {
    let mut panel = ChangesPanel::new(false);
    panel.follow(Path::new("/r/feat"));
    install(&mut panel, Path::new("/r/feat"), &working(files));
    panel
}

pub(crate) fn two_files() -> Vec<FileDiff> {
    vec![
        file("a.rs", FileStatus::Modified, 1, 3),
        file("b.rs", FileStatus::Modified, 10, 3),
    ]
}

fn text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn info(path: &str, is_main: bool) -> WorktreeInfo {
    WorktreeInfo {
        path: PathBuf::from(path),
        branch: if is_main { "main" } else { "feat" }.into(),
        is_main,
        counts: None,
        ahead: None,
        behind: None,
        head_oid: None,
    }
}

/// worktree の一覧を受け取り、index 番目を選んだ窓。投げる Task は走らせない。
fn workspace_on(list: Vec<WorktreeInfo>, index: usize) -> Workspace {
    let mut ws = Workspace::for_test();
    let _ = ws.accept(TaskResult::Worktrees(Ok(list)));
    let _ = crate::effect::select_worktree(&mut ws, index);
    ws
}

pub(crate) fn feat_workspace() -> Workspace {
    workspace_on(vec![info("/r/main", true), info("/r/feat", false)], 1)
}

pub(crate) fn deliver(ws: &mut Workspace, root: &str, files: Vec<FileDiff>) -> Vec<Effect> {
    ws.accept(TaskResult::Diff {
        root: PathBuf::from(root),
        diff: Box::new(working(files)),
    })
}

#[test]
fn main_worktree_は_code_それ以外は_files_changed_一覧が届く前は_code() {
    let panel = ChangesPanel::new(false);
    let root = Path::new("/r/x");
    assert_eq!(panel.tab(root, Some(true)), Tab::Code);
    assert_eq!(panel.tab(root, Some(false)), Tab::Files);
    assert_eq!(panel.tab(root, None), Tab::Code);
}

#[test]
fn 人が選んだタブは_worktree_ごとに覚える() {
    let mut panel = ChangesPanel::new(false);
    panel.choose(Path::new("/r/feat"), Tab::Code);
    assert_eq!(panel.tab(Path::new("/r/feat"), Some(false)), Tab::Code);
    assert_eq!(panel.tab(Path::new("/r/other"), Some(false)), Tab::Files);
}

#[test]
fn 窓が選んでいる_worktree_で既定のタブが決まる() {
    assert_eq!(feat_workspace().center_tab(), Tab::Files);
    let main = workspace_on(vec![info("/r/main", true), info("/r/feat", false)], 0);
    assert_eq!(main.center_tab(), Tab::Code);
    assert_eq!(Workspace::for_test().center_tab(), Tab::Code);
}

#[test]
fn files_changed_では左と中央の列が一覧と縦並びに替わる() {
    let mut ws = feat_workspace();
    let l = layout(&ws, Rect::new(0, 0, 160, 40));
    assert!(l.rect(Region::ChangesList).is_some() && l.rect(Region::Changes).is_some());
    assert!(l.rect(Region::ExplorerTree).is_none() && l.rect(Region::Viewer).is_none());

    ws.panels.changes.choose(Path::new("/r/feat"), Tab::Code);
    let l = layout(&ws, Rect::new(0, 0, 160, 40));
    assert!(l.rect(Region::ExplorerTree).is_some() && l.rect(Region::Viewer).is_some());
    assert!(l.rect(Region::Changes).is_none());
}

#[test]
fn files_changed_のキーは一覧と縦並びの層で解決し縦並びへ届く() {
    let mut ws = feat_workspace();
    ws.focus = Focus::Explorer;
    assert_eq!(ws.key_context(), KeyContext::ChangesList);
    ws.focus = Focus::Viewer;
    assert_eq!(ws.key_context(), KeyContext::Changes);
    let _ = deliver(&mut ws, "/r/feat", two_files());
    assert_eq!(ws.dispatch(Action::NextChangedFile), Some(Vec::new()));
    assert_eq!(ws.panels.changes.top(), Top { file: 1, row: 0 });
}

#[test]
fn タブの切替は左と中央の数字で_claude_の列からは無い() {
    let ws = Workspace::for_test();
    let one = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
    let two = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE);
    for context in [
        KeyContext::Explorer,
        KeyContext::ExplorerDiffList,
        KeyContext::Viewer,
        KeyContext::ViewerDiffMode,
        KeyContext::ChangesList,
        KeyContext::Changes,
    ] {
        assert_eq!(
            ws.keymap.resolve(&one, context),
            Some(Action::ShowCodeTab),
            "{context:?}"
        );
        assert_eq!(
            ws.keymap.resolve(&two, context),
            Some(Action::ShowFilesTab),
            "{context:?}"
        );
    }
    assert_ne!(
        ws.keymap.resolve(&one, KeyContext::Terminal),
        Some(Action::ShowCodeTab)
    );
}

#[test]
fn タブの切替キーは覚える側に落ちる() {
    let mut ws = feat_workspace();
    ws.focus = Focus::Viewer;
    assert_eq!(ws.dispatch(Action::ShowCodeTab), None);
    let effects = crate::route::global_effects(&ws, Action::ShowCodeTab);
    assert!(matches!(effects.as_slice(), [Effect::ShowTab(Tab::Code)]));
    let effects = crate::route::global_effects(&ws, Action::ShowFilesTab);
    assert!(matches!(effects.as_slice(), [Effect::ShowTab(Tab::Files)]));
}

#[test]
fn 読み込み中_変更なし_失敗はそれぞれ言う() {
    let mut ws = feat_workspace();
    let body = body_lines(&ws, 80, 10);
    assert_eq!(text(&body[0]).trim(), "loading\u{2026}");

    let _ = deliver(&mut ws, "/r/feat", Vec::new());
    let body = body_lines(&ws, 80, 10);
    assert_eq!(text(&body[0]).trim(), "No changes against main");

    let mut failed = working(Vec::new());
    failed.error = Some("could not resolve main".into());
    let _ = ws.accept(TaskResult::Diff {
        root: PathBuf::from("/r/feat"),
        diff: Box::new(failed),
    });
    let body: Vec<String> = body_lines(&ws, 80, 10).iter().map(text).collect();
    assert_eq!(body, [" could not resolve main"]);
}

#[test]
fn base_を解決できず_head_基準でも理由を先頭に残す() {
    let mut ws = feat_workspace();
    let mut fallback = working(two_files());
    fallback.error = Some("main not found; showing changes against HEAD".into());
    let _ = ws.accept(TaskResult::Diff {
        root: PathBuf::from("/r/feat"),
        diff: Box::new(fallback),
    });
    ws.panels.changes.scroll(4);
    let body = body_lines(&ws, 80, 10);
    assert!(text(&body[0]).contains("showing changes against HEAD"));
}

#[test]
fn 全ファイルを見出しの下に縦に並べ_印と変更の種類を見出しに出す() {
    let mut ws = feat_workspace();
    let files = vec![
        file("a.rs", FileStatus::Deleted, 1, 1),
        file("b.rs", FileStatus::Untracked, 1, 1),
    ];
    let _ = deliver(&mut ws, "/r/feat", files);
    let body: Vec<String> = body_lines(&ws, 60, 20).iter().map(text).collect();
    assert!(body[0].starts_with(" [ ] D a.rs  +1 \u{2212}1"), "{body:?}");
    assert!(body[1].contains("a.rs:1"));
    assert_eq!(body[4], "");
    assert!(body[5].starts_with(" [ ] U b.rs"), "{body:?}");
}

#[test]
fn ハンクの無いファイルは断り書きの_1_行() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", vec![binary("logo.png")]);
    let body: Vec<String> = body_lines(&ws, 60, 20).iter().map(text).collect();
    assert_eq!(body[1].trim(), "Binary or empty file");
}

#[test]
fn 描くのは見えている行だけ() {
    let mut ws = feat_workspace();
    let files = (0..300)
        .map(|i| file(&format!("f{i:03}.rs"), FileStatus::Modified, 1, 50))
        .collect();
    let _ = deliver(&mut ws, "/r/feat", files);
    assert_eq!(body_lines(&ws, 60, 12).len(), 12);
}

#[test]
fn 変更サマリは先頭にいる間だけ出す() {
    let mut ws = feat_workspace();
    ws.review.install(Ok(crate::review::Snapshot {
        summary: Some("why this branch exists".into()),
        ..Default::default()
    }));
    let _ = deliver(&mut ws, "/r/feat", two_files());
    let body: Vec<String> = body_lines(&ws, 60, 20).iter().map(text).collect();
    assert!(body.iter().any(|l| l.contains("why this branch exists")));
    ws.panels.changes.scroll(1);
    let body: Vec<String> = body_lines(&ws, 60, 20).iter().map(text).collect();
    assert!(!body.iter().any(|l| l.contains("why this branch exists")));
}

#[test]
fn 一覧で選ぶと中央がその見出しへ行きフォーカスは動かない() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", two_files());
    ws.sync_layout(&layout(&ws, Rect::new(0, 0, 160, 40)));
    ws.focus = Focus::Explorer;
    assert_eq!(ws.dispatch(Action::NavigateDown), Some(Vec::new()));
    assert_eq!(ws.panels.changes.top(), Top { file: 1, row: 0 });
    assert_eq!(ws.focus, Focus::Explorer);
    let list: Vec<String> = list_lines(&ws).iter().map(text).collect();
    assert_eq!(list, [" M a.rs", " M b.rs"]);
}

#[test]
fn 一覧の行のクリックでそのファイルへ飛ぶ() {
    let mut panel = loaded(two_files());
    panel.list_view = Viewport::new(5, 10);
    assert!(panel.click_list(6));
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
    assert!(!panel.click_list(9));
}

#[test]
fn 大文字の_j_と_k_で見出しを渡る() {
    let mut panel = loaded(two_files());
    body(&mut panel, Action::NextChangedFile);
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
    body(&mut panel, Action::NextChangedFile);
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
    body(&mut panel, Action::NavigateDown);
    body(&mut panel, Action::PrevChangedFile);
    assert_eq!(
        panel.top(),
        Top { file: 1, row: 0 },
        "途中からは今の見出しへ"
    );
    body(&mut panel, Action::PrevChangedFile);
    assert_eq!(panel.top(), Top { file: 0, row: 0 });
    assert_eq!(panel.cursor().selected(), 0, "一覧の選択も付いてくる");
}

#[test]
fn 縦の送りはファイルをまたいで両端で止まる() {
    let mut panel = loaded(two_files());
    panel.body_height = 10;
    // 1 ファイル = 見出し + 本文 5 行 + 空行。
    assert_eq!(panel.height(0), 7);
    body(&mut panel, Action::ScrollHalfPageDown);
    assert_eq!(panel.top(), Top { file: 0, row: 5 });
    body(&mut panel, Action::NavigateDown);
    body(&mut panel, Action::NavigateDown);
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
    body(&mut panel, Action::ScrollHalfPageUp);
    assert_eq!(panel.top(), Top { file: 0, row: 2 });
    body(&mut panel, Action::GoToBottom);
    assert_eq!(panel.top(), Top { file: 0, row: 5 });
    body(&mut panel, Action::GoToTop);
    assert_eq!(panel.top(), Top::default());
    body(&mut panel, Action::NavigateUp);
    assert_eq!(panel.top(), Top::default());
}

#[test]
fn 左右表示に切り替えると今のファイルの見出しに戻る() {
    let mut panel = loaded(two_files());
    panel.scroll(9);
    assert_eq!(panel.top(), Top { file: 1, row: 2 });
    body(&mut panel, Action::ToggleDiffView);
    assert!(panel.side_by_side());
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
    assert_eq!(panel.height(1), 7, "削除と追加が向かい合って 1 行になる");
}

#[test]
fn 読み直しても一番上をパスと新しい側の行で保つ() {
    let mut panel = loaded(two_files());
    // b.rs の 11 行目。
    panel.scroll(9);
    let mut moved = two_files();
    moved.insert(0, file("0.rs", FileStatus::Added, 1, 2));
    moved[2] = file("b.rs", FileStatus::Modified, 8, 6);
    install(&mut panel, Path::new("/r/feat"), &working(moved));
    assert_eq!(panel.top(), Top { file: 2, row: 4 });
}

#[test]
fn 削除行が一番上なら直前の行番号で保つ() {
    let mut panel = loaded(two_files());
    // a.rs の削除行。直前は 3 行目。
    panel.scroll(4);
    install(&mut panel, Path::new("/r/feat"), &working(two_files()));
    assert_eq!(panel.top(), Top { file: 0, row: 3 });
}

#[test]
fn ファイルが消えたらパス順で次の見出しへ() {
    let mut panel = loaded(vec![
        file("a.rs", FileStatus::Modified, 1, 1),
        file("b.rs", FileStatus::Modified, 1, 1),
        file("c.rs", FileStatus::Modified, 1, 1),
    ]);
    body(&mut panel, Action::NextChangedFile);
    body(&mut panel, Action::NavigateDown);
    let left = vec![
        file("a.rs", FileStatus::Modified, 1, 1),
        file("c.rs", FileStatus::Modified, 1, 1),
    ];
    install(&mut panel, Path::new("/r/feat"), &working(left));
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
}

#[test]
fn worktree_を移って戻ると一番上が戻る() {
    let mut panel = loaded(two_files());
    panel.scroll(9);
    panel.follow(Path::new("/r/other"));
    install(&mut panel, Path::new("/r/other"), &working(two_files()));
    assert_eq!(panel.top(), Top::default());
    panel.follow(Path::new("/r/feat"));
    assert!(panel.diff().is_none(), "戻った直後は読み込み中");
    install(&mut panel, Path::new("/r/feat"), &working(two_files()));
    assert_eq!(panel.top(), Top { file: 1, row: 2 });
}

#[test]
fn 別の根とコミットを出どころにした結果は捨てる() {
    let mut panel = ChangesPanel::new(false);
    panel.follow(Path::new("/r/feat"));
    install(&mut panel, Path::new("/r/main"), &working(two_files()));
    assert!(panel.diff().is_none());
    let mut commit = DiffState::new(DiffSource::commit("abc"));
    commit.files = two_files();
    install(&mut panel, Path::new("/r/feat"), &commit);
    assert!(panel.diff().is_none());
}

#[test]
fn 届く前に頼まれたファイルは届いてから出し_無ければ言う() {
    let mut panel = ChangesPanel::new(false);
    panel.follow(Path::new("/r/feat"));
    assert_eq!(
        reveal(&mut panel, "b.rs".into(), Some(12), false),
        Reveal::Waiting
    );
    assert_eq!(
        install(&mut panel, Path::new("/r/feat"), &working(two_files())),
        None
    );
    // 1〜9 行目の隠れた塊、10、11、12 行目。
    assert_eq!(panel.top(), Top { file: 1, row: 4 });

    panel.follow(Path::new("/r/x"));
    reveal(&mut panel, "gone.rs".into(), None, false);
    let missing = install(&mut panel, Path::new("/r/x"), &working(two_files()));
    assert_eq!(missing.as_deref(), Some("gone.rs"));
    assert_eq!(
        reveal(&mut panel, "gone.rs".into(), None, false),
        Reveal::Missing
    );
    assert_eq!(
        reveal(&mut panel, "gone.rs".into(), None, true),
        Reveal::Waiting
    );
}

#[test]
fn 差分つきで開く頼みは_files_changed_のその行へ() {
    let mut ws = workspace_on(vec![info("/r/main", true), info("/r/feat", false)], 0);
    let _ = crate::effect::select_worktree(&mut ws, 1);
    let mut svc = conductor_svc::Services::new();
    crate::effect::apply(
        &mut ws,
        &mut svc,
        vec![Effect::ShowChange {
            path: "b.rs".into(),
            line: Some(12),
        }],
    );
    assert_eq!(ws.center_tab(), Tab::Files);
    assert_eq!(ws.focus, Focus::Viewer);
    let _ = deliver(&mut ws, "/r/feat", two_files());
    assert_eq!(ws.panels.changes.top(), Top { file: 1, row: 4 });
}

#[test]
fn ファイルを開くと_code_タブへ移る() {
    let mut ws = feat_workspace();
    let mut svc = conductor_svc::Services::new();
    crate::effect::apply(
        &mut ws,
        &mut svc,
        vec![Effect::OpenFile {
            path: "a.rs".into(),
            line: None,
            diff: None,
            preview: false,
        }],
    );
    assert_eq!(ws.center_tab(), Tab::Code);
}

#[test]
fn エディタを開いている間はタブを切り替えない() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut ws = feat_workspace();
    let mut svc = conductor_svc::Services::new();
    let argv = ["/bin/sh", "-c", "sleep 30"].map(String::from);
    ws.panels
        .terminal
        .open_editor(&dir.path().join("a.txt"), dir.path(), &argv)
        .unwrap();
    crate::effect::apply(&mut ws, &mut svc, vec![Effect::ShowTab(Tab::Code)]);
    assert_eq!(ws.center_tab(), Tab::Files);
    let status = ws.chrome.status.as_ref().map(|s| s.text.as_str());
    assert!(
        status.is_some_and(|s| s.starts_with("Close ")),
        "{status:?}"
    );
}

#[test]
fn 枠の題でタブを示し押した桁のタブを返す() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", two_files());
    let title = tab_title(&ws);
    assert_eq!(title, "  1 Code  [2 Files changed 2] ");
    assert_eq!(tab_at(&ws, 0), None);
    assert_eq!(tab_at(&ws, 2), Some(Tab::Code));
    let gap = title.find('[').unwrap() - 1;
    assert_eq!(tab_at(&ws, gap), None, "タブの間の空白");
    assert_eq!(tab_at(&ws, gap + 1), Some(Tab::Files));
    assert_eq!(tab_at(&ws, title.find("Files").unwrap()), Some(Tab::Files));
}

#[test]
fn 中央のホイールは一覧の選択も連れていく() {
    let mut panel = loaded(two_files());
    panel.wheel(7);
    assert_eq!(panel.top(), Top { file: 1, row: 0 });
    assert_eq!(panel.cursor().selected(), 1);
}

fn deleted(path: &str, lines: usize) -> FileDiff {
    let hunk = (1..=lines)
        .map(|n| line(DiffLineTag::Delete, Some(n), None, "gone"))
        .collect();
    FileDiff {
        path: path.into(),
        status: FileStatus::Deleted,
        added_lines: 0,
        deleted_lines: lines,
        hunks: vec![DiffHunk {
            lines: hunk,
            func_header: None,
        }],
    }
}

fn with_c() -> Vec<FileDiff> {
    let mut files = two_files();
    files.push(file("c.rs", FileStatus::Added, 1, 1));
    files
}

fn show_change(ws: &mut Workspace, path: &str) {
    let mut svc = conductor_svc::Services::new();
    let change = Effect::ShowChange {
        path: path.into(),
        line: None,
    };
    crate::effect::apply(ws, &mut svc, vec![change]);
}

fn status(ws: &Workspace) -> Option<&str> {
    ws.chrome.status.as_ref().map(|s| s.text.as_str())
}

#[test]
fn 読み直し中に写しに無いファイルを頼まれたら届いてから出す() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", two_files());
    let _ = ws.panels.explorer.changes.reload();
    show_change(&mut ws, "c.rs");
    assert_eq!(
        status(&ws),
        Some("Opening c.rs once the diff loads\u{2026}")
    );
    let _ = deliver(&mut ws, "/r/feat", with_c());
    assert_eq!(ws.panels.changes.top(), Top { file: 2, row: 0 });
}

#[test]
fn コミットを選んでいる間に頼まれたら作業ツリーへ戻して届いてから出す() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", two_files());
    let _ = ws
        .panels
        .explorer
        .changes
        .set_source(DiffSource::commit("abc"));
    show_change(&mut ws, "c.rs");
    assert!(matches!(
        ws.panels.explorer.changes.source(),
        DiffSource::WorkingTree { .. }
    ));
    let _ = deliver(&mut ws, "/r/feat", with_c());
    assert_eq!(ws.panels.changes.top(), Top { file: 2, row: 0 });
}

#[test]
fn 頼まれたパスは変更ファイルの一覧と同じ規則で探す() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", two_files());
    show_change(&mut ws, "./b.rs");
    assert_eq!(ws.panels.changes.top(), Top { file: 1, row: 0 });
}

#[test]
fn 新しい側の行番号が無い行も読み直しで位置を保つ() {
    let files = || {
        vec![
            deleted("a.rs", 30),
            file("b.rs", FileStatus::Modified, 1, 3),
        ]
    };
    let mut panel = loaded(files());
    for top in [
        Top { file: 0, row: 20 },
        Top { file: 0, row: 31 },
        Top { file: 0, row: 1 },
    ] {
        panel.top = top;
        install(&mut panel, Path::new("/r/feat"), &working(files()));
        assert_eq!(panel.top(), top);
    }
    panel.top = Top { file: 0, row: 20 };
    panel.follow(Path::new("/r/other"));
    panel.follow(Path::new("/r/feat"));
    install(&mut panel, Path::new("/r/feat"), &working(files()));
    assert_eq!(panel.top(), Top { file: 0, row: 20 });
}

#[test]
fn 区切りの空行が一番上なら直前の行で保つ() {
    let mut panel = loaded(two_files());
    panel.top = Top { file: 0, row: 6 };
    install(&mut panel, Path::new("/r/feat"), &working(two_files()));
    assert_eq!(panel.top(), Top { file: 0, row: 5 });
}

#[test]
fn ハンクの無いファイルの断り書きと区切りの空行も位置を保つ() {
    let files = || vec![binary("logo.png"), file("b.rs", FileStatus::Modified, 1, 3)];
    let mut panel = loaded(files());
    for row in [1, 2] {
        panel.top = Top { file: 0, row };
        install(&mut panel, Path::new("/r/feat"), &working(files()));
        assert_eq!(panel.top(), Top { file: 0, row });
    }
}

#[test]
fn コミットを選んでいる間に写しにあるファイルを頼まれても作業ツリーを読み直す() {
    let mut ws = feat_workspace();
    let _ = deliver(&mut ws, "/r/feat", two_files());
    let _ = ws
        .panels
        .explorer
        .changes
        .set_source(DiffSource::commit("abc"));
    let effects = crate::effect::show_change(&mut ws, "b.rs".into(), None);
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Spawn(crate::task::Task::ComputeDiff { source, .. })
                if matches!(source, DiffSource::WorkingTree { .. })
        )),
        "{effects:?}"
    );
    assert_eq!(ws.panels.changes.top(), Top { file: 1, row: 0 });
}

fn comment_at(id: &str, path: &str, line: u32) -> conductor_core::review_store::ReviewComment {
    crate::review::tests::comment(id, path, line, None)
}

fn snapshot(comments: Vec<conductor_core::review_store::ReviewComment>) -> crate::review::Snapshot {
    crate::review::Snapshot {
        branch: "feat".into(),
        comments,
        ..Default::default()
    }
}

/// a.rs と c.rs はどちらも 1〜3 行目が文脈、4 行目が削除と追加。行は 見出し, 1, 2, 3, 削除, 4, 空行。
fn reviewed(comments: Vec<conductor_core::review_store::ReviewComment>) -> Workspace {
    let mut ws = feat_workspace();
    ws.sync_layout(&layout(&ws, Rect::new(0, 0, 160, 40)));
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(snapshot(comments)))));
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![
            file("a.rs", FileStatus::Modified, 1, 3),
            file("c.rs", FileStatus::Modified, 1, 3),
        ],
    );
    ws.focus = Focus::Viewer;
    ws
}

fn rows(ws: &Workspace, file: usize) -> Vec<Row> {
    ws.panels.changes.sections()[file].rows.clone()
}

fn top_to(ws: &mut Workspace, file: usize, row: usize) {
    ws.panels.changes.top = Top { file, row };
}

fn submit(ws: &Workspace, effects: Vec<Effect>) -> Vec<Effect> {
    let mut effects = effects;
    let Some(Effect::PushModal(mut modal @ crate::modal::Modal::CommentEditor(_))) = effects.pop()
    else {
        panic!("{effects:?}");
    };
    let ctx = ws.ctx();
    modal.update(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE), &ctx);
    modal.update(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &ctx)
}

fn warning(effects: &[Effect]) -> String {
    match effects {
        [Effect::Status(_, text)] => text.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn 一番上の行で_c_を押すとその行と窓の_branch_で入力欄が開く() {
    let mut ws = reviewed(Vec::new());
    top_to(&mut ws, 0, 2);
    let effects = ws.dispatch(Action::AddComment).unwrap();
    let written = submit(&ws, effects);
    assert!(
        matches!(
            written.as_slice(),
            [_, Effect::Spawn(crate::task::Task::WriteReview(ReviewWrite::AddComment { branch, file_path, line_start: 2, .. }))]
                if branch == "feat" && file_path == "a.rs"
        ),
        "{written:?}"
    );
}

#[test]
fn 削除行_見出し_断り書き_空行_左右表示ではコメントを断る() {
    let mut ws = reviewed(Vec::new());
    top_to(&mut ws, 0, 4);
    assert!(warning(&ws.dispatch(Action::AddComment).unwrap()).contains("deleted line"));
    for row in [0, 6] {
        top_to(&mut ws, 0, row);
        assert!(
            warning(&ws.dispatch(Action::AddComment).unwrap()).contains("line of code"),
            "row {row}"
        );
    }
    let _ = deliver(&mut ws, "/r/feat", vec![binary("logo.png")]);
    top_to(&mut ws, 0, 1);
    assert!(warning(&ws.dispatch(Action::AddComment).unwrap()).contains("line of code"));

    let mut ws = reviewed(Vec::new());
    let _ = ws.dispatch(Action::ToggleDiffView);
    top_to(&mut ws, 0, 2);
    assert!(warning(&ws.dispatch(Action::AddComment).unwrap()).contains("unified"));
}

#[test]
fn スレッドの行と畳んだファイルの空行でもコメントを断る() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2)]);
    top_to(&mut ws, 0, 3);
    assert!(matches!(rows(&ws, 0)[3], Row::Thread { .. }));
    assert!(warning(&ws.dispatch(Action::AddComment).unwrap()).contains("line of code"));

    top_to(&mut ws, 1, 0);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    assert_eq!(rows(&ws, 1), [Row::Header, Row::Blank]);
    top_to(&mut ws, 1, 1);
    assert!(warning(&ws.dispatch(Action::AddComment).unwrap()).contains("line of code"));
}

#[test]
fn 左右表示でも返信と解決とスレッドの開閉は効く() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 4)]);
    let _ = ws.dispatch(Action::ToggleDiffView);
    let section = &ws.panels.changes.sections()[0];
    let row = (0..section.rows.len())
        .find(|row| matches!(section.rows[*row], Row::Body(_)) && section.line_of(*row) == Some(4))
        .unwrap();
    top_to(&mut ws, 0, row);
    let effects = ws.dispatch(Action::ReplyToComment).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [Effect::PushModal(crate::modal::Modal::CommentEditor(_))]
    ));
    let effects = ws.dispatch(Action::ToggleResolve).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [Effect::Spawn(crate::task::Task::WriteReview(
            ReviewWrite::SetStatus { .. }
        ))]
    ));
    let _ = ws.dispatch(Action::ToggleInlineThread);
    assert!(
        !rows(&ws, 0)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
}

#[test]
fn 隠れた行の塊にはその先頭の行でコメントを付ける() {
    let mut ws = feat_workspace();
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![file("b.rs", FileStatus::Modified, 10, 3)],
    );
    ws.focus = Focus::Viewer;
    top_to(&mut ws, 0, 1);
    let effects = ws.dispatch(Action::AddComment).unwrap();
    let written = submit(&ws, effects);
    assert!(
        matches!(
            written.as_slice(),
            [
                _,
                Effect::Spawn(crate::task::Task::WriteReview(ReviewWrite::AddComment {
                    line_start: 1,
                    ..
                }))
            ]
        ),
        "{written:?}"
    );
}

#[test]
fn コメントのある行に印を付け未解決のスレッドを行の直後に開く() {
    let ws = reviewed(vec![comment_at("x", "a.rs", 2)]);
    let thread = rows(&ws, 0)
        .iter()
        .filter(|row| matches!(row, Row::Thread { anchor: 2, .. }))
        .count();
    assert!(thread > 0);
    assert_eq!(rows(&ws, 0)[3], Row::Thread { anchor: 2, line: 0 });
    let body: Vec<String> = body_lines(&ws, 80, 30).iter().map(text).collect();
    let at = body.iter().position(|l| l.contains("a.rs:2")).unwrap();
    assert!(!body[at].starts_with("  "), "印が付く: {:?}", body[at]);
    assert!(
        body[at + 1..at + 1 + thread]
            .iter()
            .any(|l| l.contains("body of x")),
        "{body:?}"
    );
    assert!(body[at + 1 + thread].contains("a.rs:3"));
}

#[test]
fn 描かれない行のコメントは出さない() {
    let mut ws = feat_workspace();
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(snapshot(vec![
        comment_at("x", "b.rs", 5),
    ])))));
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![file("b.rs", FileStatus::Modified, 10, 3)],
    );
    assert!(
        !rows(&ws, 0)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
    let body: Vec<String> = body_lines(&ws, 80, 30).iter().map(text).collect();
    assert!(!body.iter().any(|l| l.contains("body of x")), "{body:?}");
}

#[test]
fn space_はそのファイルのスレッドだけを開閉する() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2), comment_at("y", "c.rs", 2)]);
    top_to(&mut ws, 0, 2);
    assert_eq!(ws.dispatch(Action::ToggleInlineThread), Some(Vec::new()));
    assert!(
        !rows(&ws, 0)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
    assert!(
        rows(&ws, 1)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
    let _ = ws.dispatch(Action::ToggleInlineThread);
    assert!(
        rows(&ws, 0)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
}

#[test]
fn スレッドの行で_r_は解決を切り替え_大文字の_r_は返信を開く() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2)]);
    top_to(&mut ws, 0, 4);
    assert!(matches!(
        ws.panels.changes.sections()[0].rows[4],
        Row::Thread { .. }
    ));
    let effects = ws.dispatch(Action::ToggleResolve).unwrap();
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Spawn(crate::task::Task::WriteReview(ReviewWrite::SetStatus { id, .. }))] if id == "x"
        ),
        "{effects:?}"
    );
    let effects = ws.dispatch(Action::ReplyToComment).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [Effect::PushModal(crate::modal::Modal::CommentEditor(_))]
    ));
    top_to(&mut ws, 0, 1);
    assert_eq!(
        warning(&ws.dispatch(Action::ReplyToComment).unwrap()),
        "no comment on this line"
    );
}

#[test]
fn 波括弧でスレッドのある行へファイルを跨いで渡る() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2), comment_at("y", "c.rs", 3)]);
    let row_of = |ws: &Workspace, file: usize, line: usize| {
        let section = &ws.panels.changes.sections()[file];
        (0..section.rows.len())
            .find(|row| section.line_of(*row) == Some(line))
            .unwrap()
    };
    assert_eq!(ws.dispatch(Action::NextComment), Some(Vec::new()));
    assert_eq!(
        ws.panels.changes.top(),
        Top {
            file: 0,
            row: row_of(&ws, 0, 2)
        }
    );
    let _ = ws.dispatch(Action::NextComment);
    assert_eq!(
        ws.panels.changes.top(),
        Top {
            file: 1,
            row: row_of(&ws, 1, 3)
        }
    );
    assert_eq!(
        warning(&ws.dispatch(Action::NextComment).unwrap()),
        "no comment on this line"
    );
    let _ = ws.dispatch(Action::PrevComment);
    assert_eq!(
        ws.panels.changes.top(),
        Top {
            file: 0,
            row: row_of(&ws, 0, 2)
        }
    );

    let thread_row = row_of(&ws, 1, 3) + 1;
    top_to(&mut ws, 1, thread_row);
    let _ = ws.dispatch(Action::PrevComment);
    assert_eq!(
        ws.panels.changes.top(),
        Top {
            file: 0,
            row: row_of(&ws, 0, 2)
        },
        "スレッドの行からは付いている行を飛ばす"
    );
}

#[test]
fn 波括弧は畳んだファイルのスレッドを飛ばす() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2), comment_at("y", "c.rs", 3)]);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    top_to(&mut ws, 1, 0);
    assert_eq!(
        warning(&ws.dispatch(Action::PrevComment).unwrap()),
        "no comment on this line"
    );
    assert_eq!(ws.panels.changes.top(), Top { file: 1, row: 0 });
}

#[test]
fn 手で開閉した畳みとスレッドは_worktree_を戻ると戻る() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2)]);
    top_to(&mut ws, 0, 2);
    let _ = ws.dispatch(Action::ToggleInlineThread);
    top_to(&mut ws, 1, 0);
    let _ = ws.dispatch(Action::ToggleInlineThread);
    let before = (rows(&ws, 0), rows(&ws, 1));
    assert!(!before.0.iter().any(|row| matches!(row, Row::Thread { .. })));
    assert_eq!(before.1, [Row::Header, Row::Blank]);

    ws.panels.changes.follow(Path::new("/r/other"));
    ws.panels.changes.follow(Path::new("/r/feat"));
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![
            file("a.rs", FileStatus::Modified, 1, 3),
            file("c.rs", FileStatus::Modified, 1, 3),
        ],
    );
    assert_eq!((rows(&ws, 0), rows(&ws, 1)), before);
}

#[test]
fn 別のプロセスが書いたコメントは次の読み直しで出る() {
    let mut ws = reviewed(Vec::new());
    assert!(
        !rows(&ws, 0)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(snapshot(vec![
        comment_at("x", "a.rs", 2),
    ])))));
    assert!(
        rows(&ws, 0)
            .iter()
            .any(|row| matches!(row, Row::Thread { .. }))
    );
}

#[test]
fn 組み直しても一番上は同じ行のまま() {
    let mut ws = reviewed(Vec::new());
    top_to(&mut ws, 0, 3);
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(snapshot(vec![
        comment_at("x", "a.rs", 2),
    ])))));
    let top = ws.panels.changes.top();
    assert_eq!(
        ws.panels.changes.sections()[0].line_of(top.row),
        Some(3),
        "{top:?}"
    );

    ws.sync_layout(&layout(&ws, Rect::new(0, 0, 100, 40)));
    let top = ws.panels.changes.top();
    assert_eq!(
        ws.panels.changes.sections()[0].line_of(top.row),
        Some(3),
        "{top:?}"
    );
}

#[test]
fn 幅が変わるとスレッドの折り返しを組み直す() {
    let mut long = comment_at("x", "a.rs", 2);
    long.body = "word ".repeat(40);
    let mut ws = reviewed(vec![long]);
    let wide = rows(&ws, 0).len();
    ws.sync_layout(&layout(&ws, Rect::new(0, 0, 90, 40)));
    assert!(rows(&ws, 0).len() > wide);
}

#[test]
fn 左右表示のスレッドは右の新しい行の直後に開く() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 4)]);
    let _ = ws.dispatch(Action::ToggleDiffView);
    let rows = rows(&ws, 0);
    let at = rows
        .iter()
        .position(|row| matches!(row, Row::Thread { .. }))
        .unwrap();
    assert_eq!(
        ws.panels.changes.sections()[0].line_of(at - 1),
        Some(4),
        "{rows:?}"
    );
    assert!(matches!(rows[at - 1], Row::Body(_)));
}

fn apply(ws: &mut Workspace, effects: Vec<Effect>) {
    let mut svc = conductor_svc::Services::new();
    crate::effect::apply(ws, &mut svc, effects);
}

#[test]
fn v_で一番上のファイルを_viewed_にして見出しだけに畳む() {
    let mut ws = reviewed(Vec::new());
    top_to(&mut ws, 0, 3);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    assert!(matches!(effects.as_slice(), [Effect::ToggleViewed(path)] if path == "a.rs"));
    apply(&mut ws, effects);
    assert!(ws.review.is_viewed("a.rs"));
    assert_eq!(rows(&ws, 0), [Row::Header, Row::Blank]);
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 0 });
    let header = text(&body_lines(&ws, 80, 5)[0]);
    assert!(header.starts_with(" [x] M a.rs"), "{header}");
}

#[test]
fn v_を書き込みが返る前に続けて押すと_2_回目で戻る() {
    let mut ws = reviewed(Vec::new());
    for want in [true, false] {
        let effects = ws.dispatch(Action::ToggleViewed).unwrap();
        apply(&mut ws, effects);
        assert_eq!(ws.review.is_viewed("a.rs"), want);
    }
    assert!(rows(&ws, 0).len() > 2, "戻すと開く");
}

#[test]
fn 見出しで_space_は_viewed_のまま開き_もう一度で畳む() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    let _ = ws.dispatch(Action::ToggleInlineThread);
    assert!(rows(&ws, 0).len() > 2);
    assert!(ws.review.is_viewed("a.rs"));
    let _ = ws.dispatch(Action::ToggleInlineThread);
    assert_eq!(rows(&ws, 0), [Row::Header, Row::Blank]);

    let _ = ws.dispatch(Action::ToggleInlineThread);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    assert!(!ws.review.is_viewed("a.rs"));
    assert!(
        rows(&ws, 0).len() > 2,
        "手で開いた後に v で外しても開いたまま"
    );

    top_to(&mut ws, 1, 0);
    let _ = ws.dispatch(Action::ToggleInlineThread);
    assert_eq!(
        rows(&ws, 1),
        [Row::Header, Row::Blank],
        "viewed でなくても畳める"
    );
}

#[test]
fn viewed_はファイルがまた変わっても畳んだまま() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![file("a.rs", FileStatus::Modified, 1, 5)],
    );
    assert_eq!(rows(&ws, 0), [Row::Header, Row::Blank]);
}

#[test]
fn 削除されたファイルの本文で_v_を押すと一番上は見出し() {
    let mut ws = feat_workspace();
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![deleted("a.rs", 5), file("b.rs", FileStatus::Modified, 1, 3)],
    );
    ws.focus = Focus::Viewer;
    top_to(&mut ws, 0, 3);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 0 });
}

#[test]
fn 断り書きで_v_を押すと一番上は見出し() {
    let mut ws = feat_workspace();
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![binary("logo.png"), file("b.rs", FileStatus::Modified, 1, 3)],
    );
    ws.focus = Focus::Viewer;
    top_to(&mut ws, 0, 1);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 0 });
}

#[test]
fn スレッドの途中で読み直しが届いても位置を保つ() {
    let mut long = comment_at("x", "a.rs", 2);
    long.body = "word ".repeat(80);
    let mut ws = reviewed(vec![long.clone()]);
    let at = rows(&ws, 0)
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, Row::Thread { .. }))
        .nth(3)
        .unwrap()
        .0;
    top_to(&mut ws, 0, at);
    let reloaded = snapshot(vec![long, comment_at("y", "c.rs", 2)]);
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(reloaded))));
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: at });
}

#[test]
fn 削除の塊の途中でレビューの読み直しが届いても位置を保つ() {
    let mut hunk = vec![line(DiffLineTag::Equal, Some(1), Some(1), "ctx")];
    for n in 2..=41 {
        hunk.push(line(DiffLineTag::Delete, Some(n), None, "gone"));
    }
    hunk.push(line(DiffLineTag::Equal, Some(42), Some(2), "ctx2"));
    let removed = FileDiff {
        path: "a.rs".into(),
        status: FileStatus::Modified,
        added_lines: 0,
        deleted_lines: 40,
        hunks: vec![DiffHunk {
            lines: hunk,
            func_header: None,
        }],
    };
    let mut ws = feat_workspace();
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(snapshot(Vec::new())))));
    let _ = deliver(&mut ws, "/r/feat", vec![removed]);
    top_to(&mut ws, 0, 30);
    let reloaded = snapshot(vec![comment_at("y", "z.rs", 1)]);
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(reloaded))));
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 30 });
}

#[test]
fn viewed_で畳んだファイルも差分つきで開く頼みではその行を出す() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    apply(
        &mut ws,
        vec![Effect::ShowChange {
            path: "a.rs".into(),
            line: Some(3),
        }],
    );
    let top = ws.panels.changes.top();
    assert_eq!(ws.panels.changes.sections()[0].line_of(top.row), Some(3));
    assert!(ws.review.is_viewed("a.rs"), "viewed の印はそのまま");
}

#[test]
fn 区切りの空行で_v_を押しても一番上は見出し() {
    let mut ws = reviewed(Vec::new());
    top_to(&mut ws, 0, 6);
    assert_eq!(rows(&ws, 0)[6], Row::Blank);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 0 });
}

#[test]
fn 畳んだファイルの空行にいる間にレビューの読み直しが届いても動かない() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    top_to(&mut ws, 0, 1);
    let mut reloaded = snapshot(vec![comment_at("y", "c.rs", 2)]);
    reloaded.viewed.insert("a.rs".into());
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(reloaded))));
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 1 });
}

#[test]
fn 読んでいたスレッドの行が閉じたら付いている行へ落ちる() {
    let mut ws = reviewed(vec![comment_at("x", "a.rs", 2)]);
    top_to(&mut ws, 0, 3);
    assert!(matches!(rows(&ws, 0)[3], Row::Thread { .. }));
    let _ = ws.dispatch(Action::ToggleInlineThread);
    let top = ws.panels.changes.top();
    assert_eq!(ws.panels.changes.sections()[0].line_of(top.row), Some(2));
}

#[test]
fn 畳んだファイルを綴りの違うパスや届く前に頼まれても開いてその行を出す() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    apply(
        &mut ws,
        vec![Effect::ShowChange {
            path: "./a.rs".into(),
            line: Some(3),
        }],
    );
    let top = ws.panels.changes.top();
    assert_eq!(ws.panels.changes.sections()[0].line_of(top.row), Some(3));

    let mut ws = feat_workspace();
    let mut viewed = snapshot(Vec::new());
    viewed.viewed.insert("a.rs".into());
    let _ = ws.accept(TaskResult::Review(Ok(Box::new(viewed))));
    apply(
        &mut ws,
        vec![Effect::ShowChange {
            path: "a.rs".into(),
            line: Some(3),
        }],
    );
    let _ = deliver(
        &mut ws,
        "/r/feat",
        vec![file("a.rs", FileStatus::Modified, 1, 3)],
    );
    let top = ws.panels.changes.top();
    assert_eq!(ws.panels.changes.sections()[0].line_of(top.row), Some(3));
}

#[test]
fn 畳んだファイルの空行で_viewed_を外すと一番上は見出し() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    top_to(&mut ws, 0, 1);
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    assert!(rows(&ws, 0).len() > 2);
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 0 });
}

#[test]
fn 一覧で畳んだファイルを選んでも開かない() {
    let mut ws = reviewed(Vec::new());
    let effects = ws.dispatch(Action::ToggleViewed).unwrap();
    apply(&mut ws, effects);
    ws.panels.changes.list_view = Viewport::new(5, 10);
    top_to(&mut ws, 1, 0);
    assert!(ws.panels.changes.click_list(5));
    assert_eq!(ws.panels.changes.top(), Top { file: 0, row: 0 });
    assert_eq!(rows(&ws, 0), [Row::Header, Row::Blank]);
}
