//! The canonical spelling of the repo-relative paths that review comments and
//! walkthrough steps are keyed by.
//!
//! These paths are matched against `FileDiff::path`, which comes straight from
//! `git2` and is therefore always plain (`src/foo.rs` — no `./`, no doubled
//! separators, no trailing slash). Anything keyed by a *different* spelling of
//! the same file silently fails to resolve: a walkthrough step reports "not in
//! this diff" while the file sits right there in the list.
//!
//! Both sides go through [`normalize`]: `mcp-serve`'s tools normalize before
//! writing, and the store normalizes again when reading rows back, so rows
//! written before this existed resolve without a migration.

/// Rewrite a repo-relative path into the spelling git uses.
///
/// Drops surrounding whitespace, `.` segments (including a leading `./`),
/// empty segments (doubled slashes), and a trailing slash.
///
/// This is a pure spelling fix, not a validator — two things it deliberately
/// leaves alone, so that whoever *does* validate still sees them:
///
/// - `..` segments are preserved. Resolving them here would turn a path that
///   must be refused (`mcp_serve::reply::ensure_repo_relative`) into an
///   innocuous-looking one.
/// - A leading `/` is preserved, so an absolute path stays absolute and is
///   still caught by that same check rather than being quietly demoted to a
///   relative path pointing somewhere else entirely.
pub fn normalize(path: &str) -> String {
    let trimmed = path.trim();
    let mut out = String::with_capacity(trimmed.len());
    if trimmed.starts_with('/') {
        out.push('/');
    }
    let mut first = true;
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if !first {
            out.push('/');
        }
        out.push_str(segment);
        first = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_are_left_alone() {
        assert_eq!(normalize("src/foo.rs"), "src/foo.rs");
        assert_eq!(normalize("Cargo.toml"), "Cargo.toml");
        assert_eq!(normalize("docs/設計 メモ.md"), "docs/設計 メモ.md");
    }

    #[test]
    fn dot_slash_doubled_slash_and_trailing_slash_are_dropped() {
        assert_eq!(normalize("./src/foo.rs"), "src/foo.rs");
        assert_eq!(normalize("././src//foo.rs"), "src/foo.rs");
        assert_eq!(normalize("src/foo.rs/"), "src/foo.rs");
        assert_eq!(normalize("  src/foo.rs  "), "src/foo.rs");
        assert_eq!(normalize("./"), "");
    }

    /// `..` must survive normalization: the validation that rejects it runs on
    /// the normalized form, so resolving it here would launder an escaping
    /// path into an acceptable one.
    #[test]
    fn parent_dir_segments_survive() {
        assert_eq!(normalize("../secret"), "../secret");
        assert_eq!(normalize("a/../../b"), "a/../../b");
        assert_eq!(normalize("./../secret"), "../secret");
    }

    /// Likewise an absolute path stays absolute, so the caller that refuses
    /// absolute paths still gets to refuse it.
    #[test]
    fn absolute_paths_stay_absolute() {
        assert_eq!(normalize("/etc/passwd"), "/etc/passwd");
        assert_eq!(normalize("/etc//passwd/"), "/etc/passwd");
    }
}
