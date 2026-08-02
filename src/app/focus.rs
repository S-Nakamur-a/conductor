//! パネルフォーカスモデル: どのパネルがキーボードフォーカスを持つか、
//! フォーカス循環の順序、フォーカス変化に伴う境界線の色のグライドアニメーション。

use super::App;

/// 現在キーボードフォーカスを持っているパネル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Worktree,
    Explorer,
    Viewer,
    TerminalClaude,
    TerminalShell,
    /// 埋め込みエディタパネル（PTY内のvim/emacs）。マージされた
    /// Explorer+Viewer領域を占有する。[App::editor] がSomeの間のみ到達可能。
    Editor,
}

impl Focus {
    /// このパネルの基本キーマップコンテキスト。両方のターミナルは
    /// Terminalコンテキストを共有する（diff/コメントリストのようなサブ
    /// モードはパネル自身が別に追跡する）。
    pub fn key_context(self) -> crate::keymap::KeyContext {
        use crate::keymap::KeyContext;
        match self {
            Focus::Worktree => KeyContext::Worktree,
            Focus::Explorer => KeyContext::Explorer,
            Focus::Viewer => KeyContext::Viewer,
            Focus::TerminalClaude | Focus::TerminalShell => KeyContext::Terminal,
            Focus::Editor => KeyContext::Editor,
        }
    }

    /// このパネルがPTYをホストし、その内部のプログラム（Claude Code、
    /// シェル、エディタ）が生のキー入力を受け取るべきかどうか。イベント
    /// ディスパッチャはこれらのパネルをPTY転送経路に通す。キーマップが
    /// 奪い返すのは、[ターミナル内で発火する](crate::keymap::Action) コード
    /// だけだ。
    pub fn is_pty(self) -> bool {
        matches!(
            self,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        )
    }
}

impl App {
    /// パネルにフォーカスをセットする。必要になった時点でデータを遅延読み込みする。
    pub fn set_focus(&mut self, mut focus: Focus) {
        // 埋め込みエディタがマージされたExplorer+Viewer領域を占有している間、
        // その2つのパネルは隠れる — それらへのフォーカス要求はすべて代わりに
        // エディタへ着地する。このリダイレクトをここに集約することで、
        // フォーカスに至るあらゆる経路（Tab循環、alt+数字、クリック、
        // パレット）が、それぞれエディタの存在を知らなくてもこの不変条件を
        // 守れる。
        if self.editor.is_some() && matches!(focus, Focus::Explorer | Focus::Viewer) {
            focus = Focus::Editor;
        }

        // worktree列はモニタストリップ＋切り替えモーダルになったので、
        // 「worktreeへフォーカス」は今ではそのモーダルを開き、フォーカスは
        // 元の場所に残す。これはworktreeへのあらゆるトリガーが通る唯一の
        // 関所である（Tabはもうworktreeへは到達せず、super+1/w/パレット/
        // クリックはすべてset_focus(Worktree)を呼ぶ）。
        if focus == Focus::Worktree {
            self.overlays.active = crate::overlay::ActiveOverlay::WorktreeSwitcher;
            return;
        }

        // 幅がゼロになってしまうパネルへフォーカスが移るとき、展開中のパネルを畳む。
        if let Some(expanded) = self.expanded_panel {
            let dominated = match expanded {
                Focus::TerminalClaude | Focus::TerminalShell => {
                    matches!(focus, Focus::TerminalClaude | Focus::TerminalShell)
                }
                other => other == focus,
            };
            if !dominated {
                self.expanded_panel = None;
            }
        }
        // 注意: 単なるフォーカス変更では、あえてここでreflowトランスクリプトを
        // 閉じない。キーハンドラ（event）もレンダラ（ui::terminal_claude）も
        // reflowをfocus == TerminalClaudeでゲートしているので、他のパネルに
        // フォーカスがある間はトランスクリプトはキーを捕まえず描画もされない
        // （Claudeパネルは生のPTYにフォールバックする）。ここで解体すると
        // スクロール位置もリセットされてしまい、ユーザーが他のパネルを
        // ちらっと見ただけでライブの末尾に戻されてしまう。reflowは、
        // トランスクリプトが古くなる遷移 — セッション切り替え
        // (switch_claude_session) とworktree変更 (on_worktree_changed) —
        // と、reflowキーハンドラでのEsc/F4では引き続き閉じられる。

        match focus {
            Focus::Explorer | Focus::Viewer => {
                if self.viewer_state.tree.file_tree.is_empty() {
                    self.refresh_viewer();
                }
                if self.diff_state.committed_files.is_empty()
                    && self.diff_state.uncommitted_files.is_empty()
                {
                    self.refresh_diff();
                }
            }
            Focus::TerminalClaude => {
                // ユーザーがターミナルパネルにフォーカスしたらCC待機シグナルを
                // クリアする。実際に入力したときだけでなく。
                if let Some(idx) = self.terminal.active_claude_session {
                    self.clear_cc_waiting_signal(idx);
                }
            }
            _ => {}
        }
        // パネルの一時的な検索プロンプトはそのパネルにモーダルなので、
        // フォーカスが離れたらキー捕捉を解放しなければならない。そうしないと
        // フォーカス移動後も検索ボックスがキー入力を食い続ける（例: viewer内で
        // /を押してからTabでClaudeへ — 入力はClaudeへ行くべき）。クエリと
        // マッチ結果は保持されるので、戻ってきたときにn/Nはまだ機能する。
        if focus != Focus::Viewer {
            self.viewer_state.search.search_active = false;
        }
        if matches!(
            focus,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        ) {
            self.viewer_state.filename_search.filename_search_active = false;
        }
        // 変化を記録し、フォーカスを得る/失うパネルが境界線の色をグライド
        // できるようにする（実際に変化した場合のみ。そうしないと再フォーカスで
        // アニメーションが再始動してしまう）。
        if self.focus != focus {
            self.focus_prev = self.focus;
            self.focus_changed_at = std::time::Instant::now();
        }
        self.focus = focus;
    }

    /// パネルの境界線の色。フォーカス変化にまたがってイージングする:
    /// フォーカスを得るパネルはborder_unfocused → border_focusedへ
    /// グライドし、失うパネルは逆方向にグライドする。これをanim::FOCUS_MS
    /// にわたって行う。それ以外はすべて静的な非フォーカス色のままとどまる。
    /// これが、テーマのRGB色とTheme::lerpを使って、パネル切り替えをぱっと
    /// 切り替わるのではなく滑らかに感じさせている要因。
    pub fn animated_border_color(&self, panel: Focus) -> ratatui::style::Color {
        let t = crate::anim::eased_progress(self.focus_changed_at.elapsed(), crate::anim::FOCUS_MS);
        if self.focus == panel {
            if t >= 1.0 {
                self.theme.border_focused
            } else {
                crate::theme::Theme::lerp(self.theme.border_unfocused, self.theme.border_focused, t)
            }
        } else if self.focus_prev == panel && t < 1.0 {
            crate::theme::Theme::lerp(self.theme.border_focused, self.theme.border_unfocused, t)
        } else {
            self.theme.border_unfocused
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
        self.focus_changed_at.elapsed() < std::time::Duration::from_millis(crate::anim::FOCUS_MS)
            || self.list_hover.is_animating()
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
            && self.focus == Focus::Explorer
            && !self.viewer_state.explorer.explorer_focus_on_diff_list
        {
            self.viewer_state.explorer.explorer_focus_on_diff_list = true;
            self.focus_changed_at = std::time::Instant::now();
            return;
        }
        let next = match self.focus {
            Focus::Worktree | Focus::TerminalShell => Focus::Explorer,
            Focus::Explorer => Focus::Viewer,
            Focus::Viewer => Focus::TerminalClaude,
            Focus::Editor => Focus::TerminalClaude,
            Focus::TerminalClaude => Focus::TerminalShell,
        };
        // 他のどこからであれExplorer列に着地したときは、常にファイルツリー
        // （上のパネル）から始まる。
        if next == Focus::Explorer {
            self.viewer_state.explorer.explorer_focus_on_diff_list = false;
        }
        self.set_focus(next);
    }

    /// フォーカスを後方に循環させる。
    pub fn cycle_focus_backward(&mut self) {
        // 前方循環の鏡像: Explorer列を逆向きに歩くと変更ファイルの次に
        // ファイルツリーを訪れる。
        if self.editor.is_none()
            && self.focus == Focus::Explorer
            && self.viewer_state.explorer.explorer_focus_on_diff_list
        {
            self.viewer_state.explorer.explorer_focus_on_diff_list = false;
            self.focus_changed_at = std::time::Instant::now();
            return;
        }
        let prev = match self.focus {
            Focus::Worktree | Focus::Explorer => Focus::TerminalShell,
            Focus::Viewer => Focus::Explorer,
            Focus::Editor => Focus::TerminalShell,
            Focus::TerminalClaude => Focus::Viewer,
            Focus::TerminalShell => Focus::TerminalClaude,
        };
        // Viewer側からExplorer列に入ると、（一番近い）変更ファイルパネルに
        // 着地するので、さらにTabで戻るとツリーに到達する。
        if prev == Focus::Explorer {
            self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        }
        self.set_focus(prev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_is_pty_only_for_pty_panels() {
        assert!(Focus::TerminalClaude.is_pty());
        assert!(Focus::TerminalShell.is_pty());
        assert!(Focus::Editor.is_pty());
        assert!(!Focus::Worktree.is_pty());
        assert!(!Focus::Explorer.is_pty());
        assert!(!Focus::Viewer.is_pty());
    }

    #[test]
    fn editor_focus_uses_editor_keymap_context() {
        assert_eq!(
            Focus::Editor.key_context(),
            crate::keymap::KeyContext::Editor
        );
    }
}
