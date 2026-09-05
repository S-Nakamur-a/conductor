//! git CLI による fetch。
//!
//! libgit2 の credential 処理は macOS Keychain、gh auth、credential-manager-core
//! などを扱えないので、fetch だけは実物の git に委ねる。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::GitEngine;

impl GitEngine {
    /// `git fetch --prune origin`。ネットワーク I/O を伴うので UI スレッドから呼ばないこと。
    pub fn fetch_origin(&self) -> Result<()> {
        self.run_git_fetch(&["--prune", "origin"])
    }

    /// `git fetch origin <refspec>`。PR の head (`pull/123/head:pr-123`) など特定の ref
    /// だけを取る。ネットワーク I/O を伴うので UI スレッドから呼ばないこと。
    pub fn fetch_refspec(&self, refspec: &str) -> Result<()> {
        self.run_git_fetch(&["origin", refspec])
    }

    fn run_git_fetch(&self, args: &[&str]) -> Result<()> {
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

        let timeout = Duration::from_secs(30);
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    log::debug!("run_git_fetch: success ({})", args.join(" "));
                    return Ok(());
                }
                Ok(Some(status)) => {
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
                Ok(None) if start.elapsed() > timeout => {
                    let _ = child.kill();
                    anyhow::bail!("git fetch {} timed out after {timeout:?}", args.join(" "));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => anyhow::bail!("failed to wait for git fetch {}: {e}", args.join(" ")),
            }
        }
    }
}
