//! Terminal / PTY state management.
//!
//! Groups all PTY-related fields previously scattered in `App` into a
//! single `TerminalState` struct.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::pty_manager;
use crate::ui::common::PtyRenderCache;

// ---------------------------------------------------------------------------
// PermissionRequest hook input/output types (Claude Code native hooks)
// ---------------------------------------------------------------------------

/// A permission suggestion from Claude Code (e.g. "always allow this tool").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSuggestion {
    #[serde(rename = "type")]
    pub suggestion_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Catch-all for additional fields.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Input received from the PermissionRequest hook (JSON on HTTP POST body).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct HookPermissionInput {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub permission_suggestions: Vec<PermissionSuggestion>,
}

/// The decision part of the hook response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookPermissionDecision {
    pub behavior: String, // "allow" or "deny"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_permissions: Option<Vec<PermissionSuggestion>>,
}

/// The event-specific output wrapper.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSpecificOutput {
    pub hook_event_name: String,
    pub decision: HookPermissionDecision,
}

/// Full hook response JSON.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookPermissionResponse {
    pub hook_specific_output: HookSpecificOutput,
}

impl HookPermissionResponse {
    pub fn allow() -> Self {
        Self {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PermissionRequest".to_string(),
                decision: HookPermissionDecision {
                    behavior: "allow".to_string(),
                    message: None,
                    updated_input: None,
                    updated_permissions: None,
                },
            },
        }
    }

    pub fn allow_with_permissions(permissions: Vec<PermissionSuggestion>) -> Self {
        Self {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PermissionRequest".to_string(),
                decision: HookPermissionDecision {
                    behavior: "allow".to_string(),
                    message: None,
                    updated_input: None,
                    updated_permissions: if permissions.is_empty() {
                        None
                    } else {
                        Some(permissions)
                    },
                },
            },
        }
    }

    pub fn deny(reason: &str) -> Self {
        Self {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PermissionRequest".to_string(),
                decision: HookPermissionDecision {
                    behavior: "deny".to_string(),
                    message: Some(reason.to_string()),
                    updated_input: None,
                    updated_permissions: None,
                },
            },
        }
    }
}

// ---------------------------------------------------------------------------
// TerminalState
// ---------------------------------------------------------------------------

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
}

impl TerminalState {
    /// Create a new `TerminalState` with the given scrollback limits.
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
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
        }
    }
}
