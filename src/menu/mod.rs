//! Menu bar — the pointer-and-arrow-key route to every command.
//!
//! Conductor already had two ways to run a command: a chord from
//! `default_keybinds.toml`, and the fuzzy command palette. Both require knowing
//! what you are looking for. The menu bar is the browsable third route: a
//! permanent strip under the title bar whose dropdowns list the operations
//! grouped by what they act on.
//!
//! It adds no behaviour of its own. Every row carries a
//! [`CommandId`](crate::command_palette::CommandId) and activating it calls
//! [`App::execute_palette_command`](crate::app::App::execute_palette_command) —
//! the same entry point the palette uses and the same methods the keyboard
//! actions in `event::global` call. The shortcut shown at the right of a row is
//! read live from the keymap, so a rebind in the user's config is reflected
//! without touching this module.
//!
//! - [`model`] — the static table: which command sits under which menu.
//! - [`state`] — interaction state ([`MenuFocus`]) and the pure navigation
//!   helpers shared by the keyboard and mouse handlers.
//! - [`enabled`] — whether a command can run right now, for the greyed-out rows.
//!
//! Rendering lives in [`crate::ui::menu_bar`]; input handling in
//! [`crate::event::menu`] (keys) and `crate::event::mouse` (clicks and hover).

pub mod enabled;
pub mod model;
pub mod state;

#[cfg(test)]
mod tests;

pub use enabled::command_enabled;
pub use model::MenuItem;
pub use state::{MenuFocus, MenuState};
