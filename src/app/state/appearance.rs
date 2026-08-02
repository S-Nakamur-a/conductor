//! 見た目の決定に関わる状態: テーマの選択と、シンタックスハイライトの資源。

use syntect::parsing::SyntaxSet;

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
/// どちらも構築に時間がかかるので起動時に 1 度だけ作り、以降は使い回す。
pub struct Highlighting {
    /// 共有の syntect 構文定義。
    pub syntax_set: SyntaxSet,
    /// 有効な syntect ハイライトテーマ。
    pub theme: syntect::highlighting::Theme,
}
