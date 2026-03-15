//! Terminal / PTY state management.
//!
//! Groups all PTY-related fields previously scattered in `App` into a
//! single `TerminalState` struct.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use crate::pty_manager;
use crate::ui::common::PtyRenderCache;

/// A permission prompt that requires the user's decision.
pub struct PermissionRequest {
    /// PTY session index for the CC session.
    pub session_idx: usize,
    /// Tool name (e.g. "Bash", "Write").
    pub tool_name: String,
    /// Reason the AI judge could not decide.
    pub reason: String,
    /// The user message that triggered the tool call.
    pub user_message: String,
    /// Working directory of the CC session.
    pub cwd: PathBuf,
    /// When this request was created.
    pub created_at: Instant,
    /// PID of the osascript dialog process (if any), for killing on dismiss.
    pub dialog_pid: Option<Arc<Mutex<Option<u32>>>>,
}

/// Result from an OS dialog permission prompt.
pub struct PermissionDialogResult {
    /// PTY session index.
    pub session_idx: usize,
    /// Whether the user approved.
    pub approved: bool,
}

/// Result from `claude -p` permission judgment.
pub struct PermissionJudgeResult {
    /// PTY session index.
    pub session_idx: usize,
    /// The action: "approve", "deny", or "ask_user".
    pub action: String,
    /// Reason for the decision.
    pub reason: String,
    /// Tool name.
    pub tool_name: String,
    /// User message context.
    pub user_message: String,
    /// Working directory.
    pub cwd: PathBuf,
}

/// A permission judgment being processed by `claude -p`.
pub struct PermissionJudging {
    /// PTY session index.
    pub session_idx: usize,
    /// Tool name.
    pub tool_name: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// When the judgment started.
    pub started_at: Instant,
    /// PID of the `claude -p` process (for killing on cancel).
    pub pid: Arc<Mutex<Option<u32>>>,
}

/// Aggregated state for the dual terminal panels (Claude Code + Shell).
pub struct TerminalState {
    /// PTY session manager.
    pub pty_manager: pty_manager::PtyManager,
    /// Index of the active Claude Code session for the current worktree.
    pub active_claude_session: Option<usize>,
    /// Index of the active Shell session for the current worktree.
    pub active_shell_session: Option<usize>,
    /// Last known terminal content area size (rows, cols) for Claude PTY.
    pub size_claude: (u16, u16),
    /// Last known terminal content area size (rows, cols) for Shell PTY.
    pub size_shell: (u16, u16),
    /// Scrollback offset for the Claude Code terminal (0 = live view).
    pub scroll_claude: usize,
    /// Scrollback offset for the Shell terminal (0 = live view).
    pub scroll_shell: usize,
    /// Cached PTY render output for Claude terminal.
    pub cache_claude: PtyRenderCache,
    /// Cached PTY render output for Shell terminal.
    pub cache_shell: PtyRenderCache,
    /// Worktree paths whose Claude Code sessions are waiting for user input.
    pub cc_waiting_worktrees: HashSet<PathBuf>,
    /// Acknowledged waiting states — maps worktree path to the PTY session's
    /// `last_output_time` at the moment the user dismissed the notification.
    pub cc_waiting_ack_time: HashMap<PathBuf, Instant>,
    /// Timestamp of last click on Claude terminal blank area (for double-click detection).
    pub claude_blank_last_click: Instant,
    /// Timestamp of last click on Shell terminal blank area (for double-click detection).
    pub shell_blank_last_click: Instant,
    /// Set to `true` when a full terminal clear + redraw is needed.
    pub needs_clear: bool,
    /// Deferred prompts: session index → prompt text.
    /// Written once the CC session becomes ready (waiting for input).
    pub deferred_prompts: HashMap<usize, String>,
    /// Permission requests awaiting user decision (ask_user).
    pub permission_queue: Vec<PermissionRequest>,
    /// Currently selected index in the permission queue.
    pub permission_queue_selected: usize,
    /// Session IDs already processed (to prevent duplicate handling).
    pub permission_processed_sessions: HashSet<String>,
    /// Channel for receiving OS dialog results.
    pub permission_dialog_tx: mpsc::Sender<PermissionDialogResult>,
    /// Receiver end — polled in the main loop.
    pub permission_dialog_rx: mpsc::Receiver<PermissionDialogResult>,
    /// Currently running `claude -p` judgments (session_idx → state).
    pub permission_judging: Vec<PermissionJudging>,
    /// Channel for receiving `claude -p` judgment results.
    pub permission_judge_tx: mpsc::Sender<PermissionJudgeResult>,
    /// Receiver end — polled in the main loop.
    pub permission_judge_rx: mpsc::Receiver<PermissionJudgeResult>,
}

impl TerminalState {
    /// Create a new `TerminalState` with the given scrollback limits.
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        let (dialog_tx, dialog_rx) = mpsc::channel();
        let (judge_tx, judge_rx) = mpsc::channel();
        Self {
            pty_manager: pty_manager::PtyManager::new(active_scrollback, inactive_scrollback),
            active_claude_session: None,
            active_shell_session: None,
            size_claude: (24, 80),
            size_shell: (6, 80),
            scroll_claude: 0,
            scroll_shell: 0,
            cache_claude: Default::default(),
            cache_shell: Default::default(),
            cc_waiting_worktrees: HashSet::new(),
            cc_waiting_ack_time: HashMap::new(),
            claude_blank_last_click: Instant::now(),
            shell_blank_last_click: Instant::now(),
            needs_clear: false,
            deferred_prompts: HashMap::new(),
            permission_queue: Vec::new(),
            permission_queue_selected: 0,
            permission_processed_sessions: HashSet::new(),
            permission_dialog_tx: dialog_tx,
            permission_dialog_rx: dialog_rx,
            permission_judging: Vec::new(),
            permission_judge_tx: judge_tx,
            permission_judge_rx: judge_rx,
        }
    }
}
