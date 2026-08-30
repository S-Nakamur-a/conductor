//! ステータスバーのキーバインドヒントのテスト。

use crate::keymap::{Action, KeyContext, KeyMap};
use crate::types::Focus;
use crate::ui::chrome::status_bar::status_bar_hint;
use crate::ui::common::representative_chord;

fn keymap() -> KeyMap {
    KeyMap::new(&toml::Table::new())
}

#[test]
fn representative_chord_prefers_short_ascii_over_unicode() {
    let km = keymap();
    // cycle_focus_backward は alt+h と macOS 特有のグリフ '˙' の両方に割り当てられているが、
    // グリフの方は絶対に表示してはならない。
    let chord = representative_chord(&km, KeyContext::Global, Action::CycleFocusBackward);
    assert_eq!(chord, Some("alt+h".to_string()));

    // nav はエイリアスの 'down' より素の 'j' を優先する。
    let nav = representative_chord(&km, KeyContext::Worktree, Action::NavigateDown);
    assert_eq!(nav, Some("j".to_string()));
}

#[test]
fn worktree_footer_is_truthful_and_has_no_unicode() {
    let hint = status_bar_hint(Focus::Worktree, &keymap());
    assert!(hint.contains("j/k: nav"), "{hint}");
    assert!(hint.contains("tab: panel"), "{hint}");
    assert!(hint.contains("w: new"), "{hint}");
    // 古いハードコードされた嘘の記述は消えていて、フォールバックのグリフも混ざらない。
    assert!(!hint.contains("Cmd+1-5"), "{hint}");
    assert!(hint.is_ascii(), "footer must be ASCII-only: {hint}");
}

#[test]
fn terminal_footer_notes_passthrough_and_leave_key() {
    let hint = status_bar_hint(Focus::TerminalClaude, &keymap());
    assert!(hint.contains("keys → terminal"), "{hint}");
    // leave_terminal は ctrl+esc であり、単なる Esc ではない。
    assert!(hint.contains("ctrl+esc: leave"), "{hint}");
}
