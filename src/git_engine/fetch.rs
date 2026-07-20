//! Shelling out to the `git` CLI for `fetch`.
//!
//! libgit2's built-in credential handling doesn't support many common setups
//! (macOS Keychain, `gh auth`, credential-manager-core, etc.), so we delegate
//! to the real `git` binary which handles all of them.

use anyhow::{Context, Result};

use super::GitEngine;

impl GitEngine {
    // ── Fetch ────────────────────────────────────────────────────

    /// Run `git fetch --prune origin` by shelling out to the `git` CLI.
    ///
    /// NOTE: This performs network I/O and may block for several seconds.
    /// Do NOT call from the UI thread — use a background thread instead.
    pub fn fetch_origin(&self) -> Result<()> {
        self.run_git_fetch(&["--prune", "origin"])
    }

    /// Run `git fetch origin <refspec>` by shelling out to the `git` CLI —
    /// the refspec-taking sibling of `fetch_origin`, for pulling down a
    /// specific ref (e.g. a PR head: `pull/123/head:pr-123`) rather than
    /// syncing every remote-tracking branch.
    ///
    /// NOTE: This performs network I/O and may block for several seconds.
    /// Do NOT call from the UI thread — use a background thread instead.
    pub fn fetch_refspec(&self, refspec: &str) -> Result<()> {
        self.run_git_fetch(&["origin", refspec])
    }

    /// Shell out to `git fetch <args>`, with the timeout/output handling
    /// shared by `fetch_origin` and `fetch_refspec`.
    fn run_git_fetch(&self, args: &[&str]) -> Result<()> {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let cwd = self.repo.workdir().unwrap_or(self.repo.path());
        log::debug!(
            "run_git_fetch: running `git fetch {}` in {}",
            args.join(" "),
            cwd.display()
        );
        let mut child = Command::new("git")
            .arg("fetch")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn `git fetch`")?;

        // Wait with a timeout so we never hang the background thread.
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process exited.
                    if !status.success() {
                        let stderr = child
                            .stderr
                            .take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();
                        log::warn!("git fetch {} stderr: {stderr}", args.join(" "));
                        anyhow::bail!(
                            "git fetch {} failed (exit {}): {}",
                            args.join(" "),
                            status,
                            stderr.trim()
                        );
                    }
                    log::debug!("run_git_fetch: success ({})", args.join(" "));
                    return Ok(());
                }
                Ok(None) => {
                    // Still running.
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        anyhow::bail!("git fetch {} timed out after {timeout:?}", args.join(" "));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    anyhow::bail!("failed to wait for git fetch {}: {e}", args.join(" "));
                }
            }
        }
    }
}
