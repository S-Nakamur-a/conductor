//! ファイルツリーの型定義 — FileTreeEntry と ScoredFile、およびファイルアイコン。

use ratatui::style::Color;

use crate::config::IconSet;
use crate::git_engine::status_map::TreeGitState;
use crate::theme::Theme;

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
    /// このエントリのアイコン（生成時に一度だけ計算する）。字形の選択は描画時まで
    /// 遅延するので、これは文字セットに依存しない。
    pub icon: FileIcon,
    /// tracked/untracked/ignored の別。ツリーを（再）構築した時点の git status
    /// スナップショットに基づく — Explorer の減光表示に使う。
    pub git_state: TreeGitState,
}

/// アイコンの色を決める粗い種別。
///
/// テーマは11個あるので、ファイル種別ごとの色を新設すると全テーマに手が入る。
/// 既存の意味色へ寄せることでテーマ側の変更をなくしている。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconRole {
    /// プログラミング言語のソース。
    Code,
    /// マークアップとスタイルシート。
    Markup,
    /// 構造化データと設定ファイル。
    Data,
    /// 人間が読む文書。
    Doc,
    /// 画像・音声・動画・書庫などのバイナリ。
    Media,
    /// 鍵や lock ファイルなど、取り扱いに注意が要るもの。
    Sensitive,
    /// ディレクトリ。
    Dir,
}

impl IconRole {
    /// この種別のアイコン色。
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            IconRole::Code => theme.accent,
            IconRole::Markup => theme.success,
            IconRole::Data => theme.warning,
            IconRole::Doc => theme.info,
            IconRole::Media => theme.hint,
            IconRole::Sensitive => theme.error,
            IconRole::Dir => theme.info,
        }
    }
}

/// 1つのファイル種別に対応するアイコン。
///
/// 字形を文字セットごとに持ち、どちらを描くかは描画時に決める。ツリーの構築時点で
/// 確定させないのは、[FileTreeEntry] がツリーの再構築まで保持されるため。
#[derive(Debug, Clone, Copy)]
pub struct FileIcon {
    nerd: &'static str,
    unicode: &'static str,
    /// アイコンの色を決める種別。
    pub role: IconRole,
}

impl FileIcon {
    const fn new(nerd: &'static str, unicode: &'static str, role: IconRole) -> Self {
        Self {
            nerd,
            unicode,
            role,
        }
    }

    /// 指定された文字セットでの字形。常に1カラム幅である。
    pub fn glyph(&self, set: IconSet) -> &'static str {
        match set {
            IconSet::Nerd => self.nerd,
            IconSet::Unicode => self.unicode,
        }
    }
}

// 字形は2種類ある。
//
// nerd 側は Nerd Font の私用領域 (U+E000-F8FF) にあるグリフで、コードポイントは
// 公式の glyphnames.json (v3.5.1) から取っている。Plane 15 以降にある md-* を
// 使っていないのは、端末による幅の扱いが読めないためである。
//
// unicode 側は Nerd Font が無い環境向けで、East Asian Width が Neutral かつ
// Emoji プロパティを持たない文字だけを選んでいる。ここに絵文字を使えない理由は
// 2つあり、どちらも致命的である。1つはカラー絵文字がフォント側の固定色で描かれる
// ためテーマの色も git 状態の減光も一切効かないこと。もう1つは幅2で、しかも端末
// ごとに解釈が割れて後続の列がずれること (ui::reflow_view::glyphs に同じ問題の
// 記録がある)。

const DIR: FileIcon = FileIcon::new("\u{e613}", "\u{22a1}", IconRole::Dir);
const DIR_OPEN: FileIcon = FileIcon::new("\u{e5fe}", "\u{22a1}", IconRole::Dir);

const CODE: FileIcon = FileIcon::new("\u{e64e}", "\u{25c9}", IconRole::Code);
const MARKUP: FileIcon = FileIcon::new("\u{e64e}", "\u{25cc}", IconRole::Markup);
const DATA: FileIcon = FileIcon::new("\u{e64e}", "\u{229e}", IconRole::Data);
const DOC: FileIcon = FileIcon::new("\u{e64e}", "\u{25e6}", IconRole::Doc);
const MEDIA: FileIcon = FileIcon::new("\u{e64e}", "\u{2b25}", IconRole::Media);
const SENSITIVE: FileIcon = FileIcon::new("\u{e64e}", "\u{229f}", IconRole::Sensitive);

/// 種別ごとの既定の字形と色を引き継ぎ、Nerd Font 側だけ固有のグリフに差し替える。
const fn nerd(glyph: &'static str, base: FileIcon) -> FileIcon {
    FileIcon::new(glyph, base.unicode, base.role)
}

/// ディレクトリのアイコン。開いているかどうかで字形が変わる。
pub fn dir_icon(is_expanded: bool) -> FileIcon {
    if is_expanded { DIR_OPEN } else { DIR }
}

/// リストの展開/折りたたみマーカー。末尾のスペースは含まない。
///
/// Nerd Font 側は Codicons の chevron で、VSCode のエクスプローラが使っている
/// ものと同じ字形である。フォールバックは幅1を保証できる範囲で chevron に
/// 最も近い形を選んでいる — 塗りつぶしの三角 (U+25B6/25BC) は Emoji プロパティを
/// 持ち、端末によっては幅2で描かれる。
pub fn expand_arrow(is_expanded: bool, set: IconSet) -> &'static str {
    match (set, is_expanded) {
        (IconSet::Nerd, true) => "\u{eab4}",
        (IconSet::Nerd, false) => "\u{eab6}",
        (IconSet::Unicode, true) => "\u{2304}",
        (IconSet::Unicode, false) => "\u{203a}",
    }
}

/// ファイルの拡張子や名前からアイコンを返す。
pub fn file_icon(name: &str) -> FileIcon {
    let lower = name.to_ascii_lowercase();
    let special = match lower.as_str() {
        "cargo.toml" | "cargo.lock" => Some(nerd("\u{e68b}", CODE)),
        "package.json" | "package-lock.json" => Some(nerd("\u{e616}", DATA)),
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => Some(nerd("\u{e650}", DATA)),
        "makefile" | "cmake" | "cmakelists.txt" => Some(nerd("\u{e673}", DATA)),
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some(nerd("\u{e65d}", SENSITIVE)),
        "license" | "license.md" | "license.txt" => Some(nerd("\u{e60a}", DOC)),
        "readme.md" | "readme" | "readme.txt" => Some(nerd("\u{f02d}", DOC)),
        _ => None,
    };
    if let Some(icon) = special {
        return icon;
    }

    match name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("rs") => nerd("\u{e68b}", CODE),
        Some("py") => nerd("\u{e606}", CODE),
        Some("js") | Some("mjs") | Some("cjs") => nerd("\u{e60c}", CODE),
        Some("ts") | Some("mts") | Some("cts") => nerd("\u{e628}", CODE),
        Some("jsx") | Some("tsx") => nerd("\u{e625}", CODE),
        Some("go") => nerd("\u{e65e}", CODE),
        Some("rb") => nerd("\u{e605}", CODE),
        Some("java" | "class" | "jar") => nerd("\u{e66d}", CODE),
        Some("c" | "h") => nerd("\u{e649}", CODE),
        Some("cpp" | "cc" | "cxx" | "hpp") => nerd("\u{e646}", CODE),
        Some("cs") => nerd("\u{e648}", CODE),
        Some("swift") => nerd("\u{e699}", CODE),
        Some("kt" | "kts") => nerd("\u{e634}", CODE),
        Some("php") => nerd("\u{e608}", CODE),
        Some("lua") => nerd("\u{e620}", CODE),
        Some("sh" | "bash" | "zsh" | "fish") => nerd("\u{e691}", CODE),
        Some("html" | "htm") => nerd("\u{e60e}", MARKUP),
        Some("css" | "scss" | "sass" | "less") => nerd("\u{e614}", MARKUP),
        Some("json" | "jsonc" | "json5") => nerd("\u{e60b}", DATA),
        Some("yaml" | "yml") => nerd("\u{e6a8}", DATA),
        Some("toml") => nerd("\u{e615}", DATA),
        Some("xml" | "xsl") => nerd("\u{e619}", MARKUP),
        Some("md" | "mdx") => nerd("\u{e609}", DOC),
        Some("txt" | "text") => nerd("\u{e64e}", DOC),
        Some("sql") => nerd("\u{e64d}", DATA),
        Some("graphql" | "gql") => nerd("\u{e662}", CODE),
        Some("proto") => nerd("\u{e64e}", DATA),
        Some("png" | "jpg" | "jpeg" | "gif" | "ico" | "webp" | "bmp") => nerd("\u{e60d}", MEDIA),
        Some("svg") => nerd("\u{e698}", MARKUP),
        Some("mp4" | "mov" | "avi" | "webm") => nerd("\u{e69f}", MEDIA),
        Some("mp3" | "wav" | "ogg" | "flac") => nerd("\u{e638}", MEDIA),
        Some("zip" | "tar" | "gz" | "bz2" | "xz" | "rar" | "7z") => nerd("\u{e6aa}", MEDIA),
        Some("pdf") => nerd("\u{e67d}", DOC),
        Some("lock") => nerd("\u{e672}", SENSITIVE),
        Some("env") => nerd("\u{f084}", SENSITIVE),
        Some("log") => nerd("\u{f4ed}", DOC),
        Some("wasm") => nerd("\u{e6a1}", MEDIA),
        Some("test" | "spec") => nerd("\u{f0c3}", CODE),
        _ => nerd("\u{e64e}", DOC),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// アイコンの字形を一通り集める。網羅ではなく、各分岐に1つ以上当てるためのもの。
    fn sample_icons() -> Vec<FileIcon> {
        let names = [
            "main.rs",
            "app.py",
            "index.js",
            "types.ts",
            "App.tsx",
            "main.go",
            "app.rb",
            "Main.java",
            "core.c",
            "core.cpp",
            "Program.cs",
            "App.swift",
            "Main.kt",
            "index.php",
            "init.lua",
            "run.sh",
            "index.html",
            "style.css",
            "data.json",
            "conf.yaml",
            "Cargo.toml",
            "pom.xml",
            "README.md",
            "notes.txt",
            "schema.sql",
            "api.graphql",
            "msg.proto",
            "logo.png",
            "logo.svg",
            "clip.mp4",
            "song.mp3",
            "dist.zip",
            "paper.pdf",
            "yarn.lock",
            ".env",
            "run.log",
            "mod.wasm",
            "foo.test",
            "unknown.xyzzy",
            "package.json",
            "Dockerfile",
            "Makefile",
            ".gitignore",
            "LICENSE",
        ];
        let mut icons: Vec<FileIcon> = names.iter().map(|n| file_icon(n)).collect();
        icons.push(dir_icon(true));
        icons.push(dir_icon(false));
        icons
    }

    /// 全アイコンがどちらの文字セットでも1カラム幅であること。ここが崩れると
    /// アイコンより右の内容が1列ずれ、行がパネル端からはみ出す。
    #[test]
    fn glyphs_are_single_column() {
        for icon in sample_icons() {
            for set in [IconSet::Nerd, IconSet::Unicode] {
                let glyph = icon.glyph(set);
                assert_eq!(
                    glyph.width(),
                    1,
                    "{glyph:?} ({set:?}) は {} カラム",
                    glyph.width()
                );
            }
        }
    }

    /// フォールバック側が Nerd Font の私用領域に入り込んでいないこと。
    /// 入っていると Nerd Font の無い端末で tofu になる。
    #[test]
    fn unicode_fallback_avoids_private_use_area() {
        for icon in sample_icons() {
            let ch = icon
                .glyph(IconSet::Unicode)
                .chars()
                .next()
                .expect("字形が空");
            assert!(
                !('\u{e000}'..='\u{f8ff}').contains(&ch),
                "U+{:04X} は私用領域にある",
                ch as u32
            );
        }
    }

    /// 展開マーカーもアイコンと同じく、どちらの文字セットでも幅1であること。
    #[test]
    fn expand_arrows_are_single_column() {
        for set in [IconSet::Nerd, IconSet::Unicode] {
            for expanded in [true, false] {
                let arrow = expand_arrow(expanded, set);
                assert_eq!(
                    arrow.width(),
                    1,
                    "{arrow:?} ({set:?}) は {} カラム",
                    arrow.width()
                );
            }
        }
    }

    /// Nerd Font 側が BMP の私用領域に収まっていること。Plane 15 以降の
    /// コードポイントは端末による幅の扱いが読めないので使わない。
    #[test]
    fn nerd_glyphs_stay_in_bmp_private_use_area() {
        for icon in sample_icons() {
            let ch = icon.glyph(IconSet::Nerd).chars().next().expect("字形が空");
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&ch),
                "U+{:04X} は BMP 私用領域の外にある",
                ch as u32
            );
        }
    }
}
