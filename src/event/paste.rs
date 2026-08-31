//! bracketed-paste イベントの処理。

use crate::app::App;

use super::input_target::InputTarget;

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
    // 通常のキーイベントとして届くので影響を受けない。
    if let Some(target) = InputTarget::active(app) {
        if target.is_multiline() {
            target.insert(app, &data);
        } else {
            let single_line: String = data.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            target.insert(app, &single_line);
        }
        return;
    }

    let session_idx = app
        .terminal
        .pane(app.focus.current())
        .and_then(|p| p.active_session);

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
            if let Some(pane) = app.terminal.pane_mut(app.focus.current()) {
                pane.scroll = 0;
            }
            app.clear_cc_waiting_signal(idx);
        }
    }
}
