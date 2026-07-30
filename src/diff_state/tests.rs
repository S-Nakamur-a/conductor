//! Tests for the diff_state module: inline segment emphasis, case-only
//! rename filtering, display-list navigation edge cases, and base-ref
//! resolution (remote-tracking refs, tags, OIDs, and unresolvable bases not
//! taking down the uncommitted diff with them).

use similar::{ChangeTag, TextDiff};

// ── Shared git-repo builders for the base-ref-resolution tests below ──────

/// A throwaway commit signature; identity doesn't matter for these tests.
fn test_signature() -> git2::Signature<'static> {
    git2::Signature::now("test", "test@test.com").unwrap()
}

/// Create a commit with the given flat file contents on top of `parent`
/// (root commit if `None`). Files already in the parent's tree are carried
/// over unchanged. Does not update any ref — callers point branches/tags at
/// the returned oid explicitly, so a test controls exactly which refs exist.
fn commit_files(
    repo: &git2::Repository,
    parent: Option<&git2::Commit>,
    files: &[(&str, &[u8])],
) -> git2::Oid {
    let base_tree = parent.map(|p| p.tree().unwrap());
    let mut tb = repo.treebuilder(base_tree.as_ref()).unwrap();
    for (path, content) in files {
        let oid = repo.blob(content).unwrap();
        tb.insert(*path, oid, 0o100644).unwrap();
    }
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let signature = test_signature();
    let parents: Vec<&git2::Commit> = parent.into_iter().collect();
    repo.commit(None, &signature, &signature, "test commit", &tree, &parents)
        .unwrap()
}

/// Create (or move) local branch `name` to `oid`, make it HEAD, and check it
/// out so the workdir reflects the commit's tree.
fn checkout_branch(repo: &git2::Repository, name: &str, oid: git2::Oid) {
    let commit = repo.find_commit(oid).unwrap();
    repo.branch(name, &commit, true).unwrap();
    repo.set_head(&format!("refs/heads/{name}")).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
}

/// Create a remote-tracking ref `refs/remotes/<name>` pointing at `oid`,
/// without any actual remote configured.
fn set_remote_tracking_ref(repo: &git2::Repository, name: &str, oid: git2::Oid) {
    repo.reference(&format!("refs/remotes/{name}"), oid, true, "test")
        .unwrap();
}

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

    let files =
        DiffState::compute_diff_range(dir.path(), "main", DiffRange::Committed, false, 4).unwrap();

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

    let files =
        DiffState::compute_diff_range(dir.path(), "main", DiffRange::Committed, false, 4).unwrap();

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

// ── Base ref resolution ────────────────────────────────────────────────

/// A worktree whose base was saved as `origin/main` (Conductor's normal case
/// since 2026-07-29) must resolve even when no local `main` branch exists.
#[test]
fn base_ref_resolves_remote_tracking_ref() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    set_remote_tracking_ref(&repo, "origin/main", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    assert!(
        repo.find_branch("main", git2::BranchType::Local).is_err(),
        "test setup invariant: no local 'main' branch should exist"
    );

    let files =
        DiffState::compute_diff_range(dir.path(), "origin/main", DiffRange::Committed, false, 4)
            .unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["b.txt"]);
}

/// The same resolution must work from a linked worktree's `Repository`, not
/// just the main worktree's — remote-tracking refs live in the shared
/// commondir, not per-worktree.
#[test]
fn base_ref_resolves_from_linked_worktree() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    set_remote_tracking_ref(&repo, "origin/main", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    // Link a worktree at a fresh branch pointing at the same commit as
    // "feature" (can't reuse "feature" itself — it's already checked out in
    // the main worktree). Target a not-yet-created path outside the repo's
    // own tempdir: `git worktree add` refuses non-empty directories.
    let wt_parent = tempfile::tempdir().unwrap();
    let wt_path = wt_parent.path().join("linked-wt");
    let status = std::process::Command::new("git")
        // Isolate from the user's global/system git config (e.g. a
        // core.hooksPath post-checkout hook) so this can't fail for reasons
        // unrelated to what's under test.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args([
            "-C",
            dir.path().to_str().unwrap(),
            "worktree",
            "add",
            "-b",
            "wt-branch",
            wt_path.to_str().unwrap(),
            &head_oid.to_string(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "git worktree add failed");

    let files =
        DiffState::compute_diff_range(&wt_path, "origin/main", DiffRange::Committed, false, 4)
            .unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["b.txt"]);
}

/// Lightweight tags, annotated tags, full OIDs, and short OIDs must all
/// resolve to the same base commit.
#[test]
fn base_ref_resolves_tag_lightweight_annotated_and_oid() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    let base_commit = repo.find_commit(base_oid).unwrap();
    let base_obj = repo.find_object(base_oid, None).unwrap();

    repo.tag_lightweight("v-lw", &base_obj, false).unwrap();
    let tagger = test_signature();
    repo.tag("v-ann", &base_obj, &tagger, "annotated tag", false)
        .unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    let full_oid = base_oid.to_string();
    let short_oid = &full_oid[..7];

    for base in ["v-lw", "v-ann", full_oid.as_str(), short_oid] {
        let files = DiffState::compute_diff_range(dir.path(), base, DiffRange::Committed, false, 4)
            .unwrap_or_else(|e| panic!("base '{base}' failed to resolve: {e:#}"));
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["b.txt"],
            "base '{base}' produced unexpected diff"
        );
    }
}

/// A configured base like `develop` that exists only as a remote-tracking
/// ref (no local branch) must resolve via the `origin/` fallback.
#[test]
fn base_ref_falls_back_to_origin_prefix() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    set_remote_tracking_ref(&repo, "origin/develop", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    assert!(
        repo.find_branch("develop", git2::BranchType::Local)
            .is_err(),
        "test setup invariant: no local 'develop' branch should exist"
    );

    let files =
        DiffState::compute_diff_range(dir.path(), "develop", DiffRange::Committed, false, 4)
            .unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["b.txt"]);
}

/// A base that resolves neither directly nor via `origin/` must report an
/// error that names the base the caller actually asked for.
#[test]
fn base_ref_unresolvable_reports_error() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    let err =
        DiffState::compute_diff_range(dir.path(), "nonexistent", DiffRange::Committed, false, 4)
            .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("nonexistent"), "error message was: {msg}");
}

/// A base already qualified as `origin/<name>` must not retry as
/// `origin/origin/<name>` on failure. Without the guard, a base like
/// `origin/weird` would silently retry against `origin/origin/weird`, and if
/// some unrelated ref happens to live at that doubled path, it would resolve
/// there — diffing against a ref the caller never asked for. This test
/// creates exactly that trap ref so a missing guard shows up as an
/// unexpected `Ok`, not just as wording in the error message.
#[test]
fn base_ref_error_does_not_double_the_origin_prefix() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);
    // Exists only at the doubled path. If the `origin/` guard were missing,
    // resolving "origin/weird" would retry as "origin/origin/weird" and
    // land here instead of failing.
    set_remote_tracking_ref(&repo, "origin/origin/weird", oid);

    let err =
        DiffState::compute_diff_range(dir.path(), "origin/weird", DiffRange::Committed, false, 4)
            .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("origin/weird"), "error message was: {msg}");
    assert!(
        !msg.contains("origin/origin"),
        "error message doubled the origin/ prefix: {msg}"
    );
}

/// `revparse_single` follows git's own revspec resolution order
/// (`refs/tags/<name>` before `refs/heads/<name>`; see `gitrevisions(7)`),
/// so a `dup` naming both a tag and a local branch resolves to the tag. The
/// previous `find_branch(Local)` resolution picked the branch instead, so this
/// is a deliberate behavior change: the point is that Conductor's base
/// resolution never disagrees with what `git rev-parse dup` would print. Adding
/// a branch-first special case would make the TUI and the shell pick different
/// bases in the same repo, which is the more confusing of the two options.
#[test]
fn base_ref_prefers_tag_over_branch_like_git_rev_parse() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let root_oid = commit_files(&repo, None, &[("base.txt", b"base")]);
    let root_commit = repo.find_commit(root_oid).unwrap();

    // Local branch "dup" -> a commit that diverges from the tag's target.
    let branch_oid = commit_files(&repo, Some(&root_commit), &[("from_branch.txt", b"a")]);
    repo.branch("dup", &repo.find_commit(branch_oid).unwrap(), false)
        .unwrap();

    // Tag "dup" -> a different commit, also diverging from the branch's.
    let tag_oid = commit_files(&repo, Some(&root_commit), &[("from_tag.txt", b"b")]);
    let tag_commit = repo.find_commit(tag_oid).unwrap();
    let tag_obj = repo.find_object(tag_oid, None).unwrap();
    repo.tag_lightweight("dup", &tag_obj, false).unwrap();

    // HEAD descends from the tag's commit only, so the two resolutions give
    // different merge-bases (and thus different diffs): a branch resolution
    // hits the shared root and picks up "from_tag.txt" too, while a tag
    // resolution treats the tag's commit itself as the merge-base.
    let head_oid = commit_files(&repo, Some(&tag_commit), &[("head.txt", b"c")]);
    checkout_branch(&repo, "feature", head_oid);

    let files =
        DiffState::compute_diff_range(dir.path(), "dup", DiffRange::Committed, false, 4).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["head.txt"],
        "expected 'dup' to resolve to the tag (only head.txt ahead of it), \
         not the branch (which would also include from_tag.txt)"
    );
}

/// Uncommitted diffs (HEAD vs workdir+index) don't depend on the base at
/// all, so an unresolvable base must not prevent them from computing.
#[test]
fn uncommitted_range_ignores_unresolvable_base() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let files =
        DiffState::compute_diff_range(dir.path(), "nonexistent", DiffRange::Uncommitted, false, 4)
            .unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["c.txt"]);
}

// ── load_diff must not clear uncommitted when the base fails ──────────────

/// `load_diff` with an unresolvable base must still surface uncommitted
/// changes and the error, rather than clearing both diff sections.
#[test]
fn load_diff_keeps_uncommitted_when_base_unresolvable() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let mut ds = DiffState::new("nonexistent", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "nonexistent", false, 4);

    assert!(ds.committed_files.is_empty());
    let err = ds.error.as_deref().expect("expected an error to be set");
    assert!(err.contains("nonexistent"), "error message was: {err}");

    let uncommitted_paths: Vec<&str> = ds
        .uncommitted_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(uncommitted_paths, vec!["c.txt"]);
    assert!(
        ds.display_index_for_path("c.txt").is_some(),
        "display_list should still include the uncommitted file"
    );
}

/// When HEAD equals the merge-base with the configured base (0 commits
/// ahead), the committed section is legitimately empty and there is no
/// error — only uncommitted changes should show up.
#[test]
fn load_diff_head_equals_merge_base_shows_uncommitted_only() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);
    // "release" points at the same commit as HEAD, so
    // merge-base(release, HEAD) == HEAD == release.
    let commit = repo.find_commit(oid).unwrap();
    repo.branch("release", &commit, false).unwrap();

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let mut ds = DiffState::new("release", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "release", false, 4);

    assert!(ds.committed_files.is_empty());
    assert!(ds.error.is_none());

    let uncommitted_paths: Vec<&str> = ds
        .uncommitted_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(uncommitted_paths, vec!["c.txt"]);
}

/// Reproduces the original bug report end to end: a worktree whose base was
/// saved as `origin/main` (no local `main` branch), sitting 1 commit behind
/// `origin/main` with a pile of uncommitted changes. Before the fix, `main`
/// failed to resolve via `find_branch(Local)`, and that failure wiped
/// *both* diff sections — reporting "Changed files (0)" even though the
/// modified/untracked files were all still there.
///
/// The subtlety this test exists to pin down: **committed being 0 here is
/// correct, not a symptom of the bug.** 1-behind means
/// `merge-base(origin/main, HEAD) == HEAD`, so the committed section is
/// legitimately empty regardless of the fix. What the fix changes is that
/// `ds.error` is now `None` (the base resolved) and `uncommitted_files`
/// keeps every modified/untracked file — pre-fix, both would have been
/// cleared alongside the committed section, collapsing everything to a
/// silent (0). Don't read the committed-empty assertion below as "the bug
/// is that committed shows 0"; it never should have shown anything else.
#[test]
fn load_diff_reproduces_the_silent_zero_files_report() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // C0 -> C1 (HEAD, "feature") -> C2 (origin/main only, no local branch).
    let root_oid = commit_files(&repo, None, &[("base.txt", b"base")]);
    let root_commit = repo.find_commit(root_oid).unwrap();

    let head_oid = commit_files(
        &repo,
        Some(&root_commit),
        &[("tracked1.txt", b"orig1"), ("tracked2.txt", b"orig2")],
    );
    checkout_branch(&repo, "feature", head_oid);
    let head_commit = repo.find_commit(head_oid).unwrap();

    let origin_main_oid = commit_files(&repo, Some(&head_commit), &[("future.txt", b"ahead")]);
    set_remote_tracking_ref(&repo, "origin/main", origin_main_oid);

    assert!(
        repo.find_branch("main", git2::BranchType::Local).is_err(),
        "test setup invariant: no local 'main' branch should exist"
    );

    // Modified tracked files + untracked new files, matching the report's
    // shape (there it was 15 modified + 2 untracked; 2 + 2 is enough here).
    std::fs::write(dir.path().join("tracked1.txt"), b"modified1").unwrap();
    std::fs::write(dir.path().join("tracked2.txt"), b"modified2").unwrap();
    std::fs::write(dir.path().join("untracked1.txt"), b"new1").unwrap();
    std::fs::write(dir.path().join("untracked2.txt"), b"new2").unwrap();

    let mut ds = DiffState::new("origin/main", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "origin/main", false, 4);

    // The fix: base resolved, so no error.
    assert_eq!(ds.error, None);
    // Legitimately empty (1-behind), not a bug.
    assert!(ds.committed_files.is_empty());

    let uncommitted_paths: Vec<&str> = ds
        .uncommitted_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(
        uncommitted_paths,
        vec!["tracked1.txt", "tracked2.txt", "untracked1.txt", "untracked2.txt"]
    );
    for path in ["tracked1.txt", "tracked2.txt", "untracked1.txt", "untracked2.txt"] {
        assert!(
            ds.display_index_for_path(path).is_some(),
            "display_list should include '{path}'"
        );
    }
}

// ── Edge cases the plan's NFR checklist calls out but didn't have coverage ──

/// `repo.merge_base()` fails when the base and HEAD share no history (two
/// unrelated root commits — the same shape a shallow clone produces). The
/// plan's "not doing" list promises that this can't take committed down
/// with uncommitted, and that the reason is visible; this pins both.
#[test]
fn load_diff_keeps_uncommitted_when_merge_base_is_unrelated() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // Two root commits with no common ancestor.
    let other_oid = commit_files(&repo, None, &[("other.txt", b"other")]);
    repo.branch("other", &repo.find_commit(other_oid).unwrap(), false)
        .unwrap();

    let head_oid = commit_files(&repo, None, &[("head.txt", b"head")]);
    checkout_branch(&repo, "feature", head_oid);

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let mut ds = DiffState::new("other", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "other", false, 4);

    assert!(ds.committed_files.is_empty());
    let err = ds.error.as_deref().expect("expected an error to be set");
    // Must name both the failure mode and the base, so it reads distinctly
    // from the unresolvable-ref error in
    // `base_ref_unresolvable_reports_error` — otherwise the banner can't tell
    // "no such ref" apart from "no common ancestor".
    assert!(err.contains("merge-base"), "error message was: {err}");
    assert!(err.contains("other"), "error message was: {err}");

    let uncommitted_paths: Vec<&str> = ds
        .uncommitted_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(uncommitted_paths, vec!["c.txt"]);
}

/// A worktree with zero commits (`git init` only — HEAD is "unborn") must
/// not panic, and must surface an error rather than silently reporting zero
/// changes.
///
/// This is an existing limitation, not something this change regresses:
/// every workdir file here is technically untracked, but the uncommitted
/// diff still needs a HEAD tree to compare against and there isn't one. What
/// this change buys is that the reason now lands in `error` instead of an
/// unexplained "Changed files (0)".
#[test]
fn load_diff_on_unborn_head_reports_error_without_panicking() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("untracked.txt"), b"new").unwrap();

    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "main", false, 4);

    let err = ds.error.as_deref().expect("expected an error to be set");
    assert!(err.contains("HEAD"), "error message was: {err}");
    assert!(ds.committed_files.is_empty());
    assert!(ds.uncommitted_files.is_empty());
    assert!(ds.display_list.is_empty());
}

/// `"HEAD"` as a base is a diff-engine no-op: `merge-base(HEAD, HEAD) ==
/// HEAD`, so the committed diff is always empty and never errors. That's
/// correct for the diff engine — `"HEAD"` just isn't a useful base to have.
/// Keeping a worktree from ever *saving* `"HEAD"` as its base is a
/// write-side responsibility (`GitEngine::resolve_base_ref` /
/// `worktree_crud.rs`), not something this function should special-case.
#[test]
fn base_ref_head_yields_an_empty_committed_diff() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    let files =
        DiffState::compute_diff_range(dir.path(), "HEAD", DiffRange::Committed, false, 4).unwrap();
    assert!(files.is_empty());
}

/// A base that resolves to a non-commit object (here, a lightweight tag
/// pointing straight at a blob) must report an error rather than silently
/// returning an empty diff — the two look identical to a caller that only
/// checks `Ok(vec![])`.
#[test]
fn base_ref_pointing_at_a_blob_reports_error() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    let blob_oid = repo.blob(b"not a commit").unwrap();
    let blob_obj = repo.find_object(blob_oid, None).unwrap();
    repo.tag_lightweight("blob-tag", &blob_obj, false).unwrap();

    let err =
        DiffState::compute_diff_range(dir.path(), "blob-tag", DiffRange::Committed, false, 4)
            .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("blob-tag"), "error message was: {msg}");
}

/// `config.general.main_branch = ""` isn't rejected at the TOML layer, so an
/// empty-string base can reach diff computation. Confirmed against git2
/// directly before writing this: `revparse_single("")` errors with
/// `InvalidSpec` rather than resolving to anything, so this is already safe
/// — this test just pins that so a future git2 upgrade can't silently change
/// it into the same "HEAD" trap as
/// `base_ref_head_yields_an_empty_committed_diff`.
#[test]
fn base_ref_empty_string_reports_error() {
    use super::model::DiffRange;
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    let result = DiffState::compute_diff_range(dir.path(), "", DiffRange::Committed, false, 4);
    assert!(result.is_err(), "empty-string base should not resolve");
}
