//! Worktree management state.
//!
//! Groups worktree creation/deletion UI state and background operation
//! channels, previously scattered across the `App` struct.

use std::sync::mpsc;

use crate::app::{GrabbedBranch, PendingWorktree, WorktreeInputMode, WorktreeOpResult};
use crate::text_input::TextInput;

/// Worktree management state.
pub struct WorktreeManager {
    /// Worktree creation/deletion dialog state machine.
    pub input_mode: WorktreeInputMode,
    /// Text buffer for worktree name input.
    pub input_buffer: TextInput,
    /// Timestamp of the last click on worktree blank space (for double-click detection).
    pub blank_last_click: std::time::Instant,
    /// Branch name entered in step 1, held while step 2 (base branch) is active.
    pub pending_branch: String,
    /// Full list of branches available as base for worktree creation.
    pub base_branch_list: Vec<String>,
    /// Currently selected index in the base branch picker.
    pub base_branch_selected: usize,
    /// Filter string for narrowing the base branch list.
    pub base_branch_filter: TextInput,
    /// Branch name pending deletion after worktree was removed.
    pub pending_delete_branch: String,
    /// Reason text for worktree skip modal (shown until Esc).
    pub skip_reason: Option<String>,
    /// Currently grabbed branch info (branch name + source worktree path).
    pub grabbed_branch: Option<GrabbedBranch>,
    /// Cached local branch list (refreshed with worktrees).
    pub local_branches: Vec<String>,
    /// Worktree operations currently running in background threads.
    pub pending_worktrees: Vec<PendingWorktree>,
    /// Sender for worktree operation results (lazily created).
    pub bg_worktree_tx: Option<mpsc::Sender<WorktreeOpResult>>,
    /// Receiver for worktree operation results.
    pub bg_worktree_rx: Option<mpsc::Receiver<WorktreeOpResult>>,

    // ── Smart Worktree ──────────────────────────────────────────
    /// Multi-line task description buffer for smart worktree creation.
    pub smart_description_buffer: TextInput,
}

impl Default for WorktreeManager {
    fn default() -> Self {
        Self {
            input_mode: WorktreeInputMode::Normal,
            input_buffer: TextInput::new(),
            blank_last_click: std::time::Instant::now(),
            pending_branch: String::new(),
            base_branch_list: Vec::new(),
            base_branch_selected: 0,
            base_branch_filter: TextInput::new(),
            pending_delete_branch: String::new(),
            skip_reason: None,
            grabbed_branch: None,
            local_branches: Vec::new(),
            pending_worktrees: Vec::new(),
            bg_worktree_tx: None,
            bg_worktree_rx: None,
            smart_description_buffer: TextInput::new_multiline(),
        }
    }
}
