//! Spawning PTY sessions: building the Claude Code / Shell / Editor command
//! lines and wiring up the reader thread, vt100 parser, and shared buffers
//! that back a new [`PtySession`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, PtySystem};
use uuid::Uuid;

use super::locale::utf8_locale_overrides;
use super::{PtyManager, PtySession, SessionKind};

impl PtyManager {
    /// Spawn a new PTY session and return its index in the session list.
    ///
    /// * `kind` — whether to launch Claude Code or a shell.
    /// * `worktree` — the worktree name this session belongs to.
    /// * `label` — a human-readable label shown in the UI.
    /// * `shell_path` — path to the shell binary (used only for `SessionKind::Shell`).
    /// * `working_dir` — the working directory for the spawned process.
    /// * `rows` — number of rows for the PTY and vt100 parser.
    /// * `cols` — number of columns for the PTY and vt100 parser.
    /// * `resume_session_id` — if `Some`, pass `--resume <id>` to the Claude CLI.
    /// * `repo_root` — the repository root path, used to set `CONDUCTOR_DB_PATH`
    ///   for Claude Code sessions so the MCP server can locate the database.
    /// * `session_name` — if `Some`, pass `--name <name>` to the Claude CLI.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_session(
        &mut self,
        kind: SessionKind,
        worktree: &str,
        label: &str,
        shell_path: &str,
        working_dir: &PathBuf,
        rows: u16,
        cols: u16,
        resume_session_id: Option<&str>,
        repo_root: &Path,
        session_name: Option<&str>,
    ) -> Result<usize> {
        // Build the command depending on the session kind, then hand off to the
        // shared spawn path. For Claude sessions we also pin down the session id
        // so the reflow transcript view can later open this exact panel's log.
        let (cmd, claude_session_id) = match kind {
            SessionKind::ClaudeCode => {
                let mut c = CommandBuilder::new("claude");
                // Resume keeps the existing session id (Claude appends to the
                // same `<id>.jsonl`); a fresh spawn forces a generated id via
                // `--session-id` so we know the file name up front instead of
                // having to guess "the worktree's most recent session".
                let session_id = if let Some(resume_id) = resume_session_id {
                    c.arg("--resume");
                    c.arg(resume_id);
                    resume_id.to_string()
                } else {
                    let new_id = Uuid::new_v4().to_string();
                    c.arg("--session-id");
                    c.arg(&new_id);
                    new_id
                };
                if let Some(name) = session_name {
                    c.arg("--name");
                    c.arg(name);
                }
                // Let the conductor MCP server find the review database.
                let db_path = repo_root.join(".conductor").join("conductor.db");
                c.env("CONDUCTOR_DB_PATH", db_path);
                (c, Some(session_id))
            }
            SessionKind::Shell => (CommandBuilder::new(shell_path), None),
            SessionKind::Editor => {
                unreachable!("editor sessions are spawned via spawn_editor_session")
            }
        };
        let idx = self.finish_spawn(kind, worktree, label, working_dir, rows, cols, cmd)?;
        if let Some(session) = self.sessions.get_mut(idx) {
            session.claude_session_id = claude_session_id;
        }
        Ok(idx)
    }

    /// Spawn an external editor (`$VISUAL` / `$EDITOR`) on a single `file` as a
    /// transient PTY session. `program` + `args` is the resolved editor command
    /// line (already split into program and arguments); `file` is appended as
    /// the final argument.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_editor_session(
        &mut self,
        worktree: &str,
        label: &str,
        working_dir: &PathBuf,
        rows: u16,
        cols: u16,
        program: &str,
        args: &[String],
        file: &Path,
    ) -> Result<usize> {
        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.arg(file);

        // Ensure the editor sees a UTF-8 locale. `CommandBuilder` inherits the
        // parent environment, so when Conductor is launched without a UTF-8
        // locale (a bare login shell, cron, an SSH session forwarding `LANG=C`,
        // …) the editor inherits that too — and terminal editors like vim then
        // fall back to `encoding=latin1`, mangling full-width / multi-byte input.
        let (locale_sets, locale_removes) = utf8_locale_overrides(
            std::env::var("LC_ALL").ok().as_deref(),
            std::env::var("LC_CTYPE").ok().as_deref(),
            std::env::var("LANG").ok().as_deref(),
        );
        for (key, value) in &locale_sets {
            cmd.env(key, value);
        }
        for key in &locale_removes {
            cmd.env_remove(key);
        }

        self.finish_spawn(SessionKind::Editor, worktree, label, working_dir, rows, cols, cmd)
    }

    /// Shared tail of the spawn path: open the PTY pair, wire the reader thread
    /// and vt100 parser, and push the session. `cmd` is the fully built command
    /// (its working directory is set here).
    #[allow(clippy::too_many_arguments)]
    fn finish_spawn(
        &mut self,
        kind: SessionKind,
        worktree: &str,
        label: &str,
        working_dir: &PathBuf,
        rows: u16,
        cols: u16,
        mut cmd: CommandBuilder,
    ) -> Result<usize> {
        // 1. Open a new PTY pair with the given size.
        let pair = self
            .pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY pair")?;

        cmd.cwd(working_dir);

        // 3. Spawn the child process on the slave end.
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn command in PTY")?;

        // 4. Obtain reader and writer handles from the master end.
        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("Failed to take PTY writer")?,
        ));
        let writer_for_thread = Arc::clone(&writer);

        // 5. Set up the shared output buffer.
        let output_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let max_buffer_lines = self.inactive_scrollback;

        // 5b. Create the vt100 parser with the same size as the PTY.
        let screen: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(
            rows,
            cols,
            self.inactive_scrollback,
        )));

        // 5c. Raw byte history for reflow-on-resize. Only recorded for sessions
        // whose output can actually be reflowed by replay (see `raw_history`
        // field docs): ordinary shells, which rely on terminal autowrap. Claude
        // and the transient editor repaint in place at a fixed width, so replay
        // would cost memory without ever reflowing — skip it for them.
        let raw_history: Option<Arc<Mutex<VecDeque<u8>>>> = matches!(kind, SessionKind::Shell)
            .then(|| Arc::new(Mutex::new(VecDeque::new())));

        // 6. Spawn a background thread that continuously reads PTY output.
        let buffer_clone = Arc::clone(&output_buffer);
        let screen_clone = Arc::clone(&screen);
        let raw_history_clone = raw_history.clone();
        // We store max_buffer_lines in the session, but the reader thread
        // needs its own reference. We use a separate Arc<Mutex<usize>> so
        // that set_active() can dynamically adjust the limit.
        let buffer_limit = Arc::new(Mutex::new(max_buffer_lines));
        let buffer_limit_for_thread = Arc::clone(&buffer_limit);

        // Track when the last output was received (for input-waiting detection).
        let last_output_time = Arc::new(Mutex::new(Instant::now()));
        let last_output_time_for_thread = Arc::clone(&last_output_time);

        // Track alternate-screen transitions so the main loop can nudge
        // programs (e.g. fzf) that may not have rendered their initial UI.
        let alt_screen_entered = Arc::new(AtomicBool::new(false));
        let alt_screen_entered_for_thread = Arc::clone(&alt_screen_entered);

        let output_notify_for_thread = Arc::clone(&self.output_notify);

        thread::Builder::new()
            .name(format!("pty-reader-{label}"))
            .spawn(move || {
                Self::reader_thread(
                    reader,
                    buffer_clone,
                    buffer_limit_for_thread,
                    screen_clone,
                    raw_history_clone,
                    last_output_time_for_thread,
                    alt_screen_entered_for_thread,
                    writer_for_thread,
                    output_notify_for_thread,
                );
            })
            .context("Failed to spawn PTY reader thread")?;

        // 7. Build the session struct.
        let session = PtySession {
            id: Uuid::new_v4().to_string(),
            label: label.to_string(),
            kind,
            worktree: worktree.to_string(),
            working_dir: working_dir.clone(),
            // Populated by `spawn_session` for Claude panels after this returns;
            // Shell/Editor sessions keep it `None`.
            claude_session_id: None,
            is_active: false,
            master: pair.master,
            writer,
            child,
            output_buffer,
            max_buffer_lines,
            screen,
            raw_history,
            last_output_time,
            alt_screen_entered,
            alt_screen_nudge_until: None,
            last_nudge_time: None,
        };

        self.sessions.push(session);
        let idx = self.sessions.len() - 1;

        // Store the buffer limit Arc so that set_active() can dynamically
        // adjust it for the reader thread.
        self.buffer_limits.push(buffer_limit);

        Ok(idx)
    }
}
