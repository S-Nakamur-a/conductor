//! Tests for the diff_state module: inline segment emphasis, case-only
//! rename filtering, and display-list navigation edge cases.

use similar::{ChangeTag, TextDiff};

#[test]
fn test_inline_segments_populated_for_replace() {
    let old = "hello world\n";
    let new = "hello rust\n";
    let diff = TextDiff::from_lines(old, new);

    for op in diff.ops() {
        for change in diff.iter_inline_changes(op) {
            if change.tag() == ChangeTag::Insert {
                let has_emphasis = change.values().iter().any(|(e, _)| *e);
                assert!(has_emphasis, "Insert line should have emphasized segments");
            }
        }
    }
}

/// Test that case-only path differences with identical content are filtered out.
///
/// Creates a git repo where the tree contains entries that differ only in
/// case (e.g. `Photo.png` vs `photo.png`).  On case-insensitive
/// filesystems these refer to the same file, and `compute_diff_range`
/// should exclude them when the blob content is identical.
#[test]
fn test_case_only_rename_filtered_out() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // ── Initial commit on "main" with "Photo.png" ──
    let blob_oid = repo.blob(b"image data").unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("Photo.png", blob_oid, 0o100644).unwrap();
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    let commit1 = repo
        .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    let commit1 = repo.find_commit(commit1).unwrap();

    // ── Second commit on "feature" with "photo.png" (case change only, same blob) ──
    let mut tb2 = repo.treebuilder(None).unwrap();
    tb2.insert("photo.png", blob_oid, 0o100644).unwrap();
    let tree2_oid = tb2.write().unwrap();
    let tree2 = repo.find_tree(tree2_oid).unwrap();

    let commit2 = repo
        .commit(
            Some("refs/heads/feature"),
            &sig,
            &sig,
            "rename case",
            &tree2,
            &[&commit1],
        )
        .unwrap();

    // Point HEAD at feature.
    repo.set_head_detached(commit2).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    // Also create the local branch ref so compute_diff_range can find it.
    repo.branch("feature", &repo.find_commit(commit2).unwrap(), true)
        .unwrap();
    repo.set_head("refs/heads/feature").unwrap();

    let files = DiffState::compute_diff_range(dir.path(), "main", DiffRange::Committed, false, 4)
        .unwrap();

    // The case-only rename with identical content should be filtered out.
    assert!(
        files.is_empty(),
        "case-only rename with same content should be excluded, got: {:?}",
        files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Test that a case rename WITH content changes is NOT filtered out.
#[test]
fn test_case_rename_with_content_change_kept() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // ── Initial commit on "main" with "Photo.png" ──
    let blob1 = repo.blob(b"image data v1").unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("Photo.png", blob1, 0o100644).unwrap();
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    let commit1 = repo
        .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    let commit1 = repo.find_commit(commit1).unwrap();

    // ── Second commit: case change + content change ──
    let blob2 = repo.blob(b"image data v2 -- updated").unwrap();
    let mut tb2 = repo.treebuilder(None).unwrap();
    tb2.insert("photo.png", blob2, 0o100644).unwrap();
    let tree2_oid = tb2.write().unwrap();
    let tree2 = repo.find_tree(tree2_oid).unwrap();

    let commit2 = repo
        .commit(
            Some("refs/heads/feature"),
            &sig,
            &sig,
            "rename + edit",
            &tree2,
            &[&commit1],
        )
        .unwrap();

    repo.set_head_detached(commit2).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    repo.branch("feature", &repo.find_commit(commit2).unwrap(), true)
        .unwrap();
    repo.set_head("refs/heads/feature").unwrap();

    let files = DiffState::compute_diff_range(dir.path(), "main", DiffRange::Committed, false, 4)
        .unwrap();

    // The rename with actual content changes should still appear.
    assert!(
        !files.is_empty(),
        "case rename with content change should NOT be filtered out"
    );
}

/// Regression: collapsing an already-collapsed directory (or expanding an
/// already-expanded one) must be a no-op, not a panic. A `clippy --fix`
/// collapsible_match auto-fix once turned the inner `if` into a match guard,
/// which let these cases fall through to the `unreachable!()` arm.
#[test]
fn collapse_already_collapsed_dir_does_not_panic() {
    use super::*;
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.display_list = vec![DiffListEntry::Directory {
        path: "src".to_string(),
        name: "src".to_string(),
        depth: 0,
        collapsed: true,
    }];
    ds.collapse_section(0); // must not panic
}

#[test]
fn expand_already_expanded_dir_does_not_panic() {
    use super::*;
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.display_list = vec![DiffListEntry::Directory {
        path: "src".to_string(),
        name: "src".to_string(),
        depth: 0,
        collapsed: false,
    }];
    ds.expand_section(0); // must not panic
}

/// `display_index_for_path` is the reverse of `resolve_file` — used to
/// re-sync the diff list's cursor when a file is opened by path (e.g.
/// jumping to a walkthrough step) rather than by list index.
#[test]
fn display_index_for_path_finds_committed_and_uncommitted_files() {
    use super::*;
    let file = |path: &str| FileDiff {
        path: path.to_string(),
        added_lines: 0,
        deleted_lines: 0,
        is_new: false,
        is_deleted: false,
        hunks: Vec::new(),
    };
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.committed_files = vec![file("src/a.rs")];
    ds.uncommitted_files = vec![file("src/b.rs")];
    ds.display_list = vec![
        DiffListEntry::File {
            section: DiffSection::Committed,
            file_index: 0,
            depth: 0,
        },
        DiffListEntry::File {
            section: DiffSection::Uncommitted,
            file_index: 0,
            depth: 0,
        },
    ];

    assert_eq!(ds.display_index_for_path("src/a.rs"), Some(0));
    assert_eq!(ds.display_index_for_path("src/b.rs"), Some(1));
    assert_eq!(ds.display_index_for_path("src/missing.rs"), None);
}
