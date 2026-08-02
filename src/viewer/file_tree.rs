//! ファイルツリーの型定義 — FileTreeEntry と ScoredFile。

use crate::git_engine::status_map::TreeGitState;

/// ファイル名のあいまい検索でマッチしたファイルと、そのスコア。
#[derive(Debug, Clone)]
pub struct ScoredFile {
    /// ファイルの相対パス。
    pub path: String,
    /// あいまい検索のスコア（高いほどマッチ度が高い）。
    pub score: i32,
}

/// フラット化されたファイルツリー中の1エントリ。
#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    /// worktree ルートからの相対パス（例: "src/main.rs"）。
    pub path: String,
    /// 表示名 — パスの最後の要素。
    pub name: String,
    /// ネストの深さ（トップレベルのエントリは0）。
    pub depth: usize,
    /// このエントリがディレクトリかどうか。
    pub is_dir: bool,
    /// ディレクトリエントリが現在展開されているかどうか（ファイルでは無視される）。
    pub is_expanded: bool,
    /// このディレクトリの子要素がツリーに読み込み済みかどうか。
    /// ファイルでは常に false。ディレクトリは false から始まり、ファイルシステムから
    /// 子要素を読み込んだ後に true になる。
    pub children_loaded: bool,
    /// このエントリのアイコン文字列のキャッシュ（生成時に一度だけ計算する）。
    pub icon: &'static str,
    /// tracked/untracked/ignored の別。ツリーを（再）構築した時点の git status
    /// スナップショットに基づく — Explorer の減光表示に使う。
    pub git_state: TreeGitState,
}

/// ファイルの拡張子や名前から絵文字アイコンを返す。
pub fn file_icon(name: &str) -> &'static str {
    // 特別扱いするファイル名を先に判定する。
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

    // 拡張子で判定する。
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
