//! conductor のドメイン層。
//!
//! 依存の向きは tui → svc → core。ここは UI もスレッドも知らない。

pub mod cc_hook;
pub mod claude_log;
pub mod claude_sessions;
pub mod config;
pub mod diff_state;
pub mod git_engine;
pub mod icons;
pub mod keymap;
pub mod repo_path;
pub mod review_store;
pub mod symbol_index;
pub mod text_input;
pub mod theme;

#[cfg(test)]
mod test_support;
