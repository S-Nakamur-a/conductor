//! PTY セッションの起動: Claude Code / Shell / Editor のコマンド行を組み立て、
//! 新しい [PtySession] を支える reader スレッド・vt100 パーサ・共有バッファを配線する。

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
    /// 新しい PTY セッションを起動し、セッションリスト内でのインデックスを返す。
    ///
    /// `resume_session_id` が `Some` なら Claude CLI に `--resume <id>` を、`session_name` が
    /// `Some` なら `--name <name>` を渡す。`repo_root` は CONDUCTOR_DB_PATH の設定に使い、
    /// MCP サーバがレビューデータベースを見つけられるようにする。
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
        // パネルの識別子は SessionStart フックへ環境変数で渡し、フックが「どのパネルの通知か」を
        // 名乗れるようにする (cc_hook)。セッションを作る前に決めるのは、コマンドを組み立てる
        // 時点で環境変数に入れる必要があるため。
        let panel_id = Uuid::new_v4().to_string();

        let (cmd, claude_session_id) = match kind {
            SessionKind::ClaudeCode => {
                let mut c = CommandBuilder::new("claude");
                // resume は既存の session id を保つ。新規起動では --session-id で生成した id を強制
                // するので、「worktree の最新セッション」を推測せずに事前にファイル名がわかる。
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
                // conductor の MCP サーバがレビューデータベースを見つけられるようにする。
                let db_path = repo_root.join(".conductor").join("conductor.db");
                c.env("CONDUCTOR_DB_PATH", db_path);

                // /clear は書き込み先を新しい session id の .jsonl に移すが、相互参照が残らない。
                // SessionStart フックはパネル自身の Claude プロセスの中で走って新しい id を持って来るので、
                // ローテーションを推測ではなく事実として受け取れる。--settings は追加レイヤーとして重なる
                // (実測: ユーザ全体もプロジェクトの settings もそのまま生き残る)。
                match Self::write_hook_settings(repo_root) {
                    Ok(path) => {
                        c.arg("--settings");
                        c.arg(&path);
                        c.env(crate::cc_hook::PANEL_ID_ENV, &panel_id);
                        c.env(
                            crate::cc_hook::NOTIFY_SOCK_ENV,
                            crate::cc_notify::socket_path(repo_root),
                        );
                    }
                    // 書けなくてもパネルは動かす。ローテーション追跡は
                    // claude_sessions::rotation の推測にフォールバックする。
                    Err(e) => log::warn!("could not install the Claude session hook: {e}"),
                }
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
            // フックが名乗る id と一致させる。
            session.id = panel_id;
        }
        Ok(idx)
    }

    /// SessionStart フックを差し込む settings を .conductor/ に書き、その
    /// パスを返す。
    ///
    /// フックのコマンドは conductor 自身 (conductor cc-hook)。シェルスクリプトや
    /// jq を挟まないのは、signal をバイナリと同じ成果物に載せるため — 別
    /// リリースチャネルに置くとバージョンがずれた組み合わせで黙って効かなくなる。
    /// 実行ファイルは絶対パスで書くので、claude の PATH に conductor が
    /// 無くても動く。
    pub(super) fn write_hook_settings(repo_root: &Path) -> Result<PathBuf> {
        let exe = std::env::current_exe().context("could not locate the conductor executable")?;
        let dir = repo_root.join(".conductor");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        let path = dir.join("claude-hooks.json");

        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": format!("{} cc-hook", Self::shell_quote(&exe.to_string_lossy())),
                    }],
                }],
            },
        });
        // spawn のたびに書き直す。conductor の置き場所が変わっても追随する。
        std::fs::write(&path, serde_json::to_vec_pretty(&settings)?)
            .with_context(|| format!("could not write {}", path.display()))?;
        Ok(path)
    }

    /// フックのコマンド行に埋める実行ファイルパスを POSIX シェル用にクォートする。
    ///
    /// Claude Code はフックの command をシェルに渡すので、空白や引用符を含む
    /// パス (/Users/me/my tools/conductor など) はそのままでは分割される。
    pub(super) fn shell_quote(raw: &str) -> String {
        format!("'{}'", raw.replace('\'', r"'\''"))
    }

    /// 外部エディタ($VISUAL / $EDITOR)を、単一の file に対して一時的な
    /// PTY セッションとして起動する。program + args は解決済みのエディタ
    /// コマンド行(すでにプログラムと引数に分割済み)で、file は最後の引数
    /// として追加される。
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

        // エディタが UTF-8 ロケールを認識するようにする。CommandBuilder は
        // 親の環境を継承するため、Conductor が UTF-8 ロケール無しで起動された
        // 場合(素のログインシェル、cron、LANG=C を転送する SSH セッション
        // など)、エディタもそれを継承してしまう — すると vim のような端末
        // エディタは encoding=latin1 にフォールバックし、全角文字や
        // マルチバイト入力を壊す。
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

        self.finish_spawn(
            SessionKind::Editor,
            worktree,
            label,
            working_dir,
            rows,
            cols,
            cmd,
        )
    }

    /// spawn 経路の共有末尾処理。cmd は組み立て済みで、作業ディレクトリはここで設定する。
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
        // 1. 指定サイズで新しい PTY ペアを開く。
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

        // 3. スレーブ側で子プロセスを起動する。
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn command in PTY")?;

        // 4. マスター側から reader/writer ハンドルを取得する。
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

        // 5. 共有出力バッファをセットアップする。
        let output_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let max_buffer_lines = self.inactive_scrollback;

        // 5b. PTY と同じサイズで vt100 パーサを作る。
        let screen: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(
            rows,
            cols,
            self.inactive_scrollback,
        )));

        // 5c. リフロー用の生バイト履歴。実際に再生でリフローできる出力を持つ
        // セッション(raw_history フィールドのドキュメント参照)、つまり
        // 端末の自動折り返しに頼る通常のシェルに限って記録する。Claude と
        // 一時的なエディタは固定幅でその場描画するため、再生してもリフロー
        // されずメモリを消費するだけなので、記録をスキップする。
        let raw_history: Option<Arc<Mutex<VecDeque<u8>>>> =
            matches!(kind, SessionKind::Shell).then(|| Arc::new(Mutex::new(VecDeque::new())));

        // 6. PTY 出力を継続的に読み取るバックグラウンドスレッドを起動する。
        let buffer_clone = Arc::clone(&output_buffer);
        let screen_clone = Arc::clone(&screen);
        let raw_history_clone = raw_history.clone();
        // max_buffer_lines はセッションにも保持するが、reader スレッドは
        // 自分自身の参照を必要とする。set_active() が動的に上限を調整
        // できるよう、別の Arc<Mutex<usize>> を使う。
        let buffer_limit = Arc::new(Mutex::new(max_buffer_lines));
        let buffer_limit_for_thread = Arc::clone(&buffer_limit);

        // 直近の出力受信時刻を追跡する(入力待ち検出のため)。
        let last_output_time = Arc::new(Mutex::new(Instant::now()));
        let last_output_time_for_thread = Arc::clone(&last_output_time);

        // オルタネート画面への遷移を追跡し、初期 UI をまだ描画していないかも
        // しれないプログラム(fzf など)をメインループがナッジできるようにする。
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

        // 7. セッション構造体を組み立てる。
        let session = PtySession {
            id: Uuid::new_v4().to_string(),
            label: label.to_string(),
            kind,
            worktree: worktree.to_string(),
            working_dir: working_dir.clone(),
            // Claude パネルについてはこの戻り値の後に spawn_session が
            // 埋める。Shell/Editor セッションは None のまま。
            claude_session_id: None,
            spawned_at: std::time::SystemTime::now(),
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

        // reader スレッド用にバッファ上限の Arc を保持し、set_active() が
        // 動的に調整できるようにする。
        self.buffer_limits.push(buffer_limit);

        Ok(idx)
    }
}
