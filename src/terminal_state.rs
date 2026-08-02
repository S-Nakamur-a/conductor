//! 端末 / PTY の状態管理。
//!
//! これまで App に散らばっていた PTY 関連のフィールドを、1 つの
//! TerminalState 構造体にまとめたもの。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::pty_manager;
use crate::ui::common::PtyRenderCache;

/// 2 つの端末パネル (Claude Code と Shell) をまとめた状態。
pub struct TerminalState {
    /// PTY セッションの管理。
    pub pty_manager: pty_manager::PtyManager,
    /// 現在の worktree でアクティブな Claude Code セッションの添字。
    pub active_claude_session: Option<usize>,
    /// 現在の worktree でアクティブな Shell セッションの添字。
    pub active_shell_session: Option<usize>,
    /// Claude 用 PTY の、最後に判明した内容領域のサイズ (行数, 桁数)。
    pub size_claude: (u16, u16),
    /// Shell 用 PTY の、最後に判明した内容領域のサイズ (行数, 桁数)。
    pub size_shell: (u16, u16),
    /// 埋め込みエディタ用 PTY に最後に適用した内容領域のサイズ (行数, 桁数)。
    /// sync_pty_sizes が実際に変化したときだけリサイズできるよう別に持つ。
    pub size_editor: (u16, u16),
    /// Claude Code 端末のスクロールバックのオフセット (0 = 最新表示)。
    pub scroll_claude: usize,
    /// Shell 端末のスクロールバックのオフセット (0 = 最新表示)。
    pub scroll_shell: usize,
    /// Claude 端末の描画結果のキャッシュ。
    pub cache_claude: PtyRenderCache,
    /// Shell 端末の描画結果のキャッシュ。
    pub cache_shell: PtyRenderCache,
    /// Claude Code セッションが作業中の worktree パス。
    pub cc_active_worktrees: HashSet<PathBuf>,
    /// Claude Code セッションがユーザーの入力を待っている worktree パス。
    pub cc_waiting_worktrees: HashSet<PathBuf>,
    /// 確認済みの待機状態。worktree パスから、ユーザーが通知を消した時点での
    /// PTY セッションの last_output_time へのマップ。
    pub cc_waiting_ack_time: HashMap<PathBuf, Instant>,
    /// Claude 端末の空白部分を最後にクリックした時刻 (ダブルクリック判定用)。
    pub claude_blank_last_click: Instant,
    /// Shell 端末の空白部分を最後にクリックした時刻 (ダブルクリック判定用)。
    pub shell_blank_last_click: Instant,
    /// 端末全体のクリアと再描画が必要なときに true にする。
    pub needs_clear: bool,
    /// 保留中のプロンプト: セッション添字 → プロンプト文字列。
    /// Claude Code セッションが入力待ちになった時点で書き込まれる。
    pub deferred_prompts: HashMap<usize, String>,
    /// PTY のリーダースレッドが Claude 端末向けの新しい出力を出したときに立つ。
    pub dirty_claude: bool,
    /// PTY のリーダースレッドが Shell 端末向けの新しい出力を出したときに立つ。
    pub dirty_shell: bool,
    /// Claude セッションのタブ列で最初に見えているタブの添字 (横スクロール)。
    pub claude_tab_scroll: usize,
    /// Shell セッションのタブ列で最初に見えているタブの添字 (横スクロール)。
    pub shell_tab_scroll: usize,
    /// 次の描画でアクティブなタブが見えるよう Claude のタブ列をずらす。
    pub claude_tab_reveal: bool,
    /// 次の描画でアクティブなタブが見えるよう Shell のタブ列をずらす。
    pub shell_tab_reveal: bool,
    /// Claude のタブ列のクリック可能領域。描画のたびに記録する。
    pub claude_tab_hits: Vec<crate::ui::tab_bar::TabHit>,
    /// Shell のタブ列のクリック可能領域。描画のたびに記録する。
    pub shell_tab_hits: Vec<crate::ui::tab_bar::TabHit>,
    /// Claude のタブ列のどの領域にポインタが乗っているか。描画を変えるのは
    /// Close だけだが、アクションごと保存しておくことで「ホバーが何を意味するか」を
    /// イベント側ではなく描画側が決められるようにしている。
    pub claude_tab_hover: Option<crate::ui::tab_bar::TabAction>,
    /// Shell のタブ列のどの領域にポインタが乗っているか。
    /// [Self::claude_tab_hover] を参照。
    pub shell_tab_hover: Option<crate::ui::tab_bar::TabAction>,
}

impl TerminalState {
    /// 指定したスクロールバック上限で TerminalState を作る。
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        Self {
            pty_manager: pty_manager::PtyManager::new(active_scrollback, inactive_scrollback),
            active_claude_session: None,
            active_shell_session: None,
            size_claude: (24, 80),
            size_shell: (6, 80),
            size_editor: (24, 80),
            scroll_claude: 0,
            scroll_shell: 0,
            cache_claude: Default::default(),
            cache_shell: Default::default(),
            cc_active_worktrees: HashSet::new(),
            cc_waiting_worktrees: HashSet::new(),
            cc_waiting_ack_time: HashMap::new(),
            claude_blank_last_click: Instant::now(),
            shell_blank_last_click: Instant::now(),
            needs_clear: false,
            deferred_prompts: HashMap::new(),
            dirty_claude: true,
            dirty_shell: true,
            claude_tab_scroll: 0,
            shell_tab_scroll: 0,
            claude_tab_reveal: false,
            shell_tab_reveal: false,
            claude_tab_hits: Vec::new(),
            shell_tab_hits: Vec::new(),
            claude_tab_hover: None,
            shell_tab_hover: None,
        }
    }

    /// Claude パネルの表示を添字 idx のセッションへ切り替える。
    ///
    /// PTY セッションをアクティブにし、アクティブな Claude セッションとして記録し、
    /// パネルのスクロール位置と描画キャッシュをリセットする。キャッシュのクリアは
    /// 必須で、これは全セッションで共有される 1 つのバッファだから。クリアしないと、
    /// 別のきっかけ (スクロール、新しい出力) でたまたま作り直されるまで、パネルは
    /// 前のセッションの内容を描き続けてしまう。作り直しの条件は
    /// ui::terminal_claude を参照。
    pub fn switch_claude_session(&mut self, idx: usize) {
        self.pty_manager.activate_session(idx);
        self.active_claude_session = Some(idx);
        self.scroll_claude = 0;
        self.cache_claude = PtyRenderCache::default();
        self.claude_tab_reveal = true;
    }

    /// Shell パネルの表示を添字 idx のセッションへ切り替える。
    ///
    /// [Self::switch_claude_session] の Shell 版。キャッシュを無効化する理由も同じ。
    pub fn switch_shell_session(&mut self, idx: usize) {
        self.pty_manager.activate_session(idx);
        self.active_shell_session = Some(idx);
        self.scroll_shell = 0;
        self.cache_shell = PtyRenderCache::default();
        self.shell_tab_reveal = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    /// switch_* がキャッシュをクリアすることを示すために、描画キャッシュへ種を仕込む。
    /// パネルが同期しなくなるバグを起こした古い状態、すなわち前に表示していた
    /// セッションから残った空でないキャッシュを再現する。
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
        term.scroll_claude = 42;
        term.cache_claude = stale_cache();

        // セッションは 1 つも無い。activate_session は寛容な no-op なので、
        // ここではパネル状態のリセットだけを切り離して検証できる。
        term.switch_claude_session(0);

        assert_eq!(term.active_claude_session, Some(0));
        assert_eq!(term.scroll_claude, 0);
        assert!(
            term.cache_claude.lines.is_empty(),
            "cache must be cleared so the render guard rebuilds for the new session"
        );
        assert_eq!(term.cache_claude.effective_offset, 0);
    }

    #[test]
    fn switch_shell_session_resets_scroll_and_cache() {
        let mut term = TerminalState::new(1000, 100);
        term.scroll_shell = 42;
        term.cache_shell = stale_cache();

        term.switch_shell_session(2);

        assert_eq!(term.active_shell_session, Some(2));
        assert_eq!(term.scroll_shell, 0);
        assert!(term.cache_shell.lines.is_empty());
        assert_eq!(term.cache_shell.effective_offset, 0);
    }

    #[test]
    fn switch_claude_session_leaves_shell_panel_untouched() {
        let mut term = TerminalState::new(1000, 100);
        term.scroll_shell = 5;
        term.cache_shell = stale_cache();

        term.switch_claude_session(0);

        // Claude パネルの切り替えが Shell パネルの状態を乱してはいけない。
        assert_eq!(term.scroll_shell, 5);
        assert!(!term.cache_shell.lines.is_empty());
    }
}
