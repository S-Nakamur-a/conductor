//! 見た目の決定に関わる状態: テーマの選択と、シンタックスハイライトの資源。

use syntect::parsing::SyntaxSet;
use two_face::theme::EmbeddedLazyThemeSet;

/// 実際の [crate::theme::Theme] を組み立てるための「元データ」。
///
/// Theme そのものは App::theme に置いてある — 描画のたびに読まれる
/// ホットな値なので、1 階層浅いところに置く価値がある。こちらはそれを
/// 再構築するための入力で、テーマ切り替え・config のライブリロード・
/// OSC11 による自動切り替えのどれもがこの 2 つから Theme を作り直す。
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
