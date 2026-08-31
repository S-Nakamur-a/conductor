//! 描画に使う色と、そこから色を焼き込んだキャッシュ。

use syntect::parsing::SyntaxSet;
use two_face::theme::EmbeddedLazyThemeSet;

use crate::theme::Theme;
use crate::ui::markdown::MarkdownCache;

/// 一緒に動かさないと壊れるものの集まり。
///
/// theme を差し替えたら、その色を span へ焼き込んだキャッシュも捨てなければ
/// 古い配色が残る。差し替えの入口は [crate::app::App::install_palette] 一本。
pub struct Appearance {
    /// 描画のたびに読むので、素のフィールドで持つ。
    pub theme: Theme,
    /// theme を組み立てる元データ。
    pub sel: ThemeSelection,
    /// syntect の共有資源。
    pub highlight: Highlighting,
    /// コメント本文の ID 別キャッシュ。インラインスレッドが毎フレーム再パースしない。
    pub markdown_cache: MarkdownCache,
}

/// [Appearance::theme] を組み立てるための元データ。
///
/// テーマ切り替え・config のライブリロード・OSC11 の自動切り替えのどれもが、
/// この 2 つから Theme を作り直す。
#[derive(Default)]
pub struct ThemeSelection {
    /// 有効なテーマ名。Theme を引くための正準キー。
    pub name: String,
    /// ハイコントラスト変換を適用するか。config.ui.high_contrast の写し。
    pub high_contrast: bool,
}

/// syntect によるシンタックスハイライトの共有資源。
///
/// syntax_set と themes は構築に時間がかかるので起動時に 1 度だけ作り、
/// 以降は使い回す。theme はテーマ切り替えのたびに themes から引き直す。
pub struct Highlighting {
    /// 共有の syntect 構文定義。
    pub syntax_set: SyntaxSet,
    /// 組み込みハイライトテーマ集 (two-face)。個々のテーマは初回参照時に
    /// 遅延ロードされるので、保持しておいてもコストはほぼ無い。
    pub themes: EmbeddedLazyThemeSet,
    /// 有効な syntect ハイライトテーマ。
    pub theme: syntect::highlighting::Theme,
    /// theme を解決した入力の指紋 ([crate::config::syntax_theme_id])。
    /// 実際にテーマが変わったときだけキャッシュを捨てるための比較用。
    pub theme_id: String,
    /// テーマを差し替えるたびに増える世代番号。
    ///
    /// ハイライト結果のキャッシュキーに混ぜる。これが無いと、キャッシュは
    /// ファイル内容だけを指紋にしているので、テーマを変えても「内容は同じ」
    /// と判定されて古い配色の span が残り続ける。
    pub generation: u64,
}
