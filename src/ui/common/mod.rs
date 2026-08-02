//! 複数のパネルで共有される UI コンポーネント。
//!
//! PTY 出力の描画、セッションタブバー、ステータスバーなど再利用可能なウィジェットを提供する。
//! 責務ごとに分割している: [pty]（vt100 → ratatui の描画とそのキャッシュ）、
//! [color]（バッジ/コントラストの色計算）、そしてトップレベルのバー・ウィジェットごとに
//! 1ファイル（[title_bar], [status_bar], [worktree_label]）。

mod color;
pub mod list_row;
mod panel_chrome;
mod pty;
mod status_bar;
mod title_bar;
mod worktree_label;

#[cfg(test)]
mod tests;

pub use pty::{PtyRenderCache, build_pty_lines, render_pty_cached};
pub use status_bar::render_status_bar;
pub(crate) use status_bar::representative_chord;
pub use title_bar::render_title_bar;
pub use panel_chrome::PanelChrome;
pub use worktree_label::render_worktree_label;

/// 非同期処理中に使う点字スピナーのフレーム一覧。
const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 指定した UI tick に対応する現在のスピナーフレーム。およそ4フレームごとに進めることで
/// 安定した回転に見えるようにしている。非同期処理のスピナーを表示するすべてのパネルで
/// 共有し、アニメーションを同期させる。
pub fn spinner_frame(ui_tick: u64) -> &'static str {
    BRAILLE_SPINNER[(ui_tick as usize / 4) % BRAILLE_SPINNER.len()]
}
