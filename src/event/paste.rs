//! bracketed-paste イベントの処理。

use crate::app::{App, Focus, WorktreeInputMode};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use super::is_text_input_active;

/// bracketed-paste イベントを処理する。テキスト入力のオーバーレイ/モーダルが
/// あればまずそちらがペーストを受け取る (これにより IME 確定済みのマルチ
/// バイトテキストが、モーダルの裏にいる terminal ではなく入力欄へ届く)。
/// そうでなくて terminal パネルがフォーカスされている場合は、ペースト全体を
/// 1回の書き込みで PTY へ転送する。bracketed-paste のエスケープシーケンスで
/// 包むことで、shell/アプリケーション側が個々のキー入力ではなく1回の
/// ペーストとして扱うようにする。
pub fn handle_paste_event(app: &mut App, data: String) {
    // テキスト入力のオーバーレイ/モーダルは、その裏でどのパネルがフォーカス
    // されていてもペーストを握る — handle_key_event の §0 がキーイベントに
    // 適用しているのと同じモーダルグラブ。これが重要なのは、macOS の
    // terminal は IME 確定済みのマルチバイトテキスト (かな漢字、特に2文字
    // 以上や変換を経たもの) を個々のキーイベントではなく bracketed paste
    // として届けるため。フォーカスだけをゲートにすると、そのペーストが
    // モーダルの裏にいる Claude/Shell の PTY へ転送されてしまい、入力した
    // 日本語が入力欄から消えて terminal 側に出てしまう。半角 ASCII は
    // 通常のキーイベントとして届くので影響を受けない。is_text_input_active
    // と歩調を合わせてある: 以下の宛先はすべてそちらにも列挙されている。
    if is_text_input_active(app) {
        // ペーストデータをアクティブなオーバーレイの入力バッファへ振り分ける。
        let single_line: String = data.chars().filter(|c| *c != '\n' && *c != '\r').collect();

        if app.viewer_state.explorer.inline_reply_line.is_some() {
            app.viewer_state
                .explorer
                .inline_reply_buffer
                .insert_str(&single_line);
        } else if app.review_state.input_mode != ReviewInputMode::Normal {
            // レビュー入力は複数行。
            app.review_state.input_buffer.insert_str(&data);
        } else if app.worktree_mgr.input_mode == WorktreeInputMode::SmartDescription {
            // スマート説明は複数行。
            app.worktree_mgr.smart_description_buffer.insert_str(&data);
        } else if app.worktree_mgr.input_mode == WorktreeInputMode::CreatingWorktree
            || app.worktree_mgr.input_mode == WorktreeInputMode::CreatingWorktreeBase
        {
            app.worktree_mgr.input_buffer.insert_str(&single_line);
        } else if app.overlays.active == ActiveOverlay::GrepSearch {
            app.overlays.grep_search.query.insert_str(&single_line);
            app.overlays.grep_search.input_focused = true;
            app.overlays.grep_search.schedule();
        } else if app.viewer_state.search.search_active {
            app.viewer_state
                .search
                .search_query
                .insert_str(&single_line);
        } else if app.viewer_state.filename_search.filename_search_active {
            app.viewer_state
                .filename_search
                .filename_search_query
                .insert_str(&single_line);
        } else if app.review_state.search_active {
            app.review_state.search_query.insert_str(&single_line);
            app.review_state.apply_filter();
        } else {
            match app.overlays.active {
                ActiveOverlay::SwitchBranch => {
                    app.overlays.switch_branch.filter.insert_str(&single_line);
                }
                ActiveOverlay::CommandPalette => {
                    app.overlays.command_palette.filter.insert_str(&single_line);
                }
                ActiveOverlay::OpenRepo => {
                    app.overlays.open_repo.buffer.insert_str(&single_line);
                }
                ActiveOverlay::PrInput => {
                    app.overlays.pr_input.buffer.insert_str(&single_line);
                    app.overlays.pr_input.error = None;
                }
                ActiveOverlay::History => {
                    app.overlays.history.search_query.insert_str(&single_line);
                }
                ActiveOverlay::ResumeSession => {
                    app.overlays.resume_session.filter.insert_str(&single_line);
                }
                _ => {}
            }
        }
        return;
    }

    let session_idx = match app.focus {
        Focus::TerminalClaude => app.terminal.active_claude_session,
        Focus::TerminalShell => app.terminal.active_shell_session,
        _ => None,
    };

    // grab されている worktree の terminal へのペーストはブロックする。
    if app.is_selected_worktree_grabbed() {
        return;
    }

    if let Some(idx) = session_idx {
        // 大きなペーストがカーネルの PTY 入力バッファを溢れさせないよう、
        // bracketed-paste で包んだチャンク書き込みを使う。
        if let Err(e) = app.terminal.pty_manager.write_paste_to_session(idx, &data) {
            log::warn!("failed to write paste data to PTY session: {e}");
        } else {
            match app.focus {
                Focus::TerminalClaude => app.terminal.scroll_claude = 0,
                Focus::TerminalShell => app.terminal.scroll_shell = 0,
                _ => {}
            }
            app.clear_cc_waiting_signal(idx);
        }
    }
}
