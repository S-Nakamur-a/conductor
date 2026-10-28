//! セッションの起動: コマンド行を組み立ててホストに起こさせ、写しを用意する。

use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use conductor_core::cc_hook;
use conductor_core::config::ClaudeAccount;
use portable_pty::CommandBuilder;
use uuid::Uuid;

use super::locale::utf8_locale_overrides;
use super::wire::{SessionInfo, SpawnSpec};
use super::{PtySession, PtyStore, SessionKind};

/// 何を起動するか。必要な材料は種類ごとに違うので、それぞれの枝が持つ。
pub enum Launch<'a> {
    ClaudeCode {
        /// MCP へ渡す DB とフック設定の置き場所を決めるリポジトリの根。
        repo_root: &'a Path,
        /// 与えると `--resume`。無ければ新しい id を `--session-id` で強制する。
        resume_session_id: Option<&'a str>,
        session_name: Option<&'a str>,
        session_hooks: bool,
        chrome: bool,
        /// None は窓の env の CLAUDE_CONFIG_DIR のまま起こす。
        account: Option<&'a ClaudeAccount>,
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
        let (mut cmd, claude_session_id) = build_command(&req.launch, &panel_id);
        if let Launch::ClaudeCode {
            session_hooks: true,
            ..
        } = req.launch
        {
            relay_statusline(&mut cmd);
        }
        let spawned_at = SystemTime::now();
        let info = SessionInfo {
            id: panel_id.clone(),
            label: req.label.to_string(),
            kind,
            worktree: req.worktree.to_string(),
            working_dir: req.working_dir.to_path_buf(),
            claude_session_id: claude_session_id.clone(),
            spawned_at_ms: spawned_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            rows: req.rows,
            cols: req.cols,
        };
        let spec = SpawnSpec {
            info,
            argv: cmd
                .get_argv()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            env: cmd
                .iter_full_env_as_str()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let (io, host) = self.spawn_on_host(spec)?;

        self.sessions.push(PtySession {
            id: panel_id,
            label: req.label.to_string(),
            kind,
            worktree: req.worktree.to_string(),
            working_dir: req.working_dir.to_path_buf(),
            claude_session_id,
            spawned_at,
            host,
            io,
        });
        Ok(self.sessions.len() - 1)
    }
}

/// 窓の環境から継いだ中継の印は消す。残ると cc-statusline がユーザの statusLine を走らせない。
pub(super) fn relay_statusline(cmd: &mut CommandBuilder) {
    use cc_hook::statusline;
    cmd.env_remove(statusline::RELAYING_ENV);
    cmd.env(statusline::DIR_ENV, statusline::dir());
}

/// 既定の ~/.claude を綴りで渡すと、Claude Code はログインしていない扱いにする (実測) ので外す。
pub(super) fn use_account(cmd: &mut CommandBuilder, account: Option<&ClaudeAccount>) {
    use cc_hook::statusline::CONFIG_DIR_ENV;
    match account.map(|account| account.config_dir.as_deref()) {
        Some(Some(dir)) => cmd.env(CONFIG_DIR_ENV, dir),
        Some(None) => cmd.env_remove(CONFIG_DIR_ENV),
        None => {}
    }
}

/// コマンド行と、Claude なら書き込み先になる session id。
pub(super) fn build_command(
    launch: &Launch<'_>,
    panel_id: &str,
) -> (CommandBuilder, Option<String>) {
    match launch {
        Launch::ClaudeCode {
            repo_root,
            resume_session_id,
            session_name,
            session_hooks,
            chrome,
            account,
        } => {
            let mut cmd = CommandBuilder::new("claude");
            if *chrome {
                cmd.arg("--chrome");
            }
            use_account(&mut cmd, *account);
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
            // 書けば先客のパネルが次に読む claude-hooks.json をこちらの exe に差し替えてしまう。
            //
            // PANEL_ID は仕掛けない場合も渡す。プラグインのフックはこれの有無だけで自分が
            // 黙るかを決めるので、無いと .conductor/cc-notify.path から先客のソケットを
            // 自力で見つけて active/waiting を送り、先客のストリップが嘘をつく。
            //
            // 宛先は逆に、先に消してから仕掛けた枝だけが入れ直す。2 つ目のウィンドウを
            // 先客のパネルの中から起動すると環境変数でここまで継がれてくるので、渡さない
            // だけでは子に見えたままになる。
            cmd.env(cc_hook::PANEL_ID_ENV, panel_id);
            cmd.env_remove(cc_hook::NOTIFY_SOCK_ENV);
            match session_hooks.then(|| cc_hook::install_settings(repo_root)) {
                Some(Ok(path)) => {
                    cmd.arg("--settings");
                    cmd.arg(&path);
                    cmd.env(cc_hook::NOTIFY_SOCK_ENV, cc_hook::socket_path(repo_root));
                }
                // 書けなくてもパネルは動かす。ローテーション追跡はログからの推測に落ちる。
                Some(Err(e)) => log::warn!("could not install the Claude session hook: {e}"),
                None => {}
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
