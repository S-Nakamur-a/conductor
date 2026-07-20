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
