//! In-app self-update flow: checking GitHub Releases, downloading and
//! installing a pre-built binary, and polling background operations for the
//! main event loop.

use std::sync::mpsc;

use super::{App, StatusLevel};

/// State of the in-app update flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateState {
    /// Normal operation — no update in progress.
    #[default]
    Idle,
    /// Confirmation dialog is shown.
    Confirming,
    /// Download & build running in background thread.
    InProgress,
    /// About to restart the process.
    Restarting,
    /// An error occurred — message shown until dismissed.
    Failed,
}

/// Messages sent from the background update thread.
#[derive(Debug, Clone)]
pub enum UpdateProgress {
    /// Intermediate status message.
    Status(String),
    /// Update completed successfully.
    Done(String),
    /// Update failed with an error message.
    Error(String),
}

impl App {
    pub(super) fn cmd_update_and_restart(&mut self) {
        if self.update.info.is_some() {
            self.start_update_confirm();
        } else {
            self.set_status("No update available.".to_string(), StatusLevel::Info);
        }
    }

    /// Manually check GitHub Releases for a newer version, on demand. Unlike the
    /// silent startup/interval check, this flashes explicit feedback for every
    /// outcome (update available / already current / check failed) when the
    /// background result lands in [`poll_all_background_ops`](Self::poll_all_background_ops).
    pub(super) fn cmd_check_for_update(&mut self) {
        self.update.check_requested = true;
        self.set_status_info(format!(
            "Checking for updates… (current v{})",
            crate::update_checker::current_version()
        ));
        self.bg.update_check.start(|tx| {
            let _ = tx.send(crate::update_checker::check_for_update());
        });
    }

    /// Show the update confirmation dialog.
    pub fn start_update_confirm(&mut self) {
        self.update.state = UpdateState::Confirming;
    }

    /// Kick off the background update thread.
    pub fn start_update_download(&mut self) {
        let Some(ref info) = self.update.info else {
            return;
        };
        let version = info.latest_version.clone();
        let assets = info.assets.clone();

        self.update.state = UpdateState::InProgress;
        self.update.progress_message = "Preparing update...".to_string();

        self.update.op.start(move |tx| {
            perform_update(&tx, &version, &assets);
        });
    }

    /// Poll for progress messages from the background update thread.
    pub fn poll_update_progress(&mut self) {
        for msg in self.update.op.poll_all() {
            match msg {
                UpdateProgress::Status(s) => {
                    self.update.progress_message = s;
                }
                UpdateProgress::Done(s) => {
                    self.update.progress_message = s;
                    self.update.state = UpdateState::Restarting;
                    self.update.should_restart = true;
                    self.should_quit = true;
                }
                UpdateProgress::Error(s) => {
                    self.update.progress_message = s;
                    self.update.state = UpdateState::Failed;
                }
            }
        }
    }

    /// Poll all background operations and apply their results.
    ///
    /// Consolidates the scattered `poll_*()` calls that were previously
    /// spread across `run_loop()` in `main.rs`.
    pub fn poll_all_background_ops(&mut self) {
        self.poll_bg_branches();
        self.poll_bg_pull();
        self.poll_grep_search();
        self.poll_update_progress();
        self.poll_reflow_load();
        self.poll_pr_url();
        self.poll_worktree_switch_ops();
        self.poll_worktree_ops();
        self.poll_pr_intake();
        self.poll_walkthrough_generation();
        self.poll_publish_review();

        // ccusage
        if let Some(info) = self.bg.ccusage.poll() {
            self.stats.ccusage = Some(info);
        }

        // symbol index
        if let Some(result) = self.bg.symbol_index.poll() {
            match result {
                Ok(count) => {
                    log::info!("Symbol index built: {count} symbols");
                    self.set_status(
                        format!("Symbol index ready ({count} symbols)"),
                        StatusLevel::Success,
                    );
                }
                Err(msg) => {
                    log::warn!("Symbol index build failed: {msg}");
                }
            }
            // The root can move while a build is walking the old one, and
            // `start_symbol_index_build` declines to pile a second build on
            // top of a running one. That combination leaves the finished build
            // discarding its own result with nothing queued behind it, so the
            // index would sit empty until the next filesystem event. Kicking
            // off the catch-up build here is what closes that gap: by now the
            // slot is free, and if the root never moved this is a no-op
            // because the index is already marked available.
            if !self.code_nav.index.is_available() {
                self.start_symbol_index_build();
            }
        }

        // update check. The outer Option is "a result is ready"; the inner is
        // the check itself (Some(info) on success, None on network/parse error).
        if let Some(result) = self.bg.update_check.poll() {
            // Whether the user asked for explicit feedback this round.
            let requested = std::mem::take(&mut self.update.check_requested);
            let current = crate::update_checker::current_version();
            match result {
                Some(info) => {
                    if crate::update_checker::is_newer(&info.latest_version, current) {
                        if requested {
                            self.set_status(
                                format!(
                                    "Update available: v{} — run “App: Update and Restart”",
                                    info.latest_version
                                ),
                                StatusLevel::Success,
                            );
                        }
                        self.update.info = Some(info);
                    } else if requested {
                        self.set_status(
                            format!("Already up to date (v{current})"),
                            StatusLevel::Info,
                        );
                    }
                }
                None => {
                    if requested {
                        self.set_status(
                            "Update check failed — could not reach GitHub".to_string(),
                            StatusLevel::Warning,
                        );
                    }
                }
            }
        }
    }
}

/// Run the update download-and-build in a background thread.
///
/// Sends [`UpdateProgress`] messages via the channel to report status.
fn perform_update(
    tx: &mpsc::Sender<UpdateProgress>,
    version: &str,
    assets: &[crate::update_checker::ReleaseAsset],
) {
    let tmpdir = std::env::temp_dir().join(format!("conductor-update-{version}"));
    let _ = std::fs::remove_dir_all(&tmpdir);
    if std::fs::create_dir_all(&tmpdir).is_err() {
        let _ = tx.send(UpdateProgress::Error(
            "Failed to create temp directory".to_string(),
        ));
        return;
    }

    let installed = try_binary_update(tx, version, assets, &tmpdir);
    let _ = std::fs::remove_dir_all(&tmpdir);

    if installed {
        let _ = tx.send(UpdateProgress::Done(format!(
            "v{version} installed successfully! Restarting..."
        )));
    } else {
        // Deliberately no in-app source build: compiling inside the TUI is
        // slow and fragile, and anyone able to build from source can run the
        // command themselves. Point them at the manual path instead.
        let _ = tx.send(UpdateProgress::Error(
            "Could not install the pre-built binary. Update manually with \
             `cargo install --path .` or download a binary from the releases page."
                .to_string(),
        ));
    }
}

/// Attempt to install via pre-built binary. Returns `true` on success.
fn try_binary_update(
    tx: &mpsc::Sender<UpdateProgress>,
    version: &str,
    assets: &[crate::update_checker::ReleaseAsset],
    tmpdir: &std::path::Path,
) -> bool {
    use std::process::Command;

    let asset = match crate::update_checker::find_binary_asset(assets) {
        Some(a) => a,
        None => {
            log::debug!("no matching binary asset for this platform");
            return false;
        }
    };

    let _ = tx.send(UpdateProgress::Status(format!(
        "Downloading pre-built binary v{version}..."
    )));

    let archive = tmpdir.join(&asset.name);
    let mut curl_args = vec![
        "-fL".to_string(),
        "--max-time".to_string(),
        "120".to_string(),
        "-o".to_string(),
        archive.to_string_lossy().to_string(),
    ];

    // Use GITHUB_TOKEN if available. The token is fed to curl via a config
    // read from stdin (`--config -`), never as an argv header: command-line
    // arguments are world-readable (`ps`/`/proc/<pid>/cmdline`), so an argv
    // `-H "Authorization: token …"` would expose the credential to every
    // local process for the duration of the download.
    let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
    if token.is_some() {
        curl_args.push("--config".to_string());
        curl_args.push("-".to_string());
    }

    curl_args.push(asset.download_url.clone());

    let dl = match token {
        Some(token) => Command::new("curl")
            .args(&curl_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(stdin) = child.stdin.take() {
                    use std::io::Write;
                    let mut stdin = stdin;
                    // curl config syntax; a GitHub token never contains `"`.
                    let _ = writeln!(stdin, "header = \"Authorization: token {token}\"");
                }
                child.wait_with_output()
            }),
        None => Command::new("curl")
            .args(&curl_args)
            .stdin(std::process::Stdio::null())
            .output(),
    };
    match dl {
        Err(e) => {
            log::warn!("binary download failed (curl): {e}");
            return false;
        }
        Ok(out) if !out.status.success() => {
            log::warn!("binary download failed (HTTP error)");
            return false;
        }
        _ => {}
    }

    // Extract.
    let _ = tx.send(UpdateProgress::Status("Extracting binary...".to_string()));
    let extract = Command::new("tar")
        .arg("xzf")
        .arg(&archive)
        .arg("-C")
        .arg(tmpdir)
        .output();
    match extract {
        Err(e) => {
            log::warn!("binary extraction failed: {e}");
            return false;
        }
        Ok(out) if !out.status.success() => {
            log::warn!("binary extraction failed (tar error)");
            return false;
        }
        _ => {}
    }

    // The tar.gz contains the `conductor` binary at the top level.
    let new_binary = tmpdir.join("conductor");
    if !new_binary.exists() {
        log::warn!("conductor binary not found in archive");
        return false;
    }

    // Install over the *currently running* executable, resolved to its real
    // path. Guessing `~/.cargo/bin/conductor` would silently update the wrong
    // file when conductor was launched from elsewhere (Homebrew prefix,
    // /usr/local/bin, a symlink), leaving the user's actual binary untouched.
    let dest = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("could not resolve current executable: {e}");
            return false;
        }
    };
    let Some(dest_dir) = dest.parent().map(|d| d.to_path_buf()) else {
        log::warn!("executable has no parent directory");
        return false;
    };

    // Stage the new binary in the *same directory* as `dest` so the final swap
    // can be an atomic rename(2). A cross-filesystem rename fails with EXDEV
    // and would silently degrade to a copy, which is exactly the bug we avoid.
    let staged = dest_dir.join(format!(".conductor-update-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    let _ = tx.send(UpdateProgress::Status("Installing binary...".to_string()));
    if let Err(e) = std::fs::copy(&new_binary, &staged) {
        log::warn!("failed to stage binary: {e}");
        return false;
    }

    // Executable permission on the staged file (set before the swap).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }

    // Strip the macOS quarantine xattr so Gatekeeper won't block it. (The code
    // signature itself is embedded in the Mach-O, not an xattr; this only
    // clears `com.apple.quarantine`.)
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr").args(["-cr"]).arg(&staged).output();
    }

    // Verify the staged binary actually launches before swapping it in — this
    // catches corrupt/truncated downloads. (It does NOT exercise the
    // in-place-overwrite SIGKILL; that class is prevented structurally by the
    // atomic rename below, since `staged` is a brand-new inode.)
    if !verify_runnable(&staged) {
        log::warn!("staged binary failed to launch; aborting install");
        let _ = std::fs::remove_file(&staged);
        return false;
    }

    // Back up the current binary, then atomically swap in the new one.
    // `rename(2)` rebinds the path to a fresh inode, so the still-running
    // process keeps executing from the old (now-unlinked) inode and the next
    // `exec` sees a clean, validly-signed file. Overwriting `dest` in place
    // (the previous `fs::copy`) corrupted the running binary's code-signing
    // state on macOS arm64 and got it SIGKILLed on every subsequent launch.
    let backup = dest_dir.join(".conductor.bak");
    let _ = std::fs::remove_file(&backup);
    if let Err(e) = std::fs::rename(&dest, &backup) {
        log::warn!("failed to back up current binary: {e}");
        let _ = std::fs::remove_file(&staged);
        return false;
    }
    if let Err(e) = std::fs::rename(&staged, &dest) {
        log::warn!("failed to install new binary: {e}; rolling back");
        let _ = std::fs::rename(&backup, &dest);
        let _ = std::fs::remove_file(&staged);
        return false;
    }
    // Success — the new binary is verified and in place; discard the backup.
    let _ = std::fs::remove_file(&backup);

    true
}

/// Spawn `path --version` and report whether it exits successfully.
///
/// Used as a pre-install smoke test: a freshly downloaded binary that can't
/// even print its version (corrupt download, wrong arch, bad signature) must
/// not replace the working one.
fn verify_runnable(path: &std::path::Path) -> bool {
    use std::process::{Command, Stdio};
    match Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(e) => {
            log::warn!("failed to spawn staged binary for verification: {e}");
            false
        }
    }
}
