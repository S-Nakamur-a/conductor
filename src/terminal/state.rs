//! 端末 / PTY の状態。Claude Code と Shell は同じものを持つので、1 枚ぶんを
//! [TerminalPane] にまとめ、[TerminalState] がそれを 2 つ並べる。

use crate::hit_map::ColumnSpans;
use crate::widget::click::ClickTracker;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::pty_manager;
use crate::terminal::render::pty::PtyRenderCache;
use crate::types::Focus;
use crate::ui::tab_bar::TabAction;

/// 端末パネル 1 枚ぶんの状態。
pub struct TerminalPane {
    pub kind: pty_manager::SessionKind,
    /// 現在の worktree で、このパネルが映しているセッションの添字。
    pub active_session: Option<usize>,
    /// 最後に判明した内容領域のサイズ (行数, 桁数)。
    pub size: (u16, u16),
    /// スクロールバックのオフセット。0 が最新。
    pub scroll: usize,
    pub cache: PtyRenderCache,
    /// 空白部分へのクリック。
    pub blank_clicks: ClickTracker,
    /// PTY のリーダースレッドがこのパネル向けの出力を出したときに立つ。
    pub dirty: bool,
    /// タブ列で最初に見えているタブの添字。
    pub tab_scroll: usize,
    pub tab_reveal: bool,
    /// タブ列のクリック可能領域。描画のたびに記録する。
    pub tab_hits: ColumnSpans<TabAction>,
    /// 描画を変えるのは Close だけだが、アクションごと持つことで「ホバーが何を
    /// 意味するか」をイベント側ではなく描画側が決められる。
    pub tab_hover: Option<TabAction>,
}

impl TerminalPane {
    fn new(kind: pty_manager::SessionKind, size: (u16, u16)) -> Self {
        Self {
            kind,
            active_session: None,
            size,
            scroll: 0,
            cache: PtyRenderCache::default(),
            blank_clicks: ClickTracker::default(),
            dirty: true,
            tab_scroll: 0,
            tab_reveal: false,
            tab_hits: ColumnSpans::default(),
            tab_hover: None,
        }
    }

    /// 表示を添字 idx のセッションへ切り替える。
    ///
    /// スクロール位置と描画キャッシュを落とすのが要点で、キャッシュは全セッションで
    /// 共有される 1 つのバッファだから。残したままだと、別のきっかけ (スクロール、
    /// 新しい出力) でたまたま作り直されるまで前のセッションの内容を描き続ける。
    /// 作り直しの条件は terminal::render::claude を参照。
    pub fn switch_to(&mut self, idx: usize) {
        self.active_session = Some(idx);
        self.scroll = 0;
        self.cache = PtyRenderCache::default();
        self.tab_reveal = true;
    }
}

/// 2 つの端末パネル (Claude Code と Shell) をまとめた状態。
pub struct TerminalState {
    /// PTY セッションの管理。
    pub pty_manager: pty_manager::PtyManager,
    /// Claude Code パネル。
    pub claude: TerminalPane,
    /// Shell パネル。
    pub shell: TerminalPane,
    /// 埋め込みエディタ用 PTY に最後に適用した内容領域のサイズ (行数, 桁数)。
    /// sync_pty_sizes が実際に変化したときだけリサイズできるよう別に持つ。
    pub size_editor: (u16, u16),
    /// Claude Code セッションが作業中の worktree パス。
    pub cc_active_worktrees: HashSet<PathBuf>,
    /// Claude Code セッションがユーザーの入力を待っている worktree パス。
    pub cc_waiting_worktrees: HashSet<PathBuf>,
    /// 確認済みの待機状態。worktree パスから、ユーザーが通知を消した時点での
    /// PTY セッションの last_output_time へのマップ。
    pub cc_waiting_ack_time: HashMap<PathBuf, Instant>,
    /// 端末全体のクリアと再描画が必要なときに true にする。
    pub needs_clear: bool,
    /// 保留中のプロンプト: セッション添字 → プロンプト文字列。
    /// Claude Code セッションが入力待ちになった時点で書き込まれる。
    pub deferred_prompts: HashMap<usize, String>,
}

impl TerminalState {
    /// 指定したスクロールバック上限で TerminalState を作る。
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        Self {
            pty_manager: pty_manager::PtyManager::new(active_scrollback, inactive_scrollback),
            claude: TerminalPane::new(pty_manager::SessionKind::ClaudeCode, (24, 80)),
            shell: TerminalPane::new(pty_manager::SessionKind::Shell, (6, 80)),
            size_editor: (24, 80),
            cc_active_worktrees: HashSet::new(),
            cc_waiting_worktrees: HashSet::new(),
            cc_waiting_ack_time: HashMap::new(),
            needs_clear: false,
            deferred_prompts: HashMap::new(),
        }
    }

    /// focus が指す端末パネル。端末以外にフォーカスがあれば `None`。
    pub fn pane(&self, focus: Focus) -> Option<&TerminalPane> {
        match focus {
            Focus::TerminalClaude => Some(&self.claude),
            Focus::TerminalShell => Some(&self.shell),
            _ => None,
        }
    }

    /// [Self::pane] の可変版。
    pub fn pane_mut(&mut self, focus: Focus) -> Option<&mut TerminalPane> {
        match focus {
            Focus::TerminalClaude => Some(&mut self.claude),
            Focus::TerminalShell => Some(&mut self.shell),
            _ => None,
        }
    }

    /// 両方のパネル。順序は Claude、Shell。
    pub fn panes_mut(&mut self) -> [&mut TerminalPane; 2] {
        [&mut self.claude, &mut self.shell]
    }

    /// フォーカスされていないパネルの描画キャッシュを捨てる。
    ///
    /// キャッシュは全セッションで共有される 1 つのバッファを写したものなので、
    /// 見えていないパネルのぶんを残しておくと、戻ってきたときに古い内容が出る。
    pub fn drop_inactive_caches(&mut self, focus: Focus) {
        if focus != Focus::TerminalClaude {
            self.claude.cache = PtyRenderCache::default();
        }
        if focus != Focus::TerminalShell {
            self.shell.cache = PtyRenderCache::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    /// パネルが同期しなくなるバグを起こした古い状態、つまり前に表示していたセッションから
    /// 残った空でないキャッシュを再現する。
    fn stale_cache() -> PtyRenderCache {
        PtyRenderCache {
            lines: vec![Line::from("previous session output")],
            effective_offset: 7,
            cursor_position: Some((3, 4)),
        }
    }

    #[test]
    fn switch_claude_session_resets_scroll_and_cache() {
        let mut term = TerminalState::new(1000, 100);
        term.claude.scroll = 42;
        term.claude.cache = stale_cache();

        term.claude.switch_to(0);

        assert_eq!(term.claude.active_session, Some(0));
        assert_eq!(term.claude.scroll, 0);
        assert!(
            term.claude.cache.lines.is_empty(),
            "cache must be cleared so the render guard rebuilds for the new session"
        );
        assert_eq!(term.claude.cache.effective_offset, 0);
    }

    #[test]
    fn switch_shell_session_resets_scroll_and_cache() {
        let mut term = TerminalState::new(1000, 100);
        term.shell.scroll = 42;
        term.shell.cache = stale_cache();

        term.shell.switch_to(2);

        assert_eq!(term.shell.active_session, Some(2));
        assert_eq!(term.shell.scroll, 0);
        assert!(term.shell.cache.lines.is_empty());
        assert_eq!(term.shell.cache.effective_offset, 0);
    }

    #[test]
    fn switch_claude_session_leaves_shell_panel_untouched() {
        let mut term = TerminalState::new(1000, 100);
        term.shell.scroll = 5;
        term.shell.cache = stale_cache();

        term.claude.switch_to(0);

        // Claude パネルの切り替えが Shell パネルの状態を乱してはいけない。
        assert_eq!(term.shell.scroll, 5);
        assert!(!term.shell.cache.lines.is_empty());
    }
}
