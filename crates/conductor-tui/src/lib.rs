//! conductor の画面。
//!
//! 入力は route → update → Vec<Effect> → apply の一方通行で、描画は &Workspace しか
//! 取らない。パネルは自分の状態しか &mut で受け取れず、他への影響は Effect でしか
//! 表現できない。

/// このバイナリのバージョン。更新の判定とタイトルバーが読む。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod command;
pub mod comment_list;
pub mod effect;
pub mod entrance;
pub mod index;
pub mod layout;
pub mod list;
pub mod liveness;
pub mod markdown;
pub mod menu;
pub mod modal;
pub mod panels;
pub mod render;
pub mod review;
pub mod route;
pub mod run;
pub mod search_tree;
pub mod strip;
pub mod task;
pub mod term;
#[cfg(test)]
pub(crate) mod testing;
pub mod timer;
pub mod workspace;
