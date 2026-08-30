//! Claude terminal パネルの上に重なる、無限スクロールバック reflow
//! トランスクリプトビューのキー処理。

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;

/// reflow トランスクリプトビューがアクティブな間のキーイベントを処理する。
///
/// すべてのキーはここで消費され、PTY へは転送されない — reflow ビューは
/// 純粋な読み取り専用オーバーレイ。ナビゲーション:
///
/// * j / Down — 1行下へスクロール。
/// * k / Up — 1行上へスクロール。
/// * Ctrl-d / PageDown — 半ページ下へスクロール。
/// * Ctrl-u / PageUp — 半ページ上へスクロール。
/// * g / Home — 一番古いターン (先頭) へジャンプ。
/// * G / End — 一番新しいターン (最下部) へジャンプし、following を再開する。
/// * Esc — reflow ビューを閉じてライブ PTY へ戻る。
/// * 最下部での j / Down / PageDown — reflow を閉じる (ライブへ戻る)。
///
/// どの分岐も [ReflowView::follow](crate::reflow::ReflowView::follow) を維持する:
/// 上へ動くと解除され、最下部に着くと再度アタッチされる。このフラグは、後の
/// reflow が最新ターンへの再固定と読者の論理位置の復元のどちらを取るか判断
/// する際に参照するものなので、ここで古いままにしておくと、このビューが
/// 避けようとしている最下部への強制スナップが復活してしまう。
pub(crate) fn handle_reflow_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    use super::input::{at_bottom, clamp_scroll};
    use crossterm::event::KeyModifiers;

    let inner = app.reflow.last_inner_height as usize;
    let total = app.reflow.total_lines;
    let page: usize = (inner / 2).max(1);
    let bottom = at_bottom(app.reflow.scroll, total, inner);
    let old_scroll = app.reflow.scroll;

    match key.code {
        // 行スクロール
        KeyCode::Char('j') | KeyCode::Down => {
            if bottom {
                // 最下部 + 単発の down キー → ライブ PTY へ戻る退場スイープを開始。
                app.request_close_reflow();
                return None;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(1);
        }

        // ページスクロール
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if bottom {
                app.request_close_reflow();
                return None;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(page);
        }
        KeyCode::PageDown => {
            if bottom {
                app.request_close_reflow();
                return None;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(page);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(page);
        }
        KeyCode::PageUp => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(page);
        }

        // 先頭 / 最下部へジャンプ
        KeyCode::Char('g') | KeyCode::Home => {
            app.reflow.scroll = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            // ビューを離れることなく最新ターン (論理的な最下部) にスナップし、
            // following を再開して次のリサイズでもそこに留まるようにする。
            app.reflow_jump_to_latest();
            return None;
        }

        // 展開 / 折りたたみ
        // Claude Code 自身のトランスクリプトはツール結果や thinking ブロックを
        // 折りたたみ、ctrl+o で展開できる。conductor は同じキーを再利用するが、
        // 別の全画面ビューへ切り替えるのではなくその場で展開する。ビュー全体で
        // 1つのトグルであり、このパネルにはブロック単位のカーソルがないので
        // それより細かい粒度で狙うことはできない。
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reflow.expanded = !app.reflow.expanded;
            app.reflow.needs_rebuild = true;
            return None;
        }

        // 離脱
        KeyCode::Esc => {
            // ライブ PTY へ戻る前に退場スイープを再生する。
            app.request_close_reflow();
            return None;
        }

        _ => {} // それ以外のキーはすべて黙って消費する。
    }

    // 調整後に scroll をクランプする。上限は total - 1 ではなく total - inner
    // — 描画パスと at_bottom のロジックに合わせてある。
    app.reflow.scroll = clamp_scroll(app.reflow.scroll, total, inner);

    // アーム単位ではなく、実際に scroll が着地した位置から follow 状態を
    // 再導出する: どのキーで動いたかに関わらず、最新行が画面上にあれば
    // ちょうど following になる。(G/End は上で早期リターンしている —
    // そちらは自分でフラグをセットする。パネルの再計測を経て初めて
    // *到達可能* になる最下部でも following として扱う必要があるため。)
    app.reflow.follow = at_bottom(app.reflow.scroll, total, inner);

    // スクロールのたびに強制的にハードクリアする (synchronized output のおかげで
    // アトミックに表示される)。トランスクリプトは任意の Unicode であり、
    // terminal がカウントより幅広く描画するグリフがあると行がずれ、ratatui の
    // diff (自分のバッファ同士しか比較しない) では決して再描画されない古い
    // セルが残ることがある。ステップごとに再クリアすることで、スクロール後の
    // ビューをそうした残留物から守る。
    if app.reflow.scroll != old_scroll {
        app.terminal.needs_clear = true;
    }
    None
}
