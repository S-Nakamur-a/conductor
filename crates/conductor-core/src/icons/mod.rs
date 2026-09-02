//! UI 全体のアイコンの一元定義。
//!
//! 字形は 2 種類持つ。nerd 側は Nerd Font の私用領域 (U+E000-F8FF) のグリフで、おおむね
//! Codicons から選ぶ。Plane 15 以降の md-* を使わないのは、端末による幅の扱いが読めないため。
//!
//! unicode 側は Nerd Font が無い環境向けで、East Asian Width が Neutral かつ Emoji
//! プロパティを持たない文字だけを選ぶ。絵文字を避けるのは、カラー絵文字がフォント側の
//! 固定色で描かれてテーマも減光も効かないことと、幅 2 の解釈が端末ごとに割れて後続の
//! 列がずれるため。
//!
//! どの字形も 1 カラムであることは tests が全定数について検証している。

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

#[cfg(test)]
mod tests;

/// アイコンに使う文字セット。
///
/// Nerd Font が入っているかは端末に問い合わせられず、字形を描いて幅を測っても幅を
/// 決めているのは端末の幅テーブルなので判別できない。判るのは「その端末が Nerd Font の
/// シンボルを同梱しているか」だけで、そこから決めるのが term_caps::detect_icon_set。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconSet {
    Nerd,
    Unicode,
}

/// 1 つのアイコンの字形。文字セットごとに 1 つずつ持つ。
///
/// どちらを描くかは描画時に決める。ファイルツリーのエントリがアイコンを抱えたまま
/// 再構築まで生き残るので、生成時に確定させると文字セットの切り替えに追従できない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    nerd: &'static str,
    unicode: &'static str,
}

impl Glyph {
    const fn new(nerd: &'static str, unicode: &'static str) -> Self {
        Self { nerd, unicode }
    }

    /// 装飾であって情報ではないアイコン。フォールバック側では空文字を返すので、
    /// 呼び出し側はアイコンごと省いてテキストだけを描ける。
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

    /// 字形と、その後ろの区切りスペース。描かない文字セットでは空文字。
    pub fn labeled(&self, set: IconSet) -> String {
        let glyph = self.get(set);
        if glyph.is_empty() {
            String::new()
        } else {
            format!("{glyph} ")
        }
    }
}

/// ファイルアイコンの色と、Nerd Font 無しでの字形を決める粗い種別。
///
/// テーマは 11 個あるので、ファイル種別ごとの色を新設すると全テーマに手が入る。
/// 既存の意味色へ寄せることでテーマ側の変更をなくしている。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconRole {
    Code,
    Markup,
    Data,
    Doc,
    Media,
    Sensitive,
    Dir,
}

impl IconRole {
    /// この種別のアイコン色。
    ///
    /// theme.accent は使わない。多くのテーマで border_focused や selected_bg と同じ値なので、
    /// フォーカス中のパネルでアイコンが枠線に溶ける。Code が本文色なのは数が最も多いからで、
    /// 多数派を落ち着かせて残りを色で立たせるほうが読みやすい。
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

    const fn unicode_glyph(self) -> &'static str {
        match self {
            IconRole::Code => "\u{25c9}",
            IconRole::Markup => "\u{25cc}",
            IconRole::Data => "\u{229e}",
            IconRole::Doc => "\u{25e6}",
            IconRole::Media => "\u{2b25}",
            IconRole::Sensitive => "\u{229f}",
            IconRole::Dir => "\u{22a1}",
        }
    }
}

/// 1 つのファイル種別に対応するアイコン。Nerd Font 側だけ種別より細かい字形を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIcon {
    nerd: &'static str,
    pub role: IconRole,
}

impl FileIcon {
    const fn new(nerd: &'static str, role: IconRole) -> Self {
        Self { nerd, role }
    }

    /// 指定された文字セットでの字形。常に 1 カラム幅である。
    pub fn glyph(&self, set: IconSet) -> &'static str {
        match set {
            IconSet::Nerd => self.nerd,
            IconSet::Unicode => self.role.unicode_glyph(),
        }
    }
}

const PLAIN_FILE: &str = "\u{e64e}";

/// ディレクトリのアイコン。開いているかどうかで字形が変わる。
pub fn dir_icon(is_expanded: bool) -> FileIcon {
    let nerd = if is_expanded { "\u{e5fe}" } else { "\u{e613}" };
    FileIcon::new(nerd, IconRole::Dir)
}

/// ファイル名からアイコンを返す。名前そのものの一致を拡張子より優先する。
pub fn file_icon(name: &str) -> FileIcon {
    let lower = name.to_ascii_lowercase();
    icon_by_name(&lower)
        .unwrap_or_else(|| icon_by_extension(lower.rsplit('.').next().unwrap_or("")))
}

fn icon_by_name(lower: &str) -> Option<FileIcon> {
    use IconRole::*;
    let icon = match lower {
        "cargo.toml" | "cargo.lock" => FileIcon::new("\u{e68b}", Code),
        "package.json" | "package-lock.json" => FileIcon::new("\u{e616}", Data),
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => {
            FileIcon::new("\u{e650}", Data)
        }
        "makefile" | "cmake" | "cmakelists.txt" => FileIcon::new("\u{e673}", Data),
        ".gitignore" | ".gitattributes" | ".gitmodules" => FileIcon::new("\u{e65d}", Sensitive),
        "license" | "license.md" | "license.txt" => FileIcon::new("\u{e60a}", Doc),
        "readme.md" | "readme" | "readme.txt" => FileIcon::new("\u{f02d}", Doc),
        _ => return None,
    };
    Some(icon)
}

fn icon_by_extension(ext: &str) -> FileIcon {
    use IconRole::*;
    match ext {
        "rs" => FileIcon::new("\u{e68b}", Code),
        "py" => FileIcon::new("\u{e606}", Code),
        "js" | "mjs" | "cjs" => FileIcon::new("\u{e60c}", Code),
        "ts" | "mts" | "cts" => FileIcon::new("\u{e628}", Code),
        "jsx" | "tsx" => FileIcon::new("\u{e625}", Code),
        "go" => FileIcon::new("\u{e65e}", Code),
        "rb" => FileIcon::new("\u{e605}", Code),
        "java" | "class" | "jar" => FileIcon::new("\u{e66d}", Code),
        "c" | "h" => FileIcon::new("\u{e649}", Code),
        "cpp" | "cc" | "cxx" | "hpp" => FileIcon::new("\u{e646}", Code),
        "cs" => FileIcon::new("\u{e648}", Code),
        "swift" => FileIcon::new("\u{e699}", Code),
        "kt" | "kts" => FileIcon::new("\u{e634}", Code),
        "php" => FileIcon::new("\u{e608}", Code),
        "lua" => FileIcon::new("\u{e620}", Code),
        "sh" | "bash" | "zsh" | "fish" => FileIcon::new("\u{e691}", Code),
        "html" | "htm" => FileIcon::new("\u{e60e}", Markup),
        "css" | "scss" | "sass" | "less" => FileIcon::new("\u{e614}", Markup),
        "json" | "jsonc" | "json5" => FileIcon::new("\u{e60b}", Data),
        "yaml" | "yml" => FileIcon::new("\u{e6a8}", Data),
        "toml" => FileIcon::new("\u{e615}", Data),
        "xml" | "xsl" => FileIcon::new("\u{e619}", Markup),
        "md" | "mdx" => FileIcon::new("\u{e609}", Doc),
        "txt" | "text" => FileIcon::new(PLAIN_FILE, Doc),
        "sql" => FileIcon::new("\u{e64d}", Data),
        "graphql" | "gql" => FileIcon::new("\u{e662}", Code),
        "proto" => FileIcon::new(PLAIN_FILE, Data),
        "png" | "jpg" | "jpeg" | "gif" | "ico" | "webp" | "bmp" => FileIcon::new("\u{e60d}", Media),
        "svg" => FileIcon::new("\u{e698}", Markup),
        "mp4" | "mov" | "avi" | "webm" => FileIcon::new("\u{e69f}", Media),
        "mp3" | "wav" | "ogg" | "flac" => FileIcon::new("\u{e638}", Media),
        "zip" | "tar" | "gz" | "bz2" | "xz" | "rar" | "7z" => FileIcon::new("\u{e6aa}", Media),
        "pdf" => FileIcon::new("\u{e67d}", Doc),
        "lock" => FileIcon::new("\u{e672}", Sensitive),
        "env" => FileIcon::new("\u{f084}", Sensitive),
        "log" => FileIcon::new("\u{f4ed}", Doc),
        "wasm" => FileIcon::new("\u{e6a1}", Media),
        "test" | "spec" => FileIcon::new("\u{f0c3}", Code),
        _ => FileIcon::new(PLAIN_FILE, Doc),
    }
}

/// リストの展開/折りたたみマーカー。末尾のスペースは含まない。
///
/// フォールバックに塗りつぶしの三角 (U+25B6/25BC) を使わないのは、Emoji プロパティを
/// 持っていて端末によっては幅 2 になるため。
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
/// 塗り円に白抜きの + なのは、背景色を敷いた "+" よりボタンとして浮いて見えるため。
pub const ADD_COMMENT: Glyph = Glyph::new("\u{f055}", "\u{229e}");

/// 実行可能なテスト行のボタン。
pub const RUN_TEST: Glyph = Glyph::new("\u{eb2c}", "\u{25b8}");

/// 選択範囲の終端に置く折り返し罫線。折りたたみの hover 表示と同じ語彙。
pub const RANGE_END: Glyph = Glyph::new("\u{2570}", "\u{2570}");

/// 提案・所感のコメント。
pub const KIND_SUGGEST: Glyph = Glyph::new("\u{ea61}", "!");

/// 人に答えてほしい問いのコメント。
pub const KIND_QUESTION: Glyph = Glyph::new("\u{eb32}", "?");

pub const PANEL_EXPLORER: Glyph = Glyph::nerd_only("\u{ea83}");

pub const PANEL_CHANGED: Glyph = Glyph::nerd_only("\u{eae1}");

/// レビューコメント一覧。
pub const PANEL_COMMENTS: Glyph = Glyph::nerd_only("\u{eac7}");

/// PTY のパネル。
pub const PANEL_TERMINAL: Glyph = Glyph::nerd_only("\u{ea85}");

/// revidere のレビュービュー。
pub const PANEL_REVIEW: Glyph = Glyph::nerd_only("\u{eab3}");

/// 別プロセスに grab されていて操作できないパネル。
pub const LOCKED: Glyph = Glyph::new("\u{ea75}", "\u{22a0}");

pub const MENU_REPO: Glyph = Glyph::nerd_only("\u{ea62}");

/// worktree とブランチ。
pub const MENU_WORKTREE: Glyph = Glyph::nerd_only("\u{ec6f}");

pub const MENU_REVIEW: Glyph = Glyph::nerd_only("\u{eac7}");

/// 表示の切り替え。
pub const MENU_VIEW: Glyph = Glyph::nerd_only("\u{ea70}");

/// パネルのレイアウト。
pub const MENU_PANEL: Glyph = Glyph::nerd_only("\u{ebeb}");

pub const MENU_SEARCH: Glyph = Glyph::nerd_only("\u{ea6d}");

pub const MENU_TERMINAL: Glyph = Glyph::nerd_only("\u{ea85}");

pub const MENU_HELP: Glyph = Glyph::nerd_only("\u{eb32}");
