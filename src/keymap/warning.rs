//! `KeybindWarning` — survivable problems found while building the keymap.

// ---------------------------------------------------------------------------
// KeybindWarning — survivable problems found while building the keymap
// ---------------------------------------------------------------------------

/// A non-fatal problem found while loading user keybindings. Conductor's own
/// type so the public surface does not depend on `keymap_suite::Warning`
/// (which is `#[non_exhaustive]` and carries sequence concepts Conductor does
/// not use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindWarning {
    /// An action name in the config was not recognized; the binding was skipped.
    UnknownAction { key: String, action: String },
    /// Two keys resolved to the same chord within one layer; the last one won.
    Conflict { chord: String },
    /// A `[keybinds.layers.<name>]` table used a layer name with no matching
    /// context; its bindings were ignored.
    UnknownLayer { layer: String },
    /// The `[keybinds]` config could not be parsed at all (malformed, or the
    /// pre-0.x `[keybinds.<context>]` action→key format). User overrides were
    /// ignored and the built-in defaults are used.
    InvalidConfig { detail: String },
}

impl std::fmt::Display for KeybindWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeybindWarning::UnknownAction { key, action } => {
                write!(f, "unknown keybind action {action:?} for key {key:?}")
            }
            KeybindWarning::Conflict { chord } => {
                write!(
                    f,
                    "keybind chord {chord:?} is bound more than once in one layer"
                )
            }
            KeybindWarning::UnknownLayer { layer } => {
                write!(f, "unknown keybind layer {layer:?}")
            }
            KeybindWarning::InvalidConfig { detail } => {
                write!(f, "could not parse [keybinds] config: {detail}")
            }
        }
    }
}
