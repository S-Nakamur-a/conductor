//! File tree types — `FileTreeEntry` and `ScoredFile`.

/// A file matched by filename fuzzy search, with its score.
#[derive(Debug, Clone)]
pub struct ScoredFile {
    /// Relative path of the file.
    pub path: String,
    /// Fuzzy match score (higher = better).
    pub score: i32,
}

/// A single entry in the flattened file tree.
#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    /// Path relative to the worktree root (e.g. `"src/main.rs"`).
    pub path: String,
    /// Display name — the final component of the path.
    pub name: String,
    /// Nesting depth (0 for top-level entries).
    pub depth: usize,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Whether a directory entry is currently expanded (ignored for files).
    pub is_expanded: bool,
    /// Whether this directory's children have been loaded into the tree.
    /// Always `false` for files. Directories start as `false` and are set to
    /// `true` after their children are read from the filesystem.
    pub children_loaded: bool,
}
