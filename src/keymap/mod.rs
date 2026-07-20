//! Configurable keybindings — maps key chords to semantic actions.
//!
//! Provides a `KeyMap` that resolves `KeyEvent` → `Action` for a given
//! `KeyContext`, with user overrides from `config.toml`.
//!
//! The engine is [`keymap-suite`](keymap_suite), the one-import facade over
//! `keymap-core`/`keymap-config`/`keymap-seq`. We follow its design directly:
//!
//! * **The action vocabulary is declared once.** The [`Action`] enum, its
//!   stable config names, and `Action::ALL` come from one
//!   [`actions!`](keymap_suite::actions) block; the generated
//!   [`ActionName`](keymap_suite::ActionName) impl (`from_name` / `name`)
//!   replaces the hand-written `from_str` / `as_str` name tables and slots
//!   straight into the suite's loaders.
//! * **Loaded once, owned whole.** [`KeyMap`] holds one [`Loaded<Action>`] — the
//!   facade's TOML-build result — whose `layers` map is keyed by name. Each
//!   `KeyContext` names one layer; `Global` is the bare `[keys]` table
//!   ([`keymap_suite::GLOBAL_LAYER`]).
//! * **The caller assembles the active chain.** Per key event we hand
//!   `resolve_layered([context_layer, global], …)` to the library — the context
//!   layer wins, misses fall through to global, and a total miss returns `None`
//!   ("pass through to the PTY"). The library never tracks our focus/mode; that
//!   stack is ours, exactly as the suite intends.
//! * **Defaults ⊕ user via [`merge`](keymap_suite::merge).** Defaults are
//!   authored in `default_keybinds.toml` (embedded at compile time); user
//!   bindings from `[keybinds]` are an *overlay* merged on top. A user chord
//!   overrides the default for that exact chord; `"<chord>" = false` is a
//!   tombstone that removes a default. We surface only genuine problems as
//!   [`KeybindWarning`]s — override/unbind notes are informational, not warnings.
//! * **Help is the reverse of resolution.** [`KeyMap::keys_for_action`] uses the
//!   facade's [`keys_for_action`](keymap_suite::keys_for_action) so the rendered
//!   shortcuts can never drift from what actually resolves.
//!
//! The module is split by responsibility: [`action`] owns the [`Action`]
//! vocabulary, [`context`] the [`KeyContext`] layer selector, [`warning`] the
//! [`KeybindWarning`] type, and [`map`] the [`KeyMap`] resolver itself that
//! ties the other three together.

mod action;
mod context;
mod map;
mod warning;

pub use action::Action;
pub use context::KeyContext;
pub use map::KeyMap;
// Re-exported for the `crate::keymap::KeybindWarning` path; nothing in-crate
// currently names it directly (callers destructure `KeyMap::with_warnings`'s
// tuple without annotating the `Vec` element type), so rustc can't see it as
// used through this alias.
#[allow(unused_imports)]
pub use warning::KeybindWarning;

#[cfg(test)]
mod tests;
