//! オーバーレイハンドラ群 — worktree 入力、cherry-pick、履歴、セッション再開、
//! リポジトリセレクタ、リポジトリオープン、コメント詳細、ヘルプ、ファイル名検索、
//! grep 検索、viewer 検索、レビュー入力、レビュー検索、レビューテンプレート、
//! ブランチ切り替え、grab、prune、コマンドパレット。
//!
//! トピックごとのサブモジュールに分割している。このファイルはオーバーレイ間で
//! 共有するリスト操作ヘルパーを持ち、各ハンドラを再エクスポートすることで
//! 呼び出し側は crate::event::overlay::handle_* をそのまま使い続けられる。

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
    handle_comment_detail_key, handle_revidere_confirm_key, handle_review_input_key,
    handle_review_search_key, handle_review_template_key,
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

// オーバーレイ間で共有するリストナビゲーション

/// keymap 経由でオーバーレイポップアップ共通のリストナビゲーションキーを処理する。
///
/// KeyContext::Overlay に対してキーを解決し、*selected を 0..count の範囲で
/// 調整する。キーを消費した場合は true を返す。
fn overlay_list_nav(keymap: &KeyMap, key: &KeyEvent, selected: &mut usize, count: usize) -> bool {
    let Some(action) = keymap.resolve(key, KeyContext::Overlay) else {
        return false;
    };
    apply_list_nav(action, selected, count)
}

/// このキーイベントはテキストフィールドにそのまま文字を入力するものか？
/// 素の印字可能文字（Ctrl/Alt/Super なし）はタイプ入力とみなす。SHIFT はあえて
/// 除外条件にしていない。Shift+G はフィルタに入力すべき文字 G そのものだから。
fn is_text_input_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// テキストフィルタも持つオーバーレイ（コマンドパレット、ファイル名検索など）の
/// リストナビゲーション。素の印字可能キーはフィルタ側に落ちるため、非テキスト
/// キー（矢印、PageUp/Down）だけがナビゲーションになる。j/k/g はタイプされる。
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
        // フィルタ可能なオーバーレイでは、これらはナビゲーションではなくタイプ入力として扱う。
        for c in ['j', 'k', 'g', 'G'] {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty());
            assert!(is_text_input_key(&key), "{c:?} should be text input");
        }
        // Shift は除外条件にならない。Shift+G はタイプすべき文字 'G' そのもの。
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert!(is_text_input_key(&shift_g));
    }

    #[test]
    fn modified_and_named_keys_are_not_text_input() {
        // これらはフィルタ可能なオーバーレイでもリストナビゲーションを駆動すべき。
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
