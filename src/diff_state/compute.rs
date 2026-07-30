//! `git2`/`similar`-based diff computation: loading a `DiffState` from a
//! repository, building per-file hunks/lines, function-context header
//! detection, and case-only rename filtering (for case-insensitive
//! filesystems).

use std::path::Path;

use anyhow::{Context, Result};
use git2::Repository;
use regex::Regex;
use similar::{ChangeTag, TextDiff};

use super::model::{
    DiffHunk, DiffLine, DiffLineTag, DiffRange, DiffState, FileDiff, InlineSegment,
};

/// Resolve `base` — a branch name, a remote-tracking ref, a tag, or a raw OID —
/// to the commit the diff should be based on.
///
/// `revparse_single` rather than `find_branch` because worktrees Conductor
/// creates record `origin/<main>` as their base (see
/// `GitEngine::resolve_base_ref`), and a remote-tracking ref is not a local
/// branch — that mismatch is what made the changed-files list come back empty.
///
/// The name is resolved exactly as recorded, so a base of `origin/main` uses
/// the remote-tracking ref even when a stale local `main` exists. Note the
/// converse: PR intake records the PR's base as a bare name (`main`), and that
/// *does* resolve to the local branch, which may lag the remote. The `origin/`
/// retry is only for names with no local ref at all — a configured base like
/// `develop` that exists solely as `refs/remotes/origin/develop`.
///
/// Ordering follows git's own revspec rules, so a *tag* named `main` wins over
/// a branch named `main`, matching what `git rev-parse main` would pick.
fn resolve_base_commit(repo: &Repository, base: &str) -> Result<git2::Oid> {
    let resolved = repo.revparse_single(base).or_else(|primary| {
        // A base recorded as a bare name (`develop`) can exist only as
        // `refs/remotes/origin/develop`: git's revspec rules stop at
        // `refs/remotes/<name>` and won't fill in the remote for you. Skip the
        // retry for an already-qualified name so the error the user reads never
        // says `origin/origin/main`, and keep the first failure as the cause —
        // that's the one naming the ref they actually configured.
        if base.starts_with("origin/") {
            return Err(primary);
        }
        repo.revparse_single(&format!("origin/{base}"))
            .map_err(|_| primary)
    });
    let obj = resolved.with_context(|| format!("base ref '{base}' cannot be resolved"))?;
    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("cannot peel '{base}' to commit"))?;
    Ok(commit.id())
}

impl DiffState {
    /// Load the diff between `base_branch` and HEAD for the repository at
    /// `worktree_path`, replacing any previously stored diff data.
    ///
    /// Computes both committed (merge-base..HEAD) and uncommitted (HEAD vs
    /// workdir+index) diffs.
    pub fn load_diff(
        &mut self,
        worktree_path: &Path,
        base_branch: &str,
        word_diff: bool,
        tab_width: usize,
    ) {
        self.base_branch = base_branch.to_string();
        self.error = None;

        // Compute committed diff.
        match Self::compute_diff_range(
            worktree_path,
            base_branch,
            DiffRange::Committed,
            word_diff,
            tab_width,
        ) {
            Ok(mut files) => {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                self.committed_files = files;
            }
            Err(e) => {
                self.committed_files.clear();
                self.error = Some(format!("{e:#}"));
                // Fall through to the uncommitted computation below: it
                // doesn't depend on `base_branch`, so a bad base must not
                // hide uncommitted changes too.
            }
        }

        // Compute uncommitted diff.
        match Self::compute_diff_range(
            worktree_path,
            base_branch,
            DiffRange::Uncommitted,
            word_diff,
            tab_width,
        ) {
            Ok(mut files) => {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                self.uncommitted_files = files;
            }
            Err(e) => {
                self.uncommitted_files.clear();
                // Non-fatal: committed diff was loaded successfully.
                log::warn!("failed to compute uncommitted diff: {e:#}");
            }
        }

        self.rebuild_display_list();
        self.scroll = 0;
    }

    /// Return a regex pattern for detecting function/class/struct headers
    /// based on the file extension. Returns `None` for unsupported extensions.
    fn func_pattern_for_ext(ext: &str) -> Option<Regex> {
        let pattern = match ext {
            // Rust
            "rs" => r"^\s*(pub\s+)?(async\s+)?(fn|impl|struct|enum|trait|mod|macro_rules!)\b",
            // TypeScript / JavaScript
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "mts" | "cjs" | "cts" => {
                r"^\s*(export\s+)?(default\s+)?(async\s+)?(function\*?|class)\b|^\s*(export\s+)?(const|let|var)\s+\w+\s*="
            }
            // Python
            "py" => r"^\s*(async\s+)?(def|class)\b",
            // Go
            "go" => r"^(func|type)\b",
            // Java / C# / Kotlin
            "java" | "cs" | "kt" | "kts" => {
                r"^\s*(public|private|protected|internal|static|abstract|override|final|suspend)?\s*(public|private|protected|internal|static|abstract|override|final|suspend)?\s*(class|interface|enum|record|fun|void|int|long|string|bool|boolean|var|val|object)\b"
            }
            // C / C++
            "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => {
                r"^[a-zA-Z_][\w:*&<> ]*\s+\*?\w+\s*\(|^\s*(class|struct|enum|namespace|template)\b"
            }
            // Ruby
            "rb" => r"^\s*(def|class|module)\b",
            // PHP
            "php" => r"^\s*(public|private|protected|static)?\s*(function|class|interface|trait)\b",
            // Shell
            "sh" | "bash" | "zsh" => r"^\s*(\w+\s*\(\)|function\s+\w+)",
            _ => return None,
        };
        Regex::new(pattern).ok()
    }

    /// Scan upward from `start_line` (0-indexed) to find the nearest function
    /// header in the old file content.
    fn find_func_header(old_lines: &[&str], start_line: usize, pattern: &Regex) -> Option<String> {
        for i in (0..=start_line).rev() {
            let line = old_lines[i].trim_end();
            if pattern.is_match(line) {
                // Truncate very long headers for display.
                let trimmed = line.trim();
                let header = if trimmed.len() > 80 {
                    // Find the last char boundary at or before byte 80.
                    let mut end = 80;
                    while !trimmed.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}…", &trimmed[..end])
                } else {
                    trimmed.to_string()
                };
                return Some(header);
            }
        }
        None
    }

    /// Public wrapper for background diff computation.
    ///
    /// `committed`: if true, computes merge-base..HEAD; if false, HEAD vs workdir+index.
    pub fn compute_diff_range_static(
        worktree_path: &Path,
        base_branch: &str,
        committed: bool,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<Vec<FileDiff>> {
        let range = if committed {
            DiffRange::Committed
        } else {
            DiffRange::Uncommitted
        };
        Self::compute_diff_range(worktree_path, base_branch, range, word_diff, tab_width)
    }

    /// Use `git2` + `similar` to compute file-level diffs for a given range.
    pub(super) fn compute_diff_range(
        worktree_path: &Path,
        base_branch: &str,
        range: DiffRange,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<Vec<FileDiff>> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open repo at {}", worktree_path.display()))?;

        // Resolve HEAD.
        let head_commit = repo
            .head()
            .with_context(|| "cannot resolve HEAD")?
            .peel_to_commit()
            .with_context(|| "cannot peel HEAD to commit")?;
        let head_oid = head_commit.id();

        // Build the git2 diff depending on range.
        let diff = match range {
            DiffRange::Committed => {
                // merge-base(base, HEAD)..HEAD
                let base_oid = resolve_base_commit(&repo, base_branch)?;
                let merge_base_oid = repo.merge_base(base_oid, head_oid).with_context(|| {
                    format!("cannot find merge-base between '{base_branch}' and HEAD")
                })?;
                let merge_base_tree = repo
                    .find_commit(merge_base_oid)?
                    .tree()
                    .with_context(|| "cannot get merge-base tree")?;
                let head_tree = head_commit.tree()?;
                repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&head_tree), None)?
            }
            DiffRange::Uncommitted => {
                // HEAD..workdir+index
                let head_tree = head_commit.tree()?;
                let mut opts = git2::DiffOptions::new();
                opts.include_untracked(true);
                opts.recurse_untracked_dirs(true);
                repo.diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))?
            }
        };

        // Determine if we need to read from workdir (for unstaged/untracked files).
        let use_workdir = range == DiffRange::Uncommitted;

        let mut file_diffs = Vec::new();

        // Build a set of delta indices to skip: case-only path differences
        // with identical content.  On case-insensitive filesystems (macOS),
        // git may report a delete + add pair where the paths differ only in
        // case (e.g. "Photo.png" deleted, "photo.png" added) even though
        // the file content is identical.  We detect these pairs by comparing
        // blob OIDs and lowercased paths.
        let skip_indices = Self::find_case_only_rename_indices(&diff);

        let num_deltas = diff.deltas().len();
        for delta_idx in 0..num_deltas {
            if skip_indices.contains(&delta_idx) {
                continue;
            }

            let delta = diff.get_delta(delta_idx).unwrap();

            // Determine file path.
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "(unknown)".to_string());

            // Get old content from the blob.
            let old_content = Self::blob_content(&repo, &delta.old_file());

            // Get new content: for workdir diffs, read from disk when the
            // blob id is zero (unstaged / untracked).
            let new_content = if use_workdir && delta.new_file().id().is_zero() {
                let full_path = worktree_path.join(&path);
                match std::fs::read(&full_path) {
                    Ok(bytes) => String::from_utf8(bytes)
                        .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).to_string()),
                    Err(_) => String::new(),
                }
            } else {
                Self::blob_content(&repo, &delta.new_file())
            };

            // Also skip single-delta case-only renames (when rename detection
            // merges delete+add into one delta).
            if Self::is_case_only_rename(&delta) && old_content == new_content {
                continue;
            }

            // Skip files with no actual content changes.
            // Catches spurious deltas from case-insensitive FS stat mismatches.
            if old_content == new_content {
                continue;
            }

            // Use `similar` to compute line-level diff with context.
            let text_diff = TextDiff::from_lines(&old_content, &new_content);

            // Prepare function context extraction.
            let ext = Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let func_pattern = Self::func_pattern_for_ext(ext);
            let old_lines: Vec<&str> = old_content.lines().collect();

            let context_radius = 3;
            let mut hunks = Vec::new();
            let mut total_added = 0usize;
            let mut total_deleted = 0usize;

            for group in text_diff.grouped_ops(context_radius) {
                let mut hunk_lines = Vec::new();

                for op in &group {
                    if word_diff {
                        for inline_change in text_diff.iter_inline_changes(op) {
                            let tag = match inline_change.tag() {
                                ChangeTag::Equal => DiffLineTag::Equal,
                                ChangeTag::Insert => {
                                    total_added += 1;
                                    DiffLineTag::Insert
                                }
                                ChangeTag::Delete => {
                                    total_deleted += 1;
                                    DiffLineTag::Delete
                                }
                            };

                            let old_line_no = inline_change.old_index().map(|i| i + 1);
                            let new_line_no = inline_change.new_index().map(|i| i + 1);

                            let segments: Vec<InlineSegment> = inline_change
                                .iter_strings_lossy()
                                .map(|(emphasized, value)| InlineSegment {
                                    text: value.into_owned(),
                                    emphasized,
                                })
                                .collect();

                            // Build content by joining segment texts.
                            let content: String = segments
                                .iter()
                                .map(|s| s.text.trim_end_matches('\n').trim_end_matches('\r'))
                                .collect::<Vec<_>>()
                                .join("");
                            let content = Self::expand_tabs(&content, tab_width);

                            let has_emphasis = segments.iter().any(|s| s.emphasized);
                            let inline_segments = if has_emphasis { segments } else { Vec::new() };

                            hunk_lines.push(DiffLine {
                                tag,
                                old_line_no,
                                new_line_no,
                                inline_segments,
                                content,
                            });
                        }
                    } else {
                        for change in text_diff.iter_changes(op) {
                            let tag = match change.tag() {
                                ChangeTag::Equal => DiffLineTag::Equal,
                                ChangeTag::Insert => {
                                    total_added += 1;
                                    DiffLineTag::Insert
                                }
                                ChangeTag::Delete => {
                                    total_deleted += 1;
                                    DiffLineTag::Delete
                                }
                            };

                            let old_line_no = change.old_index().map(|i| i + 1);
                            let new_line_no = change.new_index().map(|i| i + 1);

                            let raw = change.value().trim_end_matches('\n').trim_end_matches('\r');
                            let content = Self::expand_tabs(raw, tab_width);

                            hunk_lines.push(DiffLine {
                                tag,
                                old_line_no,
                                new_line_no,
                                inline_segments: Vec::new(),
                                content,
                            });
                        }
                    }
                }

                // Extract function context header for this hunk.
                let func_header = func_pattern.as_ref().and_then(|pat| {
                    // Find the first line number in the hunk (old side).
                    let first_old_line = hunk_lines.iter().find_map(|l| l.old_line_no);
                    let first_new_line = hunk_lines.iter().find_map(|l| l.new_line_no);
                    let start = first_old_line.or(first_new_line).unwrap_or(1);
                    if start > 0 && !old_lines.is_empty() {
                        let search_from = (start - 1).min(old_lines.len() - 1);
                        Self::find_func_header(&old_lines, search_from, pat)
                    } else {
                        None
                    }
                });

                hunks.push(DiffHunk {
                    lines: hunk_lines,
                    func_header,
                });
            }

            file_diffs.push(FileDiff {
                path,
                added_lines: total_added,
                deleted_lines: total_deleted,
                hunks,
            });
        }

        Ok(file_diffs)
    }

    /// Find delta indices that form case-only rename pairs (delete + add with
    /// paths differing only in case and identical blob content).
    ///
    /// Returns a set of indices to skip during diff processing.
    fn find_case_only_rename_indices(diff: &git2::Diff<'_>) -> std::collections::HashSet<usize> {
        use std::collections::HashMap;

        let mut skip = std::collections::HashSet::new();

        // Collect deleted entries: lowercased path → (index, blob oid).
        let mut deleted: HashMap<String, Vec<(usize, git2::Oid)>> = HashMap::new();
        // Collect added entries: lowercased path → (index, blob oid).
        let mut added: HashMap<String, Vec<(usize, git2::Oid)>> = HashMap::new();

        for (idx, delta) in diff.deltas().enumerate() {
            let status = delta.status();
            match status {
                git2::Delta::Deleted => {
                    if let Some(p) = delta.old_file().path() {
                        let key = p.to_string_lossy().to_lowercase();
                        let oid = delta.old_file().id();
                        deleted.entry(key).or_default().push((idx, oid));
                    }
                }
                git2::Delta::Added | git2::Delta::Untracked => {
                    if let Some(p) = delta.new_file().path() {
                        let key = p.to_string_lossy().to_lowercase();
                        let oid = delta.new_file().id();
                        added.entry(key).or_default().push((idx, oid));
                    }
                }
                _ => {}
            }
        }

        // Match pairs: same lowercased path, same blob OID, different actual path.
        for (lower_path, del_entries) in &deleted {
            if let Some(add_entries) = added.get(lower_path) {
                for &(del_idx, del_oid) in del_entries {
                    for &(add_idx, add_oid) in add_entries {
                        if !del_oid.is_zero() && del_oid == add_oid {
                            // Verify actual paths differ (not the same exact path).
                            let del_delta = diff.get_delta(del_idx).unwrap();
                            let add_delta = diff.get_delta(add_idx).unwrap();
                            let del_path = del_delta.old_file().path().unwrap();
                            let add_path = add_delta.new_file().path().unwrap();
                            if del_path != add_path {
                                skip.insert(del_idx);
                                skip.insert(add_idx);
                            }
                        }
                    }
                }
            }
        }

        skip
    }

    /// Check whether a delta represents a case-only rename, i.e. old_path and
    /// new_path are equal when compared case-insensitively but differ in their
    /// exact bytes.  Returns `false` if either path is absent.
    fn is_case_only_rename(delta: &git2::DiffDelta<'_>) -> bool {
        if let (Some(old_path), Some(new_path)) = (delta.old_file().path(), delta.new_file().path())
        {
            let old_s = old_path.to_string_lossy();
            let new_s = new_path.to_string_lossy();
            old_s != new_s && old_s.eq_ignore_ascii_case(&new_s)
        } else {
            false
        }
    }

    /// Read blob content for a diff file entry, returning an empty string if
    /// the blob is absent (new or deleted file).
    fn blob_content(repo: &Repository, file: &git2::DiffFile<'_>) -> String {
        if file.id().is_zero() {
            return String::new();
        }
        match repo.find_blob(file.id()) {
            Ok(blob) => {
                // Attempt UTF-8; fall back to lossy conversion.
                String::from_utf8(blob.content().to_vec())
                    .unwrap_or_else(|_| String::from_utf8_lossy(blob.content()).to_string())
            }
            Err(_) => String::new(),
        }
    }
}
