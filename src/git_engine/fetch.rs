//! fetch のために git CLI をシェルアウトで呼び出す。
//!
//! libgit2 の組み込み credential 処理はよくある構成の多く(macOS Keychain、
//! gh auth、credential-manager-core など)をサポートしないため、それらを
//! すべて扱える実物の git バイナリに委譲する。

use anyhow::{Context, Result};

use super::GitEngine;

impl GitEngine {
    /// git CLI をシェルアウトして git fetch --prune origin を実行する。
    ///
    /// 注意: これはネットワーク I/O を行い、数秒ブロックする可能性がある。
    /// UI スレッドから呼んではならない — バックグラウンドスレッドを使うこと。
    pub fn fetch_origin(&self) -> Result<()> {
        self.run_git_fetch(&["--prune", "origin"])
    }

    /// git CLI をシェルアウトして git fetch origin <refspec> を実行する —
    /// fetch_origin の refspec を取る版で、すべてのリモート追跡ブランチを
    /// 同期するのではなく特定の ref(例えば PR の head: pull/123/head:pr-123)
    /// だけを取得するためのもの。
    ///
    /// 注意: これはネットワーク I/O を行い、数秒ブロックする可能性がある。
    /// UI スレッドから呼んではならない — バックグラウンドスレッドを使うこと。
    pub fn fetch_refspec(&self, refspec: &str) -> Result<()> {
        self.run_git_fetch(&["origin", refspec])
    }

    /// git fetch <args> をシェルアウトする。タイムアウトと出力の処理は
    /// fetch_origin と fetch_refspec で共有する。
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

        // バックグラウンドスレッドがハングしないようタイムアウト付きで待つ。
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // プロセスが終了した。
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
                    // まだ実行中。
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
