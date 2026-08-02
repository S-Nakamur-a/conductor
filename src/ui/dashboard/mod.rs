//! ダッシュボードのオーバーレイ群 — 履歴ビューア、worktree 入力、cherry-pick、
//! リポジトリセレクタ、open-repo ポップアップ。
//!
//! これらはメインの3カラムレイアウトの上にオーバーレイとして描画される。
//!
//! オーバーレイのグループごとにサブモジュールへ分割している。input には全体で
//! 共有するテキスト入力の描画ヘルパーを置く。呼び出し元が引き続き
//! crate::ui::dashboard::render_x を使えるよう、ここで re-export する。

mod branch_picker;
mod command_palette;
mod filename_search;
mod help;
mod history;
mod input;
mod repo;
mod resume_session;
mod update;
mod worktree;

pub use branch_picker::{
    render_cherry_pick_overlay, render_grab_overlay, render_prune_overlay,
    render_switch_branch_overlay,
};
pub use command_palette::render_command_palette_overlay;
pub use filename_search::render_filename_search_overlay;
pub use help::render_help_overlay;
pub use history::render_history_overlay;
pub use repo::{render_open_repo_overlay, render_pr_input_overlay, render_repo_selector_overlay};
pub use resume_session::render_resume_session_overlay;
pub use update::{
    render_publish_confirm_overlay, render_update_confirm_overlay, render_update_progress_overlay,
};
pub use worktree::{
    render_delete_branch_confirm_overlay, render_smart_description_overlay,
    render_worktree_base_input_overlay, render_worktree_input_overlay,
};
