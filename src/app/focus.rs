//! フォーカス変更に伴う App 側の処理: 遅延読み込み、フォーカス循環、
//! 境界線のグライドアニメーション。
//!
//! [Focus] 型そのものは [crate::types] にある。

use std::time::Instant;

use super::App;
use crate::types::Focus;

/// フォーカスと、その遷移の記録。
///
/// current が private なのは、直接代入すると [App::set_focus] のリダイレクトも
/// 遷移時刻の更新も飛ばせてしまうため。実際に 1 箇所が飛ばしていた。
pub struct FocusState {
    current: Focus,
    prev: Focus,
    changed_at: Instant,
}

impl FocusState {
    /// 遷移演出が済んだ状態から始める。起動直後の 1 フレーム目で枠線が
    /// 動き出さないよう、時刻を演出の長さぶん過去に置く。
    pub fn settled(focus: Focus) -> Self {
        Self {
            current: focus,
            prev: focus,
            changed_at: Instant::now() - std::time::Duration::from_millis(crate::anim::FOCUS_MS),
        }
    }
}

impl FocusState {
    pub fn current(&self) -> Focus {
        self.current
    }

    pub fn prev(&self) -> Focus {
        self.prev
    }

    pub fn changed_at(&self) -> Instant {
        self.changed_at
    }

    /// 移す。再フォーカスで演出が再始動しないよう、実際に変わったときだけ記録する。
    ///
    /// 普段の入口は [App::set_focus] で、そちらは遅延読み込みとエディタへの
    /// リダイレクトも行う。ここを直に呼ぶのは、その副作用が困る解体経路だけ。
    pub(crate) fn enter(&mut self, next: Focus) {
        if self.current != next {
            self.prev = self.current;
            self.changed_at = Instant::now();
        }
        self.current = next;
    }

    /// 遷移せずに時刻だけ進める。Explorer の上下ペインの移動など、パネルは
    /// 変わらないがボーダーを引き直したいとき用。
    fn touch(&mut self) {
        self.changed_at = Instant::now();
    }
}

impl App {
    /// パネルにフォーカスをセットする。必要になった時点でデータを遅延読み込みする。
    pub fn set_focus(&mut self, mut focus: Focus) {
        // 埋め込みエディタが Explorer+Viewer 領域を占有している間、その 2 つへのフォーカス要求は
        // エディタへ着地する。ここに集約することで、フォーカスに至るあらゆる経路がエディタの
        // 存在を知らなくてもこの不変条件を守れる。
        if self.editor.is_some() && matches!(focus, Focus::Explorer | Focus::Viewer) {
            focus = Focus::Editor;
        }

        // 「worktree へフォーカス」は切り替えモーダルを開き、フォーカスは元の場所に残す。
        // worktree へのあらゆるトリガーが通る唯一の関所。
        if focus == Focus::Worktree {
            self.overlays.active = crate::overlay::ActiveOverlay::WorktreeSwitcher;
            return;
        }

        // 幅がゼロになってしまうパネルへフォーカスが移るとき、展開中のパネルを畳む。
        if let Some(expanded) = self.layout.expanded {
            let dominated = match expanded {
                Focus::TerminalClaude | Focus::TerminalShell => {
                    matches!(focus, Focus::TerminalClaude | Focus::TerminalShell)
                }
                other => other == focus,
            };
            if !dominated {
                self.layout.expanded = None;
            }
        }
        // 単なるフォーカス変更では reflow トランスクリプトを閉じない。キーハンドラもレンダラも
        // focus == TerminalClaude でゲートしているので捕まらず描かれない。ここで解体すると
        // スクロール位置もリセットされ、他のパネルをちらっと見ただけでライブの末尾に戻される。
        // トランスクリプトが古くなる遷移 (セッション切り替え・worktree 変更) と Esc/F4 では閉じる。

        match focus {
            Focus::Explorer | Focus::Viewer => {
                if self.explorer.tree.file_tree.is_empty() {
                    self.refresh_viewer();
                }
                if self.diff_state.files.is_empty() {
                    self.refresh_diff();
                }
            }
            Focus::TerminalClaude => {
                // 実際に入力したときだけでなく、ターミナルパネルにフォーカスした時点で CC 待機シグナルを消す。
                if let Some(idx) = self.terminal.claude.active_session {
                    self.clear_cc_waiting_signal(idx);
                }
            }
            _ => {}
        }
        // パネルの検索プロンプトはそのパネルにモーダルなので、フォーカスが離れたらキー捕捉を
        // 解放する。しないと、移動後も検索ボックスがキー入力を食い続ける。
        if focus != Focus::Viewer {
            self.viewer.search.search_active = false;
        }
        if matches!(
            focus,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        ) {
            self.viewer.filename_search.filename_search_active = false;
        }
        self.focus.enter(focus);
    }

    /// パネルの境界線の色。フォーカス変化にまたがってイージングする:
    /// フォーカスを得るパネルはborder_unfocused → border_focusedへ
    /// グライドし、失うパネルは逆方向にグライドする。これをanim::FOCUS_MS
    /// にわたって行う。それ以外はすべて静的な非フォーカス色のままとどまる。
    /// これが、テーマのRGB色とTheme::lerpを使って、パネル切り替えをぱっと
    /// 切り替わるのではなく滑らかに感じさせている要因。
    pub fn animated_border_color(&self, panel: Focus) -> ratatui::style::Color {
        let t =
            crate::anim::eased_progress(self.focus.changed_at().elapsed(), crate::anim::FOCUS_MS);
        if self.focus.current() == panel {
            if t >= 1.0 {
                self.appearance.theme.border_focused
            } else {
                crate::theme::Theme::lerp(
                    self.appearance.theme.border_unfocused,
                    self.appearance.theme.border_focused,
                    t,
                )
            }
        } else if self.focus.prev() == panel && t < 1.0 {
            crate::theme::Theme::lerp(
                self.appearance.theme.border_focused,
                self.appearance.theme.border_unfocused,
                t,
            )
        } else {
            self.appearance.theme.border_unfocused
        }
    }

    /// UIの遷移（現状ではフォーカス境界線のグライド、または行ホバーの
    /// フェードアウト）がまだ進行中かどうか。メインループはこれを使い、
    /// 遷移がアイドル時のティックレートで止まらず実際にアニメーションする
    /// よう、アクティブなフレームレートで再描画し続ける。
    ///
    /// ホバーのフェードは今のところ偶然再描画されている。マウス移動が
    /// すでにlast_input_timeを更新しており、アイドル時のティックレートは
    /// 入力が無い状態がACTIVITY_TIMEOUT（500ms）続いた後にしか働かない
    /// ためで、これはフェードの120msより十分長い。しかしこれは無関係な
    /// 定数への偶然の依存にすぎない: もしACTIVITY_TIMEOUTが将来短縮
    /// されたら、フェードはこのコードとの目に見えるつながりのないまま
    /// 止まり始めてしまう。ここに組み込んでおくことで、フェードは自分自身の
    /// 条件でアニメーションするようになる。
    pub fn has_active_transition(&self) -> bool {
        self.focus.changed_at().elapsed() < std::time::Duration::from_millis(crate::anim::FOCUS_MS)
            || self.list_hover.is_animating()
            || self.entrance.is_animating()
    }

    // フォーカス循環

    /// フォーカスを前方に循環させる: Worktree → Explorer → Viewer → TerminalClaude → TerminalShell → Worktree
    pub fn cycle_focus_forward(&mut self) {
        // Worktreeはもうフォーカス可能な列ではない（上部ストリップ＋
        // 切り替えモーダルになった）ので、Tab循環からは除外されている。
        // エディタが開いているときは、循環の中でExplorer+Viewerの代わりを
        // 務める。set_focusがExplorer/Viewerへの指定をすべてエディタへ
        // リダイレクトするので、明示的に必要なのはエディタ自体を抜ける腕
        // だけだ。
        //
        // Explorer列は独立した2つのパネル — ファイルツリーと変更ファイル
        // 一覧 — を持つので、Tabはそれぞれを個別の停止点として訪れ
        // （ファイルツリー → 変更ファイル → Viewer）、次へ進む前にサブ
        // フォーカスを切り替える。
        if self.editor.is_none()
            && self.focus.current() == Focus::Explorer
            && !(self.explorer.focus() == crate::explorer::Pane::Bottom)
        {
            self.explorer.focus_pane(crate::explorer::Pane::Bottom);
            self.focus.touch();
            return;
        }
        let next = self.focus.current().next_in_cycle();
        // 他のどこからであれExplorer列に着地したときは、常にファイルツリー
        // （上のパネル）から始まる。
        if next == Focus::Explorer {
            self.explorer.focus_pane(crate::explorer::Pane::Tree);
        }
        self.set_focus(next);
    }

    /// フォーカスを後方に循環させる。
    pub fn cycle_focus_backward(&mut self) {
        // 前方循環の鏡像: Explorer列を逆向きに歩くと変更ファイルの次に
        // ファイルツリーを訪れる。
        if self.editor.is_none()
            && self.focus.current() == Focus::Explorer
            && (self.explorer.focus() == crate::explorer::Pane::Bottom)
        {
            self.explorer.focus_pane(crate::explorer::Pane::Tree);
            self.focus.touch();
            return;
        }
        let prev = self.focus.current().prev_in_cycle();
        // Viewer側からExplorer列に入ると、（一番近い）変更ファイルパネルに
        // 着地するので、さらにTabで戻るとツリーに到達する。
        if prev == Focus::Explorer {
            self.explorer.focus_pane(crate::explorer::Pane::Bottom);
        }
        self.set_focus(prev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ptyになるのはptyのパネルだけ() {
        assert!(Focus::TerminalClaude.is_pty());
        assert!(Focus::TerminalShell.is_pty());
        assert!(Focus::Editor.is_pty());
        assert!(!Focus::Worktree.is_pty());
        assert!(!Focus::Explorer.is_pty());
        assert!(!Focus::Viewer.is_pty());
    }

    #[test]
    fn editorのフォーカスはeditorのkeymap文脈を使う() {
        assert_eq!(
            Focus::Editor.key_context(),
            crate::keymap::KeyContext::Editor
        );
    }
}
