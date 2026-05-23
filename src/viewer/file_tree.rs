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
    /// Cached icon string for this entry (computed once at creation time).
    pub icon: &'static str,
}

/// Return an emoji icon for a file based on its extension or name.
pub fn file_icon(name: &str) -> &'static str {
    // Special filenames first.
    let lower = name.to_ascii_lowercase();
    let special = match lower.as_str() {
        "cargo.toml" | "cargo.lock" => Some("🦀"),
        "package.json" | "package-lock.json" => Some("📦"),
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => Some("🐳"),
        "makefile" | "cmake" | "cmakelists.txt" => Some("🔧"),
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some("🔀"),
        "license" | "license.md" | "license.txt" => Some("📜"),
        "readme.md" | "readme" | "readme.txt" => Some("📖"),
        _ => None,
    };
    if let Some(icon) = special {
        return icon;
    }

    // By extension.
    match name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("rs") => "🦀",
        Some("py") => "🐍",
        Some("js") | Some("mjs") | Some("cjs") => "🟨",
        Some("ts") | Some("mts") | Some("cts") => "🔷",
        Some("jsx") | Some("tsx") => "⚛\u{fe0f}",
        Some("go") => "🐹",
        Some("rb") => "💎",
        Some("java" | "class" | "jar") => "☕",
        Some("c" | "h") => "🇨",
        Some("cpp" | "cc" | "cxx" | "hpp") => "⚙\u{fe0f}",
        Some("cs") => "🟪",
        Some("swift") => "🐦",
        Some("kt" | "kts") => "🟣",
        Some("php") => "🐘",
        Some("lua") => "🌙",
        Some("sh" | "bash" | "zsh" | "fish") => "🐚",
        Some("html" | "htm") => "🌐",
        Some("css" | "scss" | "sass" | "less") => "🎨",
        Some("json" | "jsonc" | "json5") => "📋",
        Some("yaml" | "yml") => "📄",
        Some("toml") => "⚙\u{fe0f}",
        Some("xml" | "xsl") => "📰",
        Some("md" | "mdx") => "📝",
        Some("txt" | "text") => "📃",
        Some("sql") => "🗄\u{fe0f}",
        Some("graphql" | "gql") => "🔮",
        Some("proto") => "📡",
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" | "bmp") => "🖼\u{fe0f}",
        Some("mp4" | "mov" | "avi" | "webm") => "🎬",
        Some("mp3" | "wav" | "ogg" | "flac") => "🎵",
        Some("zip" | "tar" | "gz" | "bz2" | "xz" | "rar" | "7z") => "📦",
        Some("pdf") => "📕",
        Some("lock") => "🔒",
        Some("env") => "🔐",
        Some("log") => "📜",
        Some("wasm") => "🟦",
        Some("d.ts") => "🔷",
        Some("test" | "spec") => "🧪",
        _ => "📄",
    }
}
