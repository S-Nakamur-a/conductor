//! `KeyMap` — resolves a `KeyEvent` to an `Action` for a given `KeyContext`,
//! built from the embedded defaults merged with the user's `[keybinds]`
//! config overlay.

use crossterm::event::KeyEvent;
use keymap_suite::{ActionName, KeyInput, Keymap, Loaded, resolve_layered};

use super::action::Action;
use super::context::{KeyContext, PANEL_CONTEXTS};
use super::warning::KeybindWarning;

// ---------------------------------------------------------------------------
// KeyMap
// ---------------------------------------------------------------------------

/// Embedded default bindings (keymap-suite key→action TOML). See the file for
/// the schema; it is the reference for what users can write under `[keybinds]`.
pub(crate) const DEFAULT_KEYBINDS: &str = include_str!("../default_keybinds.toml");

pub struct KeyMap {
    /// The merged keymap: defaults (`default_keybinds.toml`) with the user's
    /// `[keybinds]` overlaid via [`keymap_suite::merge`]. Its `layers` map is
    /// keyed by layer name; [`KeyContext::layer_name`] selects one per event and
    /// `global()` is consulted last. Holding the facade's own `Loaded` value
    /// (rather than re-bucketing it) is the suite's intended shape.
    loaded: Loaded<Action>,
}

impl KeyMap {
    /// Build a `KeyMap` from defaults plus the user's `[keybinds]` config table,
    /// discarding any warnings. See [`KeyMap::with_warnings`] to inspect them.
    #[allow(dead_code)] // convenience constructor; the app uses `with_warnings`.
    pub fn new(user: &toml::Table) -> Self {
        Self::with_warnings(user).0
    }

    /// Build a `KeyMap`, returning any non-fatal problems found in the user's
    /// config so the caller can surface them (the app flashes them on startup).
    pub fn with_warnings(user: &toml::Table) -> (Self, Vec<KeybindWarning>) {
        let mut warnings = Vec::new();

        // 1. Embedded defaults — the merge base. Authored in-repo, so any
        //    warning is a build bug: fail loudly in debug, never reach the user.
        let defaults = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_name)
            .expect("embedded default keybinds must be valid TOML");
        debug_assert!(
            defaults.warnings.is_empty(),
            "default keybinds produced warnings: {:?}",
            defaults.warnings
        );
        for w in &defaults.warnings {
            log::error!("default keybinds produced a warning (bug): {w:?}");
        }

        // 2. Parse the user's `[keybinds]` overlay and merge it onto the
        //    defaults. `merge` does the per-chord override and applies any
        //    `= false` tombstones; we keep only real problems as warnings (its
        //    override/unbind notes are informational, not warnings).
        let loaded = match parse_user_keybinds(user, &mut warnings) {
            Some(overlay) => {
                warn_unknown_layers(&overlay, &mut warnings);
                let merged = keymap_suite::merge(defaults, overlay);
                collect_warnings(&merged.output.warnings, &mut warnings);
                merged.output
            }
            None => defaults,
        };

        (KeyMap { loaded }, warnings)
    }

    /// The active layer chain for `context`: the context's own layer first (when
    /// it has one and is not `Global`), then the always-on global layer. This is
    /// the per-event stack the suite asks the caller to assemble.
    fn chain(&self, context: KeyContext) -> Vec<&Keymap<Action>> {
        let global = self.loaded.global();
        if context == KeyContext::Global {
            return vec![global];
        }
        match self.loaded.layers.get(context.layer_name()) {
            Some(layer) => vec![layer, global],
            None => vec![global],
        }
    }

    /// Resolve a key event to an action in the given context. The context layer
    /// is consulted first, then the global layer; an unmappable key event or a
    /// total miss yields `None` (the caller passes the key through).
    ///
    /// In the terminal context, an action that does not [fire in the
    /// terminal](Action::fires_in_terminal) resolves to `None` so the chord
    /// reaches the PTY — the global fallback stays, but globally-bound actions
    /// the terminal shouldn't steal (quit, switch-repo, …) are filtered here
    /// rather than by an allowlist in the dispatcher.
    pub fn resolve(&self, key: &KeyEvent, context: KeyContext) -> Option<Action> {
        let input = KeyInput::try_from(*key).ok()?;
        let action = resolve_layered(self.chain(context).iter().copied(), &input).copied()?;
        // The editor panel forwards keys to its PTY exactly like the terminal,
        // so it honors the same "only steal terminal-firing actions" filter —
        // everything else (Esc, Ctrl+G, …) reaches vim/emacs untouched.
        if matches!(context, KeyContext::Terminal | KeyContext::Editor)
            && !action.fires_in_terminal()
        {
            return None;
        }
        Some(action)
    }

    /// Display strings for every key bound to an action in a context (context
    /// layer plus the global layer), for the help screen. Strings are
    /// keymap-core canonical form (e.g. `"ctrl+d"`, `"down"`, `"G"`), which
    /// round-trips back through the config grammar.
    pub fn keys_for_action(&self, context: KeyContext, action: Action) -> Vec<String> {
        // Keep the rendered help honest with `resolve`: in the terminal and
        // editor contexts, a globally-bound action that doesn't fire there has
        // no working chord.
        if matches!(context, KeyContext::Terminal | KeyContext::Editor)
            && !action.fires_in_terminal()
        {
            return Vec::new();
        }

        // The reverse of resolution, over the same chain `resolve` consults, so
        // the rendered help can never advertise a chord that would not fire.
        let mut keys: Vec<String> = self
            .chain(context)
            .iter()
            .flat_map(|layer| keymap_suite::keys_for_action(layer, &action))
            .map(|input| input.to_string())
            .collect();

        keys.sort();
        keys.dedup();
        keys
    }

    /// Keys bound to `action` in `context`'s OWN layer only — unlike
    /// [`keys_for_action`](Self::keys_for_action), this does NOT fold in the
    /// global layer. Lets a caller tell "bound in this panel" from "bound
    /// globally and merely reachable here" (used to scope the command palette).
    pub fn keys_in_layer(&self, context: KeyContext, action: Action) -> Vec<String> {
        let layer = if context == KeyContext::Global {
            self.loaded.global()
        } else {
            match self.loaded.layers.get(context.layer_name()) {
                Some(layer) => layer,
                None => return Vec::new(),
            }
        };
        let mut keys: Vec<String> = keymap_suite::keys_for_action(layer, &action)
            .into_iter()
            .map(|input| input.to_string())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }
}

/// Warn about any user `[keybinds.layers.<name>]` whose name matches no
/// [`KeyContext`] — its bindings are merged but never consulted. The empty
/// `GLOBAL_LAYER` the loader always injects is skipped, so only a genuinely
/// unrecognized, non-empty named layer warns.
fn warn_unknown_layers(overlay: &Loaded<Action>, warnings: &mut Vec<KeybindWarning>) {
    for (name, layer) in &overlay.layers {
        if name == keymap_suite::GLOBAL_LAYER || layer.is_empty() {
            continue;
        }
        if PANEL_CONTEXTS.iter().all(|c| c.layer_name() != name) {
            warnings.push(KeybindWarning::UnknownLayer {
                layer: name.clone(),
            });
        }
    }
}

/// Parse the user's `[keybinds]` table into a keymap-suite overlay. Returns
/// `None` (no overrides) when the table is empty or cannot be parsed; a parse
/// failure is recorded as a [`KeybindWarning::InvalidConfig`] so the app can
/// tell the user their customizations were ignored.
fn parse_user_keybinds(
    user: &toml::Table,
    warnings: &mut Vec<KeybindWarning>,
) -> Option<Loaded<Action>> {
    if user.is_empty() {
        return None;
    }

    // keymap-suite parses a standalone document; re-emit just the [keybinds]
    // subtree as TOML text. (Conductor's `toml` and the suite's may differ in
    // version, so the interface between them is text, not types.)
    let toml_text = match toml::to_string(user) {
        Ok(text) => text,
        Err(e) => {
            warnings.push(KeybindWarning::InvalidConfig {
                detail: e.to_string(),
            });
            return None;
        }
    };

    match keymap_suite::from_toml_str(&toml_text, Action::from_name) {
        Ok(build) => Some(build),
        Err(e) => {
            warnings.push(KeybindWarning::InvalidConfig {
                detail: format!(
                    "{e} (note: the keybind format is now key→action under \
                     [keybinds.keys] / [keybinds.layers.*]; the old \
                     [keybinds.<context>] action→key tables are no longer read)"
                ),
            });
            None
        }
    }
}

/// Translate the keymap-suite warnings Conductor cares about into its own
/// warning type, dropping sequence-related variants it does not use.
fn collect_warnings(from: &[keymap_suite::Warning], into: &mut Vec<KeybindWarning>) {
    for w in from {
        match w {
            keymap_suite::Warning::UnknownAction { key, action } => {
                into.push(KeybindWarning::UnknownAction {
                    key: key.clone(),
                    action: action.clone(),
                });
            }
            keymap_suite::Warning::Conflict { chord, .. } => {
                into.push(KeybindWarning::Conflict {
                    chord: chord.clone(),
                });
            }
            // PrefixShadow / EmptySequence / SequenceShadow concern sequences,
            // which Conductor does not use. `Warning` is #[non_exhaustive].
            _ => {}
        }
    }
}
