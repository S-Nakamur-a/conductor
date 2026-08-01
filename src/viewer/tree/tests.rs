use super::*;

/// The explorer must list files purely from the filesystem, independent of
/// git state. A directory ignored by `.gitignore` (i.e. not under git
/// management) and the files nested inside it must still be reachable.
#[test]
fn walk_includes_gitignored_directories_and_recurses() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();

    // A `.gitignore` that excludes `generated/` (and `*.log`) from git
    // management. `generated` is deliberately NOT one of the heavy
    // `SKIP_DIRS`, so the only reason it could be hidden would be gitignore.
    std::fs::write(root.join(".gitignore"), "/generated\n*.log\n").unwrap();
    std::fs::create_dir_all(root.join("generated/sub")).unwrap();
    std::fs::write(root.join("generated/out.txt"), "x").unwrap();
    std::fs::write(root.join("generated/sub/inner.txt"), "x").unwrap();
    std::fs::write(root.join("generated/debug.log"), "x").unwrap();

    // Top-level walk must surface the gitignored directory itself.
    let mut top = Vec::new();
    // No real git repo here — an empty status map (everything reads as
    // Tracked) is fine since this test is only about the plain filesystem
    // walk, not git-state classification (see `git_status_map` tests for that).
    let git_status = GitStatusMap::default();
    ViewerState::walk_dir(root, root, 0, &mut top, &git_status);
    assert!(
        top.iter().any(|e| e.name == "generated" && e.is_dir),
        "gitignored directory should still be listed: {:?}",
        top.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Expanding it must reveal nested files, including gitignored ones.
    let mut children = Vec::new();
    ViewerState::walk_dir(root, &root.join("generated"), 1, &mut children, &git_status);
    let names: Vec<&str> = children.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"out.txt"), "files: {names:?}");
    assert!(names.contains(&"sub"), "files: {names:?}");
    assert!(
        names.contains(&"debug.log"),
        "gitignored file should be listed: {names:?}"
    );

    // And recursion continues one level deeper.
    let mut deep = Vec::new();
    ViewerState::walk_dir(root, &root.join("generated/sub"), 2, &mut deep, &git_status);
    assert!(
        deep.iter().any(|e| e.name == "inner.txt"),
        "deep files: {:?}",
        deep.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
}

/// Heavy build/dependency directories are still skipped — that guard is a
/// performance concern, not a git-management one.
#[test]
fn walk_still_skips_heavy_dirs() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();

    let mut top = Vec::new();
    ViewerState::walk_dir(root, root, 0, &mut top, &GitStatusMap::default());
    assert!(top.iter().any(|e| e.name == "src"));
    assert!(
        !top.iter().any(|e| e.name == "node_modules"),
        "node_modules should be skipped"
    );
}

/// 同名のファイルを持つ 2 つの worktree があるとき、開く先は「今表示している
/// ツリーを歩いた根」で決まる。
///
/// worktree の切り替えはツリーの走査を裏のスレッドに回すので、選択が B に移って
/// からエントリが届くまでの間、選択は B・表示しているエントリは A、という状態が
/// 実在する。根を Viewer が持ち、エントリと一緒に差し替えることでこの隙間を潰す。
/// 呼び出し側が開くたびに「今どの worktree か」を引き直していた頃は、その隙間の
/// クリックが A の相対パスを B の根に繋いで、別ブランチの同名ファイルを黙って
/// 開いていた。
#[test]
fn tree_root_and_entries_switch_together() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("shared.txt"), "FROM_A\n").unwrap();
    std::fs::write(b.path().join("shared.txt"), "FROM_B\n").unwrap();

    let mut vs = ViewerState::default();
    vs.load_file_tree(a.path(), 4);
    vs.open_file("shared.txt", 4);
    assert_eq!(vs.content.file_content, vec!["FROM_A"]);
    assert_eq!(vs.root(), a.path(), "load_file_tree が根を確定させる");

    // 裏の走査が返ってきたのと同じ形で B のツリーを適用する。
    let mut entries = Vec::new();
    ViewerState::walk_dir(
        b.path(),
        b.path(),
        0,
        &mut entries,
        &GitStatusMap::default(),
    );
    vs.replace_tree(b.path().to_path_buf(), entries, GitStatusMap::default());

    // 相対パスは同じでも、読むのは差し替え後の根の下のファイル。
    vs.open_file("shared.txt", 4);
    assert_eq!(vs.content.file_content, vec!["FROM_B"]);
    assert_eq!(vs.root(), b.path());
}

/// A periodic / file-watcher tree refresh re-opens the previously viewed file
/// to pick up on-disk edits, and `open_file` goes through `exit_diff_mode`,
/// which clears every viewer mode flag. The SUMMARY pseudo-file view must
/// survive that round trip — otherwise selecting SUMMARY in the Changed-files
/// list silently flips back to the last opened file within seconds.
#[test]
fn tree_refresh_preserves_summary_view() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();

    let mut vs = ViewerState::default();
    vs.load_file_tree(root, 4);
    vs.open_file("a.txt", 4);
    vs.enter_summary_view();
    vs.summary_scroll = 7;

    vs.load_file_tree(root, 4);

    assert!(
        vs.is_summary(),
        "summary view must survive a tree refresh (was kicked back to a.txt)"
    );
    assert_eq!(vs.summary_scroll, 7, "summary scroll must be preserved");
}

/// The sibling guarantee for the unified diff view, which the summary fix must
/// not regress: a refresh keeps diff mode on.
#[test]
fn tree_refresh_preserves_diff_mode() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::write(root.join("a.txt"), "hello\n").unwrap();

    let mut vs = ViewerState::default();
    vs.load_file_tree(root, 4);
    vs.open_file("a.txt", 4);
    vs.diff_view.diff_mode = true;
    vs.diff_view.diff_view_scroll = 3;

    vs.load_file_tree(root, 4);

    assert!(vs.diff_view.diff_mode, "diff mode must survive a refresh");
    assert!(!vs.is_summary(), "diff mode must not turn on summary view");
    assert_eq!(vs.diff_view.diff_view_scroll, 3);
}

/// The same guarantee for the rendered-markdown view. `open_file` resets
/// `md_scroll` — correct when the *user* opens a file, wrong on the refresh
/// path, which re-opens the current file every time the watcher fires or the
/// 3s poll comes round. Without the save/restore, reading a long rendered
/// README would snap back to the top every few seconds.
#[test]
fn tree_refresh_preserves_rendered_markdown_scroll() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::write(root.join("README.md"), "# t\n\nbody\n".repeat(50)).unwrap();

    let mut vs = ViewerState::default();
    vs.load_file_tree(root, 4);
    vs.open_file("README.md", 4);
    vs.md_rendered = true;
    vs.md_scroll = 40;
    vs.content.file_scroll = 40;

    vs.load_file_tree(root, 4);

    assert_eq!(vs.content.file_scroll, 40, "raw scroll is preserved");
    assert_eq!(vs.md_scroll, 40, "rendered scroll must be preserved too");
    assert!(vs.md_rendered, "the mode itself must survive a refresh");
}
