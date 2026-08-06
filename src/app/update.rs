//! アプリ内自己更新フロー: GitHub Releasesの確認、ビルド済みバイナリの
//! ダウンロードとインストール、メインイベントループのためのバックグラウンド
//! 処理のポーリング。

use std::sync::mpsc;

use super::{App, StatusLevel};

/// アプリ内更新フローの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateState {
    /// 通常運用 — 更新は進行していない。
    #[default]
    Idle,
    /// 確認ダイアログを表示中。
    Confirming,
    /// バックグラウンドスレッドでダウンロード＆ビルドを実行中。
    InProgress,
    /// プロセスを再起動しようとしている。
    Restarting,
    /// エラーが発生した — 解除されるまでメッセージを表示する。
    Failed,
}

/// バックグラウンド更新スレッドから送られるメッセージ。
#[derive(Debug, Clone)]
pub enum UpdateProgress {
    /// 途中経過のステータスメッセージ。
    Status(String),
    /// 更新が正常に完了した。
    Done(String),
    /// 更新がエラーメッセージとともに失敗した。
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

    /// GitHub Releasesに新しいバージョンがないか、要求に応じて手動で確認する。
    /// 起動時/一定間隔でのサイレントな確認と違い、これはバックグラウンドの
    /// 結果が [poll_all_background_ops](Self::poll_all_background_ops) に届いた
    /// 時点で、どの結果（更新あり／既に最新／確認失敗）でも明示的なフィード
    /// バックを出す。
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

    /// 更新確認ダイアログを表示する。
    pub fn start_update_confirm(&mut self) {
        self.update.state = UpdateState::Confirming;
    }

    /// バックグラウンド更新スレッドを起動する。
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

    /// バックグラウンド更新スレッドからの進捗メッセージをポーリングする。
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

    /// すべてのバックグラウンド処理をポーリングし、その結果を反映する。
    ///
    /// 以前はmain.rsのrun_loop()に散らばっていたpoll_*()呼び出しを、
    /// ここに集約している。
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
        self.poll_revidere();
        self.poll_publish_review();

        // ccusage
        if let Some(info) = self.bg.ccusage.poll() {
            self.stats.ccusage = Some(info);
        }

        // シンボル索引
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
            // ビルドが古い根を走査している間に根が動くことがあり、かつ
            // start_symbol_index_buildは実行中のビルドの上に2つ目を積む
            // ことを拒む。この組み合わせにより、完了したビルドが自分の
            // 結果を何もキューに積まずに捨ててしまい、次のファイルシステム
            // イベントまで索引が空のままになる。ここで追いつきビルドを
            // 起動するのがその隙間を塞ぐ方法だ: この時点でスロットは
            // 空いており、根が一度も動いていなければ索引はすでに利用可能
            // とマークされているのでこれは何もしない。
            if !self.code_nav.index.is_available() {
                self.start_symbol_index_build();
            }
        }

        // 更新確認。外側のOptionは「結果が届いた」ことを表し、内側は確認
        // そのものの結果（成功ならSome(info)、ネットワーク/パースエラー
        // ならNone）。
        if let Some(result) = self.bg.update_check.poll() {
            // 今回、ユーザーが明示的なフィードバックを要求していたかどうか。
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

/// ダウンロードとビルドの更新処理をバックグラウンドスレッドで実行する。
///
/// ステータス報告のため、チャンネル経由で [UpdateProgress] メッセージを送る。
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
        // あえてアプリ内でのソースビルドはしない: TUI内でのコンパイルは
        // 遅く壊れやすく、ソースからビルドできる人なら自分でコマンドを
        // 実行できる。代わりに手動での手順を案内する。
        let _ = tx.send(UpdateProgress::Error(
            "Could not install the pre-built binary. Update manually with \
             `cargo install --path .` or download a binary from the releases page."
                .to_string(),
        ));
    }
}

/// ビルド済みバイナリ経由でのインストールを試みる。成功時trueを返す。
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

    // GITHUB_TOKENがあれば使う。トークンはargvのヘッダーとしてではなく、
    // 標準入力から読む設定(--config -)経由でcurlに渡す: コマンドライン
    // 引数は誰でも読める(ps//proc/<pid>/cmdline)ので、argvで
    // -H "Authorization: token …" とすると、ダウンロードしている間ずっと
    // ローカルの全プロセスに資格情報がさらされてしまう。
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
                    // curlの設定構文。GitHubトークンに " が含まれることはない。
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

    // 展開する。
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

    // tar.gzの最上位にconductorバイナリが入っている。
    let new_binary = tmpdir.join("conductor");
    if !new_binary.exists() {
        log::warn!("conductor binary not found in archive");
        return false;
    }

    // *現在実行中の*実行ファイルを、その実パスに解決した上で上書きする。
    // ~/.cargo/bin/conductorだと決め打ちすると、conductorが別の場所
    // （Homebrewのprefix、/usr/local/bin、シンボリックリンクなど）から
    // 起動されていた場合に間違ったファイルを黙って更新してしまい、
    // ユーザーの実際のバイナリは手つかずのまま残ってしまう。
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

    // 新しいバイナリをdestと*同じディレクトリ*に置いておくことで、最終的な
    // 入れ替えをアトミックなrename(2)にできる。ファイルシステムをまたぐ
    // renameはEXDEVで失敗し、黙ってcopyに劣化してしまう — それはまさに
    // 避けたいバグそのものだ。
    let staged = dest_dir.join(format!(".conductor-update-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    let _ = tx.send(UpdateProgress::Status("Installing binary...".to_string()));
    if let Err(e) = std::fs::copy(&new_binary, &staged) {
        log::warn!("failed to stage binary: {e}");
        return false;
    }

    // 配置したファイルに実行権限を付与する（入れ替えの前に設定する）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }

    // macOSのquarantine xattrを外し、Gatekeeperに止められないようにする。
    // （コード署名そのものはxattrではなくMach-Oに埋め込まれているので、
    // ここではcom.apple.quarantineだけを消す。）
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr").args(["-cr"]).arg(&staged).output();
    }

    // 入れ替える前に、配置したバイナリが実際に起動できることを確認する
    // — これは壊れた/途中で切れたダウンロードを検出する。（インプレース
    // 上書きによるSIGKILLは検証しない。そのクラスの問題は下のアトミック
    // renameによって構造的に防がれている。stagedは新規のinodeだからだ。）
    if !verify_runnable(&staged) {
        log::warn!("staged binary failed to launch; aborting install");
        let _ = std::fs::remove_file(&staged);
        return false;
    }

    // 現在のバイナリをバックアップしてから、新しいものをアトミックに
    // 入れ替える。rename(2)はパスを新しいinodeに結び直すので、実行中の
    // プロセスは古い（今はunlinkされた）inodeから実行され続け、次の
    // execはクリーンで正しく署名されたファイルを見る。（前身の
    // fs::copyのように）destをインプレース上書きすると、macOS arm64上で
    // 実行中バイナリのコード署名状態が壊れ、以降起動するたびにSIGKILLされた。
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
    // 成功 — 新しいバイナリは検証済みで配置済みなので、バックアップは破棄する。
    let _ = std::fs::remove_file(&backup);

    true
}

/// pathに対して --version を実行し、正常終了したかどうかを報告する。
///
/// インストール前のスモークテストとして使う: バージョンすら出力できない
/// (ダウンロードが壊れている、アーキテクチャ違い、署名不正などの)
/// できたてのバイナリで、動いているものを置き換えてはならない。
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
