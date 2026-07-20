//! Overlay handlers — worktree input, cherry-pick, history, resume session,
//! repo selector, open repo, comment detail, help, filename search, grep search,
//! viewer search, review input, review search, review template, switch branch,
//! grab, prune, command palette.
//!
//! Split into per-topic submodules; this file holds shared list-navigation
//! helpers used across overlays and re-exports each handler so callers keep
//! using `crate::event::overlay::handle_*` unchanged.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keymap::{Action, KeyContext, KeyMap};

mod misc;
mod repo;
mod review;
mod search;
mod session;
mod symbol;
mod vcs;
mod worktree;

pub(super) use misc::{handle_command_palette_key, handle_help_key, handle_theme_picker_key};
pub(super) use repo::{handle_open_repo_key, handle_pr_input_key, handle_repo_selector_key};
pub(super) use review::{
    handle_comment_detail_key, handle_review_input_key, handle_review_search_key,
    handle_review_template_key,
};
pub(super) use search::{
    handle_filename_search_key, handle_grep_search_key, handle_viewer_search_key,
};
pub(super) use session::{handle_history_key, handle_resume_session_key};
pub(super) use symbol::{
    handle_references_key, handle_symbol_action_key, handle_symbol_hint_key,
};
pub(super) use vcs::{
    handle_cherry_pick_key, handle_grab_key, handle_prune_key, handle_switch_branch_key,
};
pub(super) use worktree::handle_worktree_input_key;

// ── Shared overlay list navigation ────────────────────────────────────

/// Handle common list-navigation keys for overlay popups via the keymap.
///
/// Resolves the key against `KeyContext::Overlay` and adjusts `*selected`
/// within `0..count`. Returns `true` if the key was consumed.
fn overlay_list_nav(keymap: &KeyMap, key: &KeyEvent, selected: &mut usize, count: usize) -> bool {
    let Some(action) = keymap.resolve(key, KeyContext::Overlay) else {
        return false;
    };
    apply_list_nav(action, selected, count)
}

/// Would this key event produce a literal character for a text field? A bare
/// printable char (no Ctrl/Alt/Super) is typed input. SHIFT is intentionally
/// NOT disqualifying: `Shift+G` is a literal `G` to type into a filter.
fn is_text_input_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// List navigation for overlays that ALSO have a text filter (command palette,
/// filename search, …). A bare printable key falls through to the filter, so
/// only non-text keys (arrows, PageUp/Down) navigate — `j`/`k`/`g` get typed.
fn filterable_overlay_list_nav(
    keymap: &KeyMap,
    key: &KeyEvent,
    selected: &mut usize,
    count: usize,
) -> bool {
    if is_text_input_key(key) {
        return false;
    }
    overlay_list_nav(keymap, key, selected, count)
}

fn apply_list_nav(action: Action, selected: &mut usize, count: usize) -> bool {
    match action {
        Action::NavigateDown => {
            if count > 0 && *selected + 1 < count {
                *selected += 1;
            }
            true
        }
        Action::NavigateUp => {
            if *selected > 0 {
                *selected -= 1;
            }
            true
        }
        Action::GoToTop => {
            *selected = 0;
            true
        }
        Action::GoToBottom => {
            if count > 0 {
                *selected = count - 1;
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod nav_guard_tests {
    use super::is_text_input_key;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn bare_printable_char_is_text_input() {
        // In a filterable overlay these must be typed, not treated as navigation.
        for c in ['j', 'k', 'g', 'G'] {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty());
            assert!(is_text_input_key(&key), "{c:?} should be text input");
        }
        // Shift is not disqualifying: Shift+G is a literal 'G' to type.
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert!(is_text_input_key(&shift_g));
    }

    #[test]
    fn modified_and_named_keys_are_not_text_input() {
        // These should still drive list navigation in a filterable overlay.
        let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        let alt_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        assert!(!is_text_input_key(&ctrl_n));
        assert!(!is_text_input_key(&alt_j));
        assert!(!is_text_input_key(&up));
        assert!(!is_text_input_key(&down));
    }
}
