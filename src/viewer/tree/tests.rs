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
    ViewerState::walk_dir(root, root, 0, &mut top);
    assert!(
        top.iter().any(|e| e.name == "generated" && e.is_dir),
        "gitignored directory should still be listed: {:?}",
        top.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Expanding it must reveal nested files, including gitignored ones.
    let mut children = Vec::new();
    ViewerState::read_dir_entries(root, &root.join("generated"), 1, &mut children);
    let names: Vec<&str> = children.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"out.txt"), "files: {names:?}");
    assert!(names.contains(&"sub"), "files: {names:?}");
    assert!(
        names.contains(&"debug.log"),
        "gitignored file should be listed: {names:?}"
    );

    // And recursion continues one level deeper.
    let mut deep = Vec::new();
    ViewerState::read_dir_entries(root, &root.join("generated/sub"), 2, &mut deep);
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
    ViewerState::walk_dir(root, root, 0, &mut top);
    assert!(top.iter().any(|e| e.name == "src"));
    assert!(
        !top.iter().any(|e| e.name == "node_modules"),
        "node_modules should be skipped"
    );
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
    vs.open_file(root, "a.txt", 4);
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
    vs.open_file(root, "a.txt", 4);
    vs.diff_view.diff_mode = true;
    vs.diff_view.diff_view_scroll = 3;

    vs.load_file_tree(root, 4);

    assert!(vs.diff_view.diff_mode, "diff mode must survive a refresh");
    assert!(!vs.is_summary(), "diff mode must not turn on summary view");
    assert_eq!(vs.diff_view.diff_view_scroll, 3);
}
