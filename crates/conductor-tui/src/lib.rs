//! conductor の画面。
//!
//! 入力は route → update → Vec<Effect> → apply の一方通行で、描画は &Workspace しか
//! 取らない。パネルは自分の状態しか &mut で受け取れず、他への影響は Effect でしか
//! 表現できない。

pub mod effect;
pub mod layout;
pub mod liveness;
pub mod modal;
pub mod render;
pub mod route;
pub mod run;
pub mod task;
pub mod term;
pub mod workspace;
