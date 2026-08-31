//! 画面全幅のバー（タイトルバー・ステータスバー・worktree ラベル）。
//!
//! いずれも [crate::ui::layout::render] からのみ呼ばれる、レイアウトの
//! 3カラムに属さない行。パネル横断で使う色計算などの共有プリミティブは
//! [crate::ui::common] にある。

mod status_bar;
mod title_bar;
mod worktree_label;

#[cfg(test)]
mod tests;

pub use status_bar::render_status_bar;
pub use title_bar::render_title_bar;
pub use worktree_label::render_worktree_label;
