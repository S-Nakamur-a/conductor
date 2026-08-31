//! UI 全体のアイコンの一元定義。
//!
//! 字形は 2 種類持つ。nerd 側は Nerd Font の私用領域 (U+E000-F8FF) のグリフで、字形は
//! おおむね Codicons から選んでいる。Plane 15 以降の md-* を使わないのは端末による幅の
//! 扱いが読めないため。
//!
//! unicode 側は Nerd Font が無い環境向けで、East Asian Width が Neutral かつ Emoji
//! プロパティを持たない文字だけを選ぶ。絵文字が使えないのは、カラー絵文字がフォント
//! 側の固定色で描かれてテーマも git 状態の減光も効かないことと、幅 2 の解釈が端末ごと
//! に割れて後続の列がずれるため。
//!
//! 字形が 1 カラムであることは [tests] が全定数について機械的に検証している。

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

/// アイコンに使う文字セット。
///
/// Nerd Font が入っているかは端末に問い合わせられない。字形を描いて幅を測る手も、幅を
/// 決めているのが端末の幅テーブルなので判別できない。判るのは「その端末が Nerd Font の
/// シンボルを同梱しているか」だけで、そこから決めるのが term_caps::detect_icon_set。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconSet {
    /// Nerd Font の私用領域のグリフ。
    Nerd,
    /// Nerd Font を必要としない汎用の記号。
    Unicode,
}

/// 1つのアイコンの字形。文字セットごとに1つずつ持つ。
///
/// どちらを描くかは描画時に決める。生成時に確定させないのは、ファイルツリーの
/// エントリがアイコンを抱えたまま再構築まで生き残るためである。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    nerd: &'static str,
    unicode: &'static str,
}

impl Glyph {
    const fn new(nerd: &'static str, unicode: &'static str) -> Self {
        Self { nerd, unicode }
    }

    /// Nerd Font でしか描かないアイコン。装飾であって情報ではないものに使う —
    /// フォールバック側では [Glyph::get] が空文字を返すので、呼び出し側が
    /// アイコンごと省いてテキストだけを描ける。
    const fn nerd_only(nerd: &'static str) -> Self {
        Self { nerd, unicode: "" }
    }

    /// 指定された文字セットでの字形。空文字はこの文字セットでは描かないことを表す。
    pub fn get(&self, set: IconSet) -> &'static str {
        match set {
            IconSet::Nerd => self.nerd,
            IconSet::Unicode => self.unicode,
        }
    }

    /// 字形と、その後ろの区切りスペース。空文字のときは何も返さないので、
    /// フォールバック時に余分な空白が残らない。
    pub fn labeled(&self, set: IconSet) -> String {
        let g = self.get(set);
        if g.is_empty() {
            String::new()
        } else {
            format!("{g} ")
        }
    }
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
    ///
    /// theme.accent は使わない。多くのテーマで accent は border_focused や selected_bg と同じ
    /// 値なので、静的な属性に割り当てるとフォーカス中のパネルでアイコンが枠線に溶ける。
    ///
    /// ソースコードが本文色なのは数が最も多いから。字形が既に種別を示しているので、多数派を
    /// 落ち着かせて残りを色で立たせるほうが読みやすい。
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            IconRole::Code => theme.fg,
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
#[derive(Debug, Clone, Copy)]
pub struct FileIcon {
    glyph: Glyph,
    /// アイコンの色を決める種別。
    pub role: IconRole,
}

impl FileIcon {
    const fn new(nerd: &'static str, unicode: &'static str, role: IconRole) -> Self {
        Self {
            glyph: Glyph::new(nerd, unicode),
            role,
        }
    }

    /// 指定された文字セットでの字形。常に1カラム幅である。
    pub fn glyph(&self, set: IconSet) -> &'static str {
        self.glyph.get(set)
    }
}

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
    FileIcon::new(glyph, base.glyph.unicode, base.role)
}

/// ディレクトリのアイコン。開いているかどうかで字形が変わる。
pub fn dir_icon(is_expanded: bool) -> FileIcon {
    if is_expanded { DIR_OPEN } else { DIR }
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

/// リストの展開/折りたたみマーカー。末尾のスペースは含まない。
///
/// Nerd Font 側は Codicons の chevron。フォールバックは幅 1 を保証できる範囲で最も近い形
/// — 塗りつぶしの三角 (U+25B6/25BC) は Emoji プロパティを持ち、端末によっては幅 2 になる。
pub fn expand_arrow(is_expanded: bool, set: IconSet) -> &'static str {
    match (set, is_expanded) {
        (IconSet::Nerd, true) => "\u{eab4}",
        (IconSet::Nerd, false) => "\u{eab6}",
        (IconSet::Unicode, true) => "\u{2304}",
        (IconSet::Unicode, false) => "\u{203a}",
    }
}

/// コメント範囲の終端に置くマーカー。クリックするとスレッドが開く。
pub const COMMENT: Glyph = Glyph::new("\u{ea6b}", "\u{275d}");

/// コメント範囲のうち終端より前の行を繋ぐ罫線。
pub const COMMENT_SPAN: Glyph = Glyph::new("\u{2502}", "\u{2502}");

/// コメントを書き始めるボタン。ガターに hover している間だけ出る。
///
/// 塗り円に白抜きの + (Font Awesome) を使っているのは、背景色を敷いた文字の "+"
/// よりボタンとして浮いて見えるためである。
pub const ADD_COMMENT: Glyph = Glyph::new("\u{f055}", "\u{229e}");

/// 実行可能なテスト行のボタン。押すとテストコマンドを Shell の PTY へ送る。
pub const RUN_TEST: Glyph = Glyph::new("\u{eb2c}", "\u{25b8}");

/// 選択範囲の終端に置く折り返し罫線。折りたたみの hover 表示と同じ語彙。
pub const RANGE_END: Glyph = Glyph::new("\u{2570}", "\u{2570}");

/// 提案・所感のコメント。
pub const KIND_SUGGEST: Glyph = Glyph::new("\u{ea61}", "!");

/// 人に答えてほしい問いのコメント。
pub const KIND_QUESTION: Glyph = Glyph::new("\u{eb32}", "?");

/// ファイルツリー。
pub const PANEL_EXPLORER: Glyph = Glyph::nerd_only("\u{ea83}");

/// 変更ファイル一覧。
pub const PANEL_CHANGED: Glyph = Glyph::nerd_only("\u{eae1}");

/// レビューコメント一覧。
pub const PANEL_COMMENTS: Glyph = Glyph::nerd_only("\u{eac7}");

/// PTY のパネル。
pub const PANEL_TERMINAL: Glyph = Glyph::nerd_only("\u{ea85}");

/// revidere のレビュービュー。
pub const PANEL_REVIEW: Glyph = Glyph::nerd_only("\u{eab3}");

/// 別プロセスに grab されていて操作できないパネル。
pub const LOCKED: Glyph = Glyph::new("\u{ea75}", "\u{22a0}");

/// リポジトリ操作。
pub const MENU_REPO: Glyph = Glyph::nerd_only("\u{ea62}");

/// worktree とブランチ。
pub const MENU_WORKTREE: Glyph = Glyph::nerd_only("\u{ec6f}");

/// レビュー。
pub const MENU_REVIEW: Glyph = Glyph::nerd_only("\u{eac7}");

/// 表示の切り替え。
pub const MENU_VIEW: Glyph = Glyph::nerd_only("\u{ea70}");

/// パネルのレイアウト。
pub const MENU_PANEL: Glyph = Glyph::nerd_only("\u{ebeb}");

/// 検索。
pub const MENU_SEARCH: Glyph = Glyph::nerd_only("\u{ea6d}");

/// ターミナル。
pub const MENU_TERMINAL: Glyph = Glyph::nerd_only("\u{ea85}");

/// ヘルプ。
pub const MENU_HELP: Glyph = Glyph::nerd_only("\u{eb32}");

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// このモジュールが公開している Glyph 定数すべて。追加したらここにも足す。
    fn all_glyphs() -> Vec<(&'static str, Glyph)> {
        vec![
            ("COMMENT", COMMENT),
            ("COMMENT_SPAN", COMMENT_SPAN),
            ("ADD_COMMENT", ADD_COMMENT),
            ("RUN_TEST", RUN_TEST),
            ("RANGE_END", RANGE_END),
            ("KIND_SUGGEST", KIND_SUGGEST),
            ("KIND_QUESTION", KIND_QUESTION),
            ("PANEL_EXPLORER", PANEL_EXPLORER),
            ("PANEL_CHANGED", PANEL_CHANGED),
            ("PANEL_COMMENTS", PANEL_COMMENTS),
            ("PANEL_TERMINAL", PANEL_TERMINAL),
            ("PANEL_REVIEW", PANEL_REVIEW),
            ("LOCKED", LOCKED),
            ("MENU_REPO", MENU_REPO),
            ("MENU_WORKTREE", MENU_WORKTREE),
            ("MENU_REVIEW", MENU_REVIEW),
            ("MENU_VIEW", MENU_VIEW),
            ("MENU_PANEL", MENU_PANEL),
            ("MENU_SEARCH", MENU_SEARCH),
            ("MENU_TERMINAL", MENU_TERMINAL),
            ("MENU_HELP", MENU_HELP),
        ]
    }

    /// ファイルアイコンの字形を一通り集める。網羅ではなく、各分岐に1つ以上当てるためのもの。
    fn sample_file_icons() -> Vec<FileIcon> {
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

    /// 全ファイルアイコンがどちらの文字セットでも1カラム幅であること。ここが崩れると
    /// アイコンより右の内容が1列ずれ、行がパネル端からはみ出す。
    #[test]
    fn file_icons_are_single_column() {
        for icon in sample_file_icons() {
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

    /// UI アイコンも同じ制約に従うこと。フォールバック側は空 (この文字セットでは
    /// 描かない) を許すが、描くなら1カラムでなければならない。
    #[test]
    fn ui_glyphs_are_single_column() {
        for (name, glyph) in all_glyphs() {
            assert_eq!(
                glyph.get(IconSet::Nerd).width(),
                1,
                "{name} の nerd 側が1カラムでない"
            );
            let fallback = glyph.get(IconSet::Unicode);
            assert!(
                fallback.is_empty() || fallback.width() == 1,
                "{name} のフォールバックが1カラムでない"
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

    /// フォールバック側が Nerd Font の私用領域に入り込んでいないこと。
    /// 入っていると Nerd Font の無い端末で tofu になる。
    #[test]
    fn unicode_fallback_avoids_private_use_area() {
        let fallbacks = sample_file_icons()
            .iter()
            .map(|i| i.glyph(IconSet::Unicode))
            .chain(all_glyphs().iter().map(|(_, g)| g.get(IconSet::Unicode)))
            .chain([expand_arrow(true, IconSet::Unicode)])
            .chain([expand_arrow(false, IconSet::Unicode)])
            .collect::<Vec<_>>();
        for glyph in fallbacks {
            let Some(ch) = glyph.chars().next() else {
                continue; // この文字セットでは描かない。
            };
            assert!(
                !('\u{e000}'..='\u{f8ff}').contains(&ch),
                "U+{:04X} は私用領域にある",
                ch as u32
            );
        }
    }

    /// Nerd Font 側が BMP の私用領域に収まっていること。Plane 15 以降の
    /// コードポイントは端末による幅の扱いが読めないので使わない。
    ///
    /// 罫線のグリフ (COMMENT_SPAN、RANGE_END) は Nerd Font に依存しないので除く。
    #[test]
    fn nerd_glyphs_stay_in_bmp_private_use_area() {
        let box_drawing = ["\u{2502}", "\u{2570}"];
        let nerd_glyphs = sample_file_icons()
            .iter()
            .map(|i| i.glyph(IconSet::Nerd))
            .chain(all_glyphs().iter().map(|(_, g)| g.get(IconSet::Nerd)))
            .chain([expand_arrow(true, IconSet::Nerd)])
            .chain([expand_arrow(false, IconSet::Nerd)])
            .collect::<Vec<_>>();
        for glyph in nerd_glyphs {
            if box_drawing.contains(&glyph) {
                continue;
            }
            let ch = glyph.chars().next().expect("字形が空");
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&ch),
                "U+{:04X} は BMP 私用領域の外にある",
                ch as u32
            );
        }
    }
}
