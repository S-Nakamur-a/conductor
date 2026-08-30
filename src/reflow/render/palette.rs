//! Claude Code の固定パレット（ダークテーマ）。CLI にハードコードされたダークテーマから
//! そのまま持ってきており、conductor 側のテーマ設定に関わらずトランスクリプトが本物どおりに
//! 見えるようにする。（conductor 自体のテーマは他の全パネルを支配するが、このオーバーレイだけ
//! Claude のパレットに固定する。）

use ratatui::style::Color;

use crate::theme::Theme;

/// Claude を象徴するコーラル/オレンジのアクセントカラー。claude トークン。
pub(crate) const CLAUDE: Color = Color::Rgb(215, 119, 87);
/// 本文テキスト。端末のデフォルト前景色に追従する（Color::Reset）。ライブの PTY 表示
/// （vt100::Color::Default → Color::Reset）とまったく同じ扱いである。ここに純白を
/// ハードコードすると、ライブパネルからトランスクリプトへスクロールした際にライブより
/// 明るく粗く見えた。端末デフォルトに委ねることで、本文テキストについては両ビューが
/// ピクセル単位で一致する。
pub(crate) const TEXT: Color = Color::Reset;
/// ツール呼び出しの箇条書きマーカー。success トークン（緑）。
pub(crate) const SUCCESS: Color = Color::Rgb(78, 186, 101);
/// エラー接続線。error トークン（コーラルレッド）。
pub(crate) const ERROR: Color = Color::Rgb(255, 107, 128);
/// 淡色・補助テキスト。inactive トークン（グレー）。
pub(crate) const INACTIVE: Color = Color::Rgb(153, 153, 153);
/// 見出し・リンク用のアクセント。permission トークン（ペリウィンクル）。
pub(crate) const PERMISSION: Color = Color::Rgb(177, 185, 249);
/// インラインコード・ごく淡い色。subtle トークン。
pub(crate) const SUBTLE: Color = Color::Rgb(80, 80, 80);
/// user ターンの全幅ブロックの背景色（実測値）。
pub(crate) const USER_BG: Color = Color::Rgb(55, 55, 55);
/// user ターンの背景ブロック上での ❯ プロンプトマーカーの色。
pub(crate) const USER_MARKER_FG: Color = Color::Rgb(80, 80, 80);
/// user ターンの背景ブロック上での本文テキストの色。
pub(crate) const USER_TEXT: Color = Color::Rgb(255, 255, 255);

/// Markdown レンダラ向けに Claude 風の [Theme] を組み立てる。これにより、トランスクリプト内の
/// 地の文・見出し・リンク・コードが、有効な conductor テーマではなく Claude Code の
/// パレットを採用するようになる。Markdown レンダラが参照するフィールドだけを上書きし、
/// それ以外は base から引き継ぐ。
pub(crate) fn claude_markdown_theme(base: &Theme) -> Theme {
    let mut t = base.clone();
    t.fg = TEXT;
    t.muted = INACTIVE;
    t.hint = INACTIVE;
    t.accent = CLAUDE;
    t.info = PERMISSION;
    t.success = SUCCESS;
    t.error = ERROR;
    t.warning = Color::Rgb(255, 193, 7);
    t.border_secondary = SUBTLE;
    t.code_fg = TEXT;
    t.code_bg = Color::Rgb(43, 43, 43);
    t
}
