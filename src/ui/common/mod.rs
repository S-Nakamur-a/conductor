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

pub use panel_chrome::PanelChrome;
pub use pty::{PtyRenderCache, build_pty_lines, render_pty_cached};
pub use status_bar::render_status_bar;
pub(crate) use status_bar::representative_chord;
pub use title_bar::render_title_bar;
pub use worktree_label::render_worktree_label;

/// 非同期処理中に使う点字スピナーのフレーム一覧。
const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 指定した UI tick に対応する現在のスピナーフレーム。およそ4フレームごとに進めることで
/// 安定した回転に見えるようにしている。非同期処理のスピナーを表示するすべてのパネルで
/// 共有し、アニメーションを同期させる。
pub fn spinner_frame(ui_tick: u64) -> &'static str {
    BRAILLE_SPINNER[(ui_tick as usize / 4) % BRAILLE_SPINNER.len()]
}

/// revidere の状態を表す 1 文字。幅は常に 1。
///
/// 色だけで区別すると配色や色覚によって読めなくなるので、形でも分かるように
/// してある。✓ は「作業ツリーが綺麗」「ファイルを読んだ」に既に使っていて、
/// レビューの印に流用すると git の情報と見分けが付かない。
pub fn revidere_marker(state: crate::revidere::ArtifactState, ui_tick: u64) -> &'static str {
    use crate::revidere::ArtifactState as S;
    match state {
        S::Running => spinner_frame(ui_tick),
        S::Fresh => "\u{25a4}", // ▤
        S::Stale => "!",
        S::None => "\u{25cb}", // ○
    }
}

/// revidere の状態の色。muted は複数のテーマで見えなくなるので使わない。
///
/// この色は必ず素の背景の上で使う。選択中の worktree チップのような塗りの
/// 上に重ねてはいけない — 全テーマで accent と selected_bg が同じ色なので、
/// 実行中の印が背景と完全に同色になって消える。
pub fn revidere_color(
    theme: &crate::theme::Theme,
    state: crate::revidere::ArtifactState,
) -> ratatui::style::Color {
    use crate::revidere::ArtifactState as S;
    match state {
        S::None => theme.hint,
        S::Running => theme.accent,
        S::Fresh => theme.success,
        S::Stale => theme.warning,
    }
}
