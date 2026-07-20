//! Explorer panel key handling (file tree, diff list, comment list).
//!
//! Entry point ([`tree::handle_explorer_key`]) plus the two sub-panels it
//! delegates to when the bottom pane has focus: [`diff_list`] (unified diff
//! list navigation) and [`comment_list`] (review comment list navigation,
//! including [`comment_list::navigate_to_comment_with_focus`]).
//! [`viewer_actions`] holds the Viewer-panel-triggered comment actions
//! (opening the add-comment input, the comment detail modal, and parsing
//! submitted comment text) that don't belong to tree navigation itself.

mod comment_list;
mod diff_list;
mod tree;
mod viewer_actions;

// Re-exported so sibling `event` submodules (`mouse`, `viewer`, `overlay`,
// and `event::mod` itself) keep resolving their existing
// `super::explorer::X` / `crate::event::explorer::X` references unchanged
// now that these items live one directory level deeper.
pub(in crate::event) use comment_list::{handle_explorer_comment_list_key, navigate_to_comment_with_focus};
pub(in crate::event) use tree::handle_explorer_key;
pub(in crate::event) use viewer_actions::{open_viewer_comment, open_viewer_comment_detail, submit_new_comment};
