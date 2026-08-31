//! 差し替え可能な AI 呼び出しの抽象。
//!
//! どの LLM プロバイダも小さな 1 つの継ぎ目を満たす:
//! (system_prompt, user_message) -> String。プロンプトの組み立ても応答のパースも
//! Conductor が持ち、プロバイダは生のテキストを返すだけ。だからユーザー向けの
//! 拡張点が極めて単純に保たれる。出力形式がプロバイダの境界を越えることは無い。
//!
//! 組み込みのプロバイダは 1 つ ([GeminiCaller]) で、バイナリに同梱されている。
//! ユーザーが拡張する経路は [CommandCaller] で、設定でコマンドを指定すると
//! Conductor が最小限で安定したプロトコルでそれと話す。Conductor が自前で CLI を
//! ハードコードすることは決してない。他のプロバイダはすべて設定が指すものになる。
//!
//! 外部 LLM コマンドのプロトコル (v2)
//!
//! [api] command が指すのは AI ツールそのもの。プロンプトを受け取って補完を
//! 出力する任意の CLI であればよい。ここはタスクごとの振る舞いを書く場所ではない。
//! どの出力形式が要るか、モデルがツールを使ってよいか、どのディレクトリを見るべきかは、
//! すべて補完を求めている機能の側が決める。そのため両者をつなぐラッパースクリプトは要らない。
//!
//! - 起動: Conductor は設定された argv を直接実行する (シェルを介さない)。
//! - プレースホルダ: どの引数にも {prompt} (組み立て済みのプロンプト) や
//!   {workdir} (タスクのディレクトリ) を書ける。プロンプトを位置引数で受け取る
//!   ツールはその引数の位置に {prompt} を置く。stdin から読むツールは
//!   プレースホルダを一切使わなくてよい。
//! - 入力: {prompt} があればプロンプトはその引数に入り、stdin は即座に閉じられる。
//!   無ければプロンプト (システムプロンプト + 改行 2 つ + ユーザーメッセージ) を
//!   UTF-8 で stdin へ書いてから閉じる。
//! - 作業ディレクトリ: 子プロセスはタスクのディレクトリで動くので、パスを相対で
//!   解決するツール (-w .) は {workdir} が無くても正しい場所に着地する。
//! - 出力: コマンドはモデルの補完を stdout へ書き、Conductor が自分の側でパースする。
//!   コマンドは整形も JSON の抽出も行わない。機能ごとのパーサがコードフェンスや
//!   地の文を許容する。
//! - 終了コード: 0 が成功。非ゼロは失敗で、stderr がエラーに載る。
//! - stderr: 診断のみ。パースはしない。
//! - タイムアウトとキャンセル: Conductor がタスクごとの実時間タイムアウトを課し
//!   ([TaskEnv] を参照)、ユーザーがキャンセルしたら子プロセスを kill する。
//!   コマンドは単に kill されるだけで、通知は受けない。
//!
//! コマンドではなく機能の側に属するもの
//!
//! 各タスクは自分のシステムプロンプトを書き、制約はそこに置く。スマート worktree の
//! 命名はモデルに「ツールを使わず JSON オブジェクト 1 つで答えよ」と伝える。
//! 形式から外れた応答を再試行するかどうかも同様に機能側の判断。再試行のコストを
//! 知っているのはその機能だけだから。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::ApiConfig;

/// (唯一の) スマート worktree 生成タスク向けの、ハードコードしたトークン上限。
/// これは Gemini へのリクエストのつまみであって継ぎ目の一部ではないので、ここに置く。
const GEMINI_MAX_TOKENS: u32 = 1024;

/// [CommandCaller] が子プロセスの終了・キャンセル・タイムアウトを確認するために
/// 起きる間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// プロンプトを生の補完テキストに変えるプロバイダ。
///
/// システムプロンプトを持つのも結果をパースするのも呼び出し側で、実装は
/// モデルのテキスト (かエラー) を返すだけでよい。下層の呼び出しを中断できる場合
/// (サブプロセスなど) は cancel を尊重すること。
pub trait AiCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String>;
}

/// 組み込み: Google Gemini の HTTP API。
pub struct GeminiCaller {
    pub model: String,
    pub max_tokens: u32,
}

impl AiCaller for GeminiCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        crate::gemini_api::call_messages_api(
            system_prompt,
            user_message,
            Some(&self.model),
            self.max_tokens,
        )
        .map_err(|e| format!("{e}"))
    }
}

// 組み込みの Claude プロバイダをあえて置いていない。Conductor は補完のために
// claude プロセスを起動しない。使いたいなら構わないが、それは設定の話であって
// ラッパースクリプトも要らない:
//
//     [api]
//     provider = "command"
//     command = ["claude", "-p", "{prompt}"]
//
// プロンプトを入れて補完を出す CLI なら、他のものも同じやり方で設定できる。

/// 組み立て済みのプロンプトに置き換えられるプレースホルダ。これがあると、
/// プロンプトの受け渡しが stdin から argv へ切り替わる。
const PROMPT_PLACEHOLDER: &str = "{prompt}";

/// タスクの作業ディレクトリに置き換えられるプレースホルダ。
const WORKDIR_PLACEHOLDER: &str = "{workdir}";

/// ユーザーが拡張する経路。モジュールのドキュメントにあるプロトコルを話す外部コマンド。
pub struct CommandCaller {
    /// argv。cmd[0] が実行ファイルで、残りは固定の引数。いずれにも
    /// {prompt} や {workdir} を含められる。
    pub cmd: Vec<String>,
    /// 実時間のタイムアウト (秒)。0 でタイムアウト無効。
    pub timeout_secs: u64,
    /// コマンドを実行するディレクトリ。タスクが対象とするリポジトリまたは worktree で、
    /// {workdir} が展開される先でもある。None なら Conductor の cwd を引き継ぐ。
    pub working_dir: Option<PathBuf>,
}

/// 展開後の argv と、{prompt} が見つかったかを返す。「どこにも無い」はプロンプトを
/// 受け取らないコマンドではなく stdin を意味しなければならない。
fn expand_argv(cmd: &[String], prompt: &str, workdir: Option<&Path>) -> (Vec<String>, bool) {
    let workdir = workdir.map(|d| d.to_string_lossy().into_owned());
    let mut saw_prompt = false;
    let expanded = cmd
        .iter()
        .map(|arg| {
            let mut out = arg.clone();
            if out.contains(PROMPT_PLACEHOLDER) {
                saw_prompt = true;
                out = out.replace(PROMPT_PLACEHOLDER, prompt);
            }
            if let Some(dir) = &workdir {
                out = out.replace(WORKDIR_PLACEHOLDER, dir);
            }
            out
        })
        .collect();
    (expanded, saw_prompt)
}

impl AiCaller for CommandCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        let payload = format!("{system_prompt}\n\n{user_message}");
        let (argv, prompt_in_argv) = expand_argv(&self.cmd, &payload, self.working_dir.as_deref());
        let program = argv
            .first()
            .ok_or_else(|| "AI command is empty".to_string())?
            .clone();
        log::info!(
            "AI caller: using external command '{program}' (prompt via {})",
            if prompt_in_argv { "argv" } else { "stdin" }
        );

        let mut command = Command::new(&program);
        command.args(&argv[1..]);
        if let Some(dir) = &self.working_dir {
            command.current_dir(dir);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn AI command '{program}': {e}"))?;

        // stdout と stderr はそれぞれ専用スレッドで吸い出す。よく喋るコマンド
        // (進捗を stderr に出すツールなど) がパイプのバッファを埋めて、終了する前に
        // デッドロックすることがないようにするため。
        let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
        let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

        // プロンプトを渡す。既に引数として渡してある場合でも stdin は即座に閉じる
        // (ハンドルを drop する)。それでも stdin を読むツールには、永久にブロックする
        // のではなく EOF を見せなければならないし、プロンプトを 2 回送るのは
        // まったく送らないより悪いから。
        //
        // 書き込みは別スレッドに出す。レビューのプロンプトはパイプのバッファ
        // (64KB 程度) を平気で超えるので、ここで待つと stdin を読まないコマンドを
        // 指したときに下の loop へ辿り着けず、タイムアウトもキャンセルも効かなくなる。
        // 書けなかったこと自体は失敗にしない (相手が途中で読むのをやめた EPIPE は
        // 正常にもなり得る)。成否は終了コードと stdout で見る。
        let stdin_writer = child.stdin.take().map(|mut stdin| {
            let payload = if prompt_in_argv {
                Vec::new()
            } else {
                payload.into_bytes()
            };
            std::thread::spawn(move || {
                let _ = stdin.write_all(&payload);
            })
        });

        // 終了をポーリングする。キャンセルと実時間タイムアウトを尊重する。
        let start = Instant::now();
        let exit_status = loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Cancelled".to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if self.timeout_secs > 0 && start.elapsed().as_secs() >= self.timeout_secs {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "AI command '{program}' timed out after {}s",
                            self.timeout_secs
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(format!("Failed to wait on AI command: {e}")),
            }
        };

        // 子が終わっている以上、書き手も EOF か EPIPE で必ず解ける。
        if let Some(h) = stdin_writer {
            let _ = h.join();
        }
        let stdout = join_pipe_reader(stdout_reader);
        let stderr = join_pipe_reader(stderr_reader);

        if !exit_status.success() {
            return Err(format!(
                "AI command '{program}' failed ({exit_status}): {}",
                tail_chars(stderr.trim(), 500)
            ));
        }
        if stdout.trim().is_empty() {
            return Err(format!("AI command '{program}' returned empty output"));
        }
        Ok(stdout)
    }
}

/// 子プロセスのパイプをワーカースレッドで最後まで読み、UTF-8 として寛容にデコードする。
fn spawn_pipe_reader<R: Read + Send + 'static>(mut pipe: R) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// パイプ読み取りスレッドを join し、取得したテキストを返す (join に失敗したら空)。
fn join_pipe_reader(handle: Option<JoinHandle<String>>) -> String {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// s の末尾 n 文字 (文字境界を壊さず、中間のアロケーションも無い)。
fn tail_chars(s: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match s.char_indices().nth_back(n - 1) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// タスクがプロンプト以外に AI へ渡す必要があるもの。どれだけ時間をかけてよいかと、
/// どのディレクトリについての話か。
///
/// どちらもプロバイダ単位ではなくタスク単位。スマート worktree の命名は数秒の
/// 純粋なテキスト生成なので [api] command_timeout_secs をそのまま使うが、
/// 数分かかるタスクが同じ設定値に頭打ちにされては困る。予算を知っているのは
/// タスクの側なので、上書きの口をここに開けてある。
#[derive(Debug, Clone, Default)]
pub struct TaskEnv {
    /// 設定されていれば [api] command_timeout_secs を上書きする。0 で無効。
    pub timeout_secs: Option<u64>,
    /// 外部コマンドを実行するディレクトリ。タスクが対象とする worktree。
    pub working_dir: Option<PathBuf>,
}

/// タスク向けに、設定された AI 呼び出しを組み立てる。
///
/// プロバイダ ([api] provider)。各プロバイダは独立していて、失敗しても黙って
/// 別のプロバイダへフォールバックせず、ユーザーに提示される。
/// - "gemini" (既定): Gemini の HTTP API。
/// - "command": ユーザーの外部コマンド ([api] command)。
///
/// 組み込みの claude プロバイダはあえて存在しない。Conductor の中から claude
/// CLI を起動することは、このコードベースのどこでも許していない。Claude を
/// 動かすためにあるのがまさに provider = "command" で、ユーザーが直接 CLI を
/// 指定すれば、Conductor はその背後のモデルが何かを知る必要が無い。
///
/// なお "gemini" は素の HTTP 補完なのでリポジトリを読めない。リポジトリを
/// 読ませる必要のあるタスクは、エージェント型の CLI を指した "command" の
/// 下でのみ動く。
pub fn build_caller(api: &ApiConfig, env: &TaskEnv) -> Result<Box<dyn AiCaller>, String> {
    match api.provider.trim().to_lowercase().as_str() {
        "gemini" => Ok(Box::new(GeminiCaller {
            model: api.model.clone(),
            max_tokens: GEMINI_MAX_TOKENS,
        })),
        "command" => {
            if api.command.is_empty() {
                return Err(
                    "provider = \"command\" but [api] command is empty; set command = [\"...\"]"
                        .to_string(),
                );
            }
            Ok(Box::new(CommandCaller {
                cmd: api.command.clone(),
                timeout_secs: env.timeout_secs.unwrap_or(api.command_timeout_secs),
                working_dir: env.working_dir.clone(),
            }))
        }
        other => Err(format!(
            "unknown AI provider '{other}' (expected: gemini, command)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(provider: &str) -> ApiConfig {
        ApiConfig {
            provider: provider.to_string(),
            ..Default::default()
        }
    }

    // build_caller のプロバイダ選択と検証

    #[test]
    fn build_caller_accepts_known_providers() {
        assert!(build_caller(&api("gemini"), &TaskEnv::default()).is_ok());
        assert!(build_caller(&ApiConfig::default(), &TaskEnv::default()).is_ok());
    }

    #[test]
    fn build_caller_is_case_and_whitespace_insensitive() {
        assert!(build_caller(&api("GEMINI"), &TaskEnv::default()).is_ok());
        assert!(build_caller(&api("  gemini  "), &TaskEnv::default()).is_ok());
    }

    /// Conductor が claude CLI を自分で起動することは決してあってはならないので、
    /// かつてまさにそれをしていたプロバイダ名は、いまはただの未知の値になっている。
    /// そしてエラーは command を指し示さねばならない。Claude を使う構成はそちらで
    /// 組むから。
    #[test]
    fn build_caller_rejects_the_removed_claude_provider() {
        let err = build_caller(&api("claude"), &TaskEnv::default())
            .err()
            .unwrap();
        assert!(err.contains("claude"), "should echo the bad value: {err}");
        assert!(err.contains("command"), "should point at the way in: {err}");
    }

    #[test]
    fn build_caller_rejects_unknown_provider() {
        let err = build_caller(&api("ollama"), &TaskEnv::default())
            .err()
            .unwrap();
        assert!(err.contains("ollama"), "should echo the bad value: {err}");
        assert!(err.contains("gemini"), "should list valid values: {err}");
    }

    #[test]
    fn build_caller_rejects_empty_command() {
        let cfg = ApiConfig {
            provider: "command".to_string(),
            command: Vec::new(),
            ..Default::default()
        };
        let err = build_caller(&cfg, &TaskEnv::default()).err().unwrap();
        assert!(err.contains("command"), "actionable message: {err}");
    }

    #[test]
    fn build_caller_accepts_nonempty_command() {
        let cfg = ApiConfig {
            provider: "command".to_string(),
            command: vec!["cat".to_string()],
            ..Default::default()
        };
        assert!(build_caller(&cfg, &TaskEnv::default()).is_ok());
    }

    // tail_chars

    #[test]
    fn tail_chars_takes_last_n() {
        assert_eq!(tail_chars("hello", 3), "llo");
        assert_eq!(tail_chars("hi", 5), "hi");
        assert_eq!(tail_chars("hi", 0), "");
        // マルチバイト入力でも文字境界を壊さない
        assert_eq!(tail_chars("あいうえお", 2), "えお");
    }

    // CommandCaller (実際のサブプロセス。Unix のみ)

    #[cfg(unix)]
    mod command {
        use super::*;

        fn sh(script: &str, timeout_secs: u64) -> CommandCaller {
            CommandCaller {
                cmd: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
                timeout_secs,
                working_dir: None,
            }
        }

        #[test]
        fn echoes_prompt_via_stdin() {
            let caller = sh("cat", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let out = caller.complete("SYS", "USER", &cancel).unwrap();
            assert!(out.contains("SYS") && out.contains("USER"), "got: {out}");
        }

        /// コマンドはタスクが対象とするディレクトリで動く。エージェント型の
        /// コマンドが、問われている当のコードへ辿り着く唯一の手段がこれなので、
        /// これは実装の細部ではなくプロトコルの一部。
        #[test]
        fn runs_in_the_task_working_directory() {
            let dir = tempfile::tempdir().unwrap();
            let caller = CommandCaller {
                cmd: vec!["sh".to_string(), "-c".to_string(), "pwd".to_string()],
                timeout_secs: 5,
                working_dir: Some(dir.path().to_path_buf()),
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let out = caller.complete("s", "u", &cancel).unwrap();
            // macOS は /var… の一時ディレクトリを /private/var… と報告するので、
            // 元の文字列ではなく解決後の形で比較する。
            let reported = std::fs::canonicalize(out.trim()).unwrap();
            assert_eq!(reported, std::fs::canonicalize(dir.path()).unwrap());
        }

        /// タイムアウトを指定したタスクはその値を使い、設定の値は指定が無いときだけ
        /// 埋める。数秒の命名を想定した command_timeout_secs の下で、それより
        /// 長く走るタスクが頭打ちにされないための口。
        ///
        /// 組み立てた caller を覗くのではなく振る舞いで検証する。設定側は
        /// タイムアウトを完全に無効にしてあるので、コマンドが kill されるのは
        /// タスク自身の値が届いたときだけ。
        #[test]
        fn task_timeout_overrides_the_configured_one() {
            let cfg = ApiConfig {
                provider: "command".to_string(),
                command: vec!["sh".to_string(), "-c".to_string(), "sleep 30".to_string()],
                command_timeout_secs: 0,
                ..Default::default()
            };
            let caller = build_caller(
                &cfg,
                &TaskEnv {
                    timeout_secs: Some(1),
                    working_dir: None,
                },
            )
            .unwrap();

            let start = Instant::now();
            let err = caller
                .complete("s", "u", &Arc::new(AtomicBool::new(false)))
                .unwrap_err();
            assert!(err.contains("timed out after 1s"), "got: {err}");
            assert!(start.elapsed() < Duration::from_secs(10));
        }

        /// プロンプトを位置引数で受け取るツール (claude -p と、たいていの
        /// エージェント型 CLI) は設定で直接指定する。ラッパースクリプト無しで
        /// それを可能にしているのが {prompt}。
        #[test]
        fn prompt_placeholder_delivers_via_argv() {
            let caller = CommandCaller {
                // printf %s は引数をそのまま出すので、stdout は argv に着地した
                // ものそのものになる。
                cmd: vec![
                    "printf".to_string(),
                    "%s".to_string(),
                    "PRE[{prompt}]POST".to_string(),
                ],
                timeout_secs: 5,
                working_dir: None,
            };
            let out = caller
                .complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(out, "PRE[SYS\n\nUSER]POST");
        }

        /// さらに、プロンプトが stdin にも届いてはいけない。届くとモデルが 2 回
        /// 見ることになる。cat なら stdin の内容をそのまま後ろに足してしまう。
        #[test]
        fn prompt_placeholder_leaves_stdin_empty() {
            let caller = CommandCaller {
                cmd: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf 'argv=%s;' \"$1\"; printf 'stdin='; cat".to_string(),
                    "sh".to_string(),
                    "{prompt}".to_string(),
                ],
                timeout_secs: 5,
                working_dir: None,
            };
            let out = caller
                .complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(out, "argv=SYS\n\nUSER;stdin=");
        }

        /// どこにもプレースホルダが無ければ stdin での受け渡しが保たれる。
        /// stdin 型のツール (ollama run …) がこれに依存している。
        #[test]
        fn without_a_placeholder_the_prompt_still_goes_to_stdin() {
            let caller = sh("cat", 5);
            let out = caller
                .complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(out, "SYS\n\nUSER");
        }

        /// {workdir} はタスクのディレクトリに展開される。cwd ではなく明示的な
        /// フラグとして受け取りたいツールでも、ユーザーが設定に特定の worktree を
        /// ハードコードせずに済む。
        #[test]
        fn workdir_placeholder_expands_to_the_task_directory() {
            let dir = tempfile::tempdir().unwrap();
            let caller = CommandCaller {
                cmd: vec![
                    "printf".to_string(),
                    "%s".to_string(),
                    "{workdir}".to_string(),
                ],
                timeout_secs: 5,
                working_dir: Some(dir.path().to_path_buf()),
            };
            let out = caller
                .complete("s", "u", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(
                std::fs::canonicalize(out.trim()).unwrap(),
                std::fs::canonicalize(dir.path()).unwrap()
            );
        }

        #[test]
        fn nonzero_exit_surfaces_stderr() {
            let caller = sh("echo boom >&2; exit 1", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("boom"), "stderr tail: {err}");
            assert!(err.contains("failed"));
        }

        #[test]
        fn empty_success_is_an_error() {
            let caller = sh("exit 0", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("empty"), "got: {err}");
        }

        /// stdin を読まないコマンドに、パイプのバッファを超えるプロンプトを渡しても
        /// 打ち切れる。書き込みを待ってから時間を見る作りだと、ここで止まったまま
        /// タイムアウトもキャンセルも一度も効かない。レビューのプロンプトは
        /// 実際にこの大きさになる。
        #[test]
        fn a_command_that_never_reads_stdin_still_times_out() {
            let caller = sh("sleep 30", 1);
            let cancel = Arc::new(AtomicBool::new(false));
            let big = "x".repeat(1 << 20);
            let start = Instant::now();
            let err = caller.complete(&big, &big, &cancel).unwrap_err();
            assert!(err.contains("timed out"), "got: {err}");
            assert!(start.elapsed() < Duration::from_secs(10));
        }

        #[test]
        fn times_out_without_waiting_for_the_command() {
            let caller = sh("sleep 5", 1);
            let cancel = Arc::new(AtomicBool::new(false));
            let start = Instant::now();
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("timed out"), "got: {err}");
            assert!(
                start.elapsed() < Duration::from_secs(4),
                "should not wait 5s"
            );
        }

        #[test]
        fn preset_cancel_returns_immediately() {
            let caller = sh("sleep 5", 0);
            let cancel = Arc::new(AtomicBool::new(true));
            let start = Instant::now();
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert_eq!(err, "Cancelled");
            assert!(start.elapsed() < Duration::from_secs(4));
        }

        #[test]
        fn missing_program_is_a_spawn_error() {
            let caller = CommandCaller {
                cmd: vec!["definitely_not_a_real_binary_xyzzy".to_string()],
                timeout_secs: 5,
                working_dir: None,
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(
                err.contains("definitely_not_a_real_binary_xyzzy"),
                "got: {err}"
            );
        }
    }
}
