//! Command palette — fuzzy-searchable command index.
//!
//! Provides a VSCode-style command palette (`Ctrl+P` / `:`) for discovering and
//! executing any application command. Each command carries the keymap [`Action`]
//! it corresponds to (when it has one), so its displayed shortcut and its scope
//! (global vs. the focused panel's layer) are derived live from the keymap and
//! never go stale. Palette-only commands (no keybinding) carry `action: None`.
//!
//! Split into: `types` (the `CommandId`/`CommandCategory`/`CommandScope`/
//! `PaletteCommand`/`ScoredCommand` model), `commands` (the static `COMMANDS`
//! table), and `search` (fuzzy filtering/scoring). Re-exported here so callers
//! keep using `crate::command_palette::X`.

mod commands;
mod search;
#[cfg(test)]
mod tests;
mod types;

pub use commands::COMMANDS;
pub use search::filter_commands;
// `CommandCategory`/`PaletteCommand`/`ScoredCommand` aren't referenced outside
// this module today, but they're part of the pre-split public path
// (`crate::command_palette::X`) and are re-exported to preserve it.
#[allow(unused_imports)]
pub use types::{CommandCategory, CommandId, CommandScope, PaletteCommand, ScoredCommand};
