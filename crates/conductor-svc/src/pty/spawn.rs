//! セッションの起動: コマンド行を組み立て、reader スレッド・vt100 パーサ・
//! 共有バッファを配線する。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use conductor_core::cc_hook;
use portable_pty::{CommandBuilder, PtySize, PtySystem};
use uuid::Uuid;

use super::locale::utf8_locale_overrides;
use super::{PtySession, PtyStore, SessionKind, SharedIo};

/// 何を起動するか。必要な材料は種類ごとに違うので、それぞれの枝が持つ。
pub enum Launch<'a> {
    ClaudeCode {
        /// MCP へ渡す DB とフック設定の置き場所を決めるリポジトリの根。
        repo_root: &'a Path,
        /// 与えると `--resume`。無ければ新しい id を `--session-id` で強制する。
        resume_session_id: Option<&'a str>,
        session_name: Option<&'a str>,
    },
    Shell {
        program: &'a str,
    },
    /// $VISUAL / $EDITOR を 1 ファイルに対して起動する。program と args は解決済み。
    Editor {
        program: &'a str,
        args: &'a [String],
        file: &'a Path,
    },
}

impl Launch<'_> {
    fn kind(&self) -> SessionKind {
        match self {
            Self::ClaudeCode { .. } => SessionKind::ClaudeCode,
            Self::Shell { .. } => SessionKind::Shell,
            Self::Editor { .. } => SessionKind::Editor,
        }
    }
}

/// 1 セッションの起動要求。
pub struct Spawn<'a> {
    pub launch: Launch<'a>,
    pub worktree: &'a str,
    pub label: &'a str,
    pub working_dir: &'a Path,
    pub rows: u16,
    pub cols: u16,
}

impl PtyStore {
    /// セッションを起動し、一覧内でのインデックスを返す。
    pub fn spawn(&mut self, req: Spawn<'_>) -> Result<usize> {
        // パネル id は SessionStart フックへ環境変数で渡すので、コマンドを組み立てる
        // 前に決める。フックはこの id を名乗って自分の session id を報告してくる。
        let panel_id = Uuid::new_v4().to_string();
        let kind = req.launch.kind();
        let (cmd, claude_session_id) = build_command(&req.launch, &panel_id);

        let pair = self
            .pty_system
            .openpty(PtySize {
                rows: req.rows,
                cols: req.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY pair")?;

        let mut cmd = cmd;
        cmd.cwd(req.working_dir);
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn command in PTY")?;

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("failed to take PTY writer")?,
        ));

        let io = SharedIo {
            lines: Arc::new(Mutex::new(Vec::new())),
            line_limit: Arc::new(Mutex::new(self.inactive_scrollback)),
            screen: Arc::new(Mutex::new(vt100::Parser::new(
                req.rows,
                req.cols,
                self.inactive_scrollback,
            ))),
            raw_history: matches!(kind, SessionKind::Shell)
                .then(|| Arc::new(Mutex::new(VecDeque::new()))),
            last_output: Arc::new(Mutex::new(Instant::now())),
            alt_screen_entered: Arc::new(AtomicBool::new(false)),
            output_notify: Arc::clone(&self.output_notify),
        };

        let reader_io = io.clone();
        let reader_writer = Arc::clone(&writer);
        thread::Builder::new()
            .name(format!("pty-reader-{}", req.label))
            .spawn(move || super::reader::run(reader, reader_io, reader_writer))
            .context("failed to spawn PTY reader thread")?;

        self.sessions.push(PtySession {
            id: panel_id,
            label: req.label.to_string(),
            kind,
            worktree: req.worktree.to_string(),
            working_dir: req.working_dir.to_path_buf(),
            claude_session_id,
            spawned_at: std::time::SystemTime::now(),
            master: pair.master,
            writer,
            child,
            io,
            nudge_until: None,
            last_nudge: None,
        });
        Ok(self.sessions.len() - 1)
    }
}

/// コマンド行と、Claude なら書き込み先になる session id。
fn build_command(launch: &Launch<'_>, panel_id: &str) -> (CommandBuilder, Option<String>) {
    match launch {
        Launch::ClaudeCode {
            repo_root,
            resume_session_id,
            session_name,
        } => {
            let mut cmd = CommandBuilder::new("claude");
            // resume は既存の id を保つ。新規は id を先に決めて渡すので、「worktree の
            // 最新セッション」を推測せずにログのファイル名が事前にわかる。
            let session_id = match resume_session_id {
                Some(id) => {
                    cmd.arg("--resume");
                    cmd.arg(id);
                    (*id).to_string()
                }
                None => {
                    let id = Uuid::new_v4().to_string();
                    cmd.arg("--session-id");
                    cmd.arg(&id);
                    id
                }
            };
            if let Some(name) = session_name {
                cmd.arg("--name");
                cmd.arg(name);
            }
            // MCP サーバがレビュー DB を見つけるため。
            cmd.env(
                "CONDUCTOR_DB_PATH",
                conductor_core::git_engine::conductor_dir(repo_root).join("conductor.db"),
            );

            // --settings はレイヤーを足す形なので、ユーザ自身の settings は生き残る (実測)。
            match cc_hook::install_settings(repo_root) {
                Ok(path) => {
                    cmd.arg("--settings");
                    cmd.arg(&path);
                    cmd.env(cc_hook::PANEL_ID_ENV, panel_id);
                    cmd.env(cc_hook::NOTIFY_SOCK_ENV, cc_hook::socket_path(repo_root));
                }
                // 書けなくてもパネルは動かす。ローテーション追跡はログからの推測に落ちる。
                Err(e) => log::warn!("could not install the Claude session hook: {e}"),
            }
            (cmd, Some(session_id))
        }
        Launch::Shell { program } => (CommandBuilder::new(*program), None),
        Launch::Editor {
            program,
            args,
            file,
        } => {
            let mut cmd = CommandBuilder::new(*program);
            for a in *args {
                cmd.arg(a);
            }
            cmd.arg(file);

            // CommandBuilder は親の環境を継承するので、conductor 自身が UTF-8 ロケール
            // 無しで起動されていると vim が encoding=latin1 に落ち、全角入力が壊れる。
            let (sets, removes) = utf8_locale_overrides(
                std::env::var("LC_ALL").ok().as_deref(),
                std::env::var("LC_CTYPE").ok().as_deref(),
                std::env::var("LANG").ok().as_deref(),
            );
            for (key, value) in sets {
                cmd.env(key, value);
            }
            for key in removes {
                cmd.env_remove(key);
            }
            (cmd, None)
        }
    }
}
