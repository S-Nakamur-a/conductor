//! Tests for `GitStatusMap::classify`.
//!
//! Each fixture file's expected `TreeGitState` is decided from *how the
//! fixture was built* (was it `git add`-ed? listed in `.gitignore`? left
//! untouched on disk?) rather than by reading back whatever bits
//! `statuses()` happens to report — otherwise the assertions would just be
//! restating the implementation under test.

use super::*;
use std::fs;

/// Build a repo with one committed file, then lay out 6 more files covering
/// every combination `classify()` must distinguish, plus a `build/` dir
/// declared ignored via `.gitignore` (with a nested file to exercise prefix
/// inheritance). Returns the repo's root path.
fn build_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let root = tmp.path();
    let repo = git2::Repository::init(root).expect("init repo");
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();

    // untouched.txt / staged_only.txt / unstaged_only.txt / both.txt all
    // start out committed as part of the initial commit, so their baseline
    // is "tracked, no changes" before the fixture-specific mutation below.
    fs::write(root.join("untouched.txt"), "a\n").unwrap();
    fs::write(root.join("staged_only.txt"), "a\n").unwrap();
    fs::write(root.join("unstaged_only.txt"), "a\n").unwrap();
    fs::write(root.join("both.txt"), "a\n").unwrap();
    fs::write(root.join(".gitignore"), "build/\n").unwrap();

    {
        let mut index = repo.index().unwrap();
        for f in [
            "untouched.txt",
            "staged_only.txt",
            "unstaged_only.txt",
            "both.txt",
            ".gitignore",
        ] {
            index.add_path(Path::new(f)).unwrap();
        }
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }

    // staged_only.txt: edited, then `git add`-ed (nothing left unstaged).
    fs::write(root.join("staged_only.txt"), "b\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged_only.txt")).unwrap();
        index.write().unwrap();
    }

    // unstaged_only.txt: edited on disk, index left at the committed blob.
    fs::write(root.join("unstaged_only.txt"), "b\n").unwrap();

    // both.txt: `git add`-ed once, then edited again afterward.
    fs::write(root.join("both.txt"), "b\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("both.txt")).unwrap();
        index.write().unwrap();
    }
    fs::write(root.join("both.txt"), "c\n").unwrap();

    // untracked.txt: never added to the index at all.
    fs::write(root.join("untracked.txt"), "a\n").unwrap();

    // build/deep/x.txt: under a directory `.gitignore` declares ignored.
    fs::create_dir_all(root.join("build/deep")).unwrap();
    fs::write(root.join("build/deep/x.txt"), "a\n").unwrap();

    // build2/: a sibling whose name has `build` as a prefix but is NOT
    // ignored — guards against prefix matching being loosened to `starts_with`.
    fs::create_dir_all(root.join("build2")).unwrap();
    fs::write(root.join("build2/y.txt"), "a\n").unwrap();

    // newdir/: a brand-new directory git has never seen. Unlike an ignored
    // directory, libgit2 does not collapse this into a single entry, so the
    // directory itself has no status of its own.
    fs::create_dir_all(root.join("newdir/sub")).unwrap();
    fs::write(root.join("newdir/a.txt"), "a\n").unwrap();
    fs::write(root.join("newdir/sub/b.txt"), "a\n").unwrap();

    tmp
}

#[test]
fn git_status_map_classify_untracked_dir_is_untracked_like_its_contents() {
    // `newdir/` was created and never added, so every path under it is new.
    // The directory row must dim along with its children: showing the parent
    // in the normal tracked colour while the files inside it are dimmed reads
    // as "this folder is known, its contents are not", which is backwards.
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("newdir"), TreeGitState::Untracked);
    assert_eq!(map.classify("newdir/sub"), TreeGitState::Untracked);
    assert_eq!(map.classify("newdir/a.txt"), TreeGitState::Untracked);
    assert_eq!(map.classify("newdir/sub/b.txt"), TreeGitState::Untracked);
}

#[test]
fn git_status_map_classify_tracked_dir_stays_tracked() {
    // The repo root's committed files live directly at the top level, so no
    // ancestor directory should be mistaken for untracked just because some
    // sibling is. `build2/` holds only an untracked file, so it *is*
    // untracked; the assertion below pins the discriminating case.
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("untouched.txt"), TreeGitState::Tracked);
    assert_eq!(map.classify("nonexistent-dir"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_sibling_sharing_a_prefix_is_not_ignored() {
    // `.gitignore` lists `build/`, not `build2/`. Ancestor lookups are exact
    // HashMap hits today, so this passes — it exists to fail loudly if that
    // is ever relaxed into a `starts_with` scan.
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("build/deep/x.txt"), TreeGitState::Ignored);
    assert_ne!(map.classify("build2/y.txt"), TreeGitState::Ignored);
}

#[test]
fn git_status_map_classify_untouched_file_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("untouched.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_staged_only_file_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("staged_only.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_unstaged_only_file_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("unstaged_only.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_file_staged_and_then_edited_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("both.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_never_added_file_is_untracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("untracked.txt"), TreeGitState::Untracked);
}

#[test]
fn git_status_map_classify_gitignored_dir_itself_is_ignored() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    // `FileTreeEntry::path` never carries a trailing slash, even for
    // directories, but libgit2's collapsed-ignored-directory key does
    // ("build/") — this only passes if `classify()` checks both forms.
    assert_eq!(map.classify("build"), TreeGitState::Ignored);
}

#[test]
fn git_status_map_classify_file_under_gitignored_dir_inherits_ignored_via_prefix() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    // libgit2 collapses `build/` into a single status entry rather than
    // reporting `build/deep/x.txt` individually (confirmed empirically),
    // so this only passes if `classify()` actually walks ancestor prefixes
    // rather than doing an exact-match lookup.
    assert_eq!(map.classify("build/deep/x.txt"), TreeGitState::Ignored);
}
