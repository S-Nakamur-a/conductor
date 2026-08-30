//! App のフィールドを機能単位にまとめた状態構造体群。
//!
//! App は全状態をフラットなフィールドとして持つ単一構造体だが、フィールドが
//! 増えるにつれ「どれとどれが一緒に動くのか」がコメントの区切り線でしか表現
//! されなくなっていた。ここでは同じ機能に属していて必ず一緒に読み書きされる
//! フィールドだけを構造体に束ねる。既にある BackgroundOps と同じ方針。
//!
//! 逆に、単独で意味が完結しているフィールド (theme のような 1 個で 1 概念の
//! もの) は入れ子にしても読みやすくならないので App 直下に残している。

mod appearance;
mod bars;
mod code_nav;
mod hover;
mod layout;
mod panel_number;
mod publish;
mod repo;
mod stats;
mod update_flow;
mod view_restore;
mod worktrees;

pub use crate::revidere::state::RevidereState;
pub use appearance::{Highlighting, ThemeSelection};
pub use bars::WtbarState;
pub use code_nav::CodeNav;
pub use hover::ListHover;
pub use layout::PanelLayout;
pub use panel_number::PanelNumberOverlay;
pub use publish::PublishState;
pub use repo::RepoState;
pub use stats::SessionStats;
pub use update_flow::UpdateFlow;
pub use view_restore::ViewRestore;
pub use worktrees::WorktreeList;
