//! Test fixtures shared by the keymap's test suite, split by concern into
//! [`resolution`] (default-keymap resolution across contexts), [`overrides`]
//! (user `[keybinds]` overlay/tombstone/warning behavior), and [`edge_cases`]
//! (chord-normalization and miscellaneous edge cases).

use super::*;

mod edge_cases;
mod overrides;
mod resolution;

fn default_keymap() -> KeyMap {
    KeyMap::new(&toml::Table::new())
}
