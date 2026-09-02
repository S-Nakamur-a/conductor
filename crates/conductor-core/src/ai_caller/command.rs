//! ユーザーが設定した外部コマンドを AI として呼ぶ。プロトコルはモジュールの
//! ドキュメントを参照。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::AiCaller;

const PROMPT_PLACEHOLDER: &str = "{prompt}";
const WORKDIR_PLACEHOLDER: &str = "{workdir}";

/// 子プロセスの終了・キャンセル・タイムアウトを確認しに起きる間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct CommandCaller {
    /// argv。cmd[0] が実行ファイル。いずれの要素にもプレースホルダを書ける。
    pub cmd: Vec<String>,
    /// 実時間のタイムアウト (秒)。0 で無効。
    pub timeout_secs: u64,
    /// 実行するディレクトリ。{workdir} の展開先でもある。None なら Conductor の cwd。
    pub working_dir: Option<PathBuf>,
}

/// 展開後の argv と、{prompt} が見つかったか。「どこにも無い」はプロンプトを
/// 受け取らないコマンドではなく stdin を意味する。
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

        // よく喋るコマンドがパイプのバッファを埋めて終了前にデッドロックしないよう、
        // stdout と stderr は専用スレッドで吸い出す。
        let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
        let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

        // argv で渡した場合も stdin は必ず閉じる。EOF を見せないと stdin を読む
        // ツールが永久にブロックするし、プロンプトを 2 回送るのは送らないより悪い。
        //
        // 書き込みを別スレッドに出すのは、レビューのプロンプトがパイプのバッファ
        // (64KB 程度) を超えるため。ここで待つと stdin を読まないコマンドで
        // タイムアウトもキャンセルも効かなくなる。EPIPE は正常にも起きるので、
        // 書けなかったこと自体は失敗にしない。
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

fn spawn_pipe_reader<R: Read + Send + 'static>(mut pipe: R) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn join_pipe_reader(handle: Option<JoinHandle<String>>) -> String {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// s の末尾 n 文字 (文字境界を壊さず、中間のアロケーションも無い)。
pub(super) fn tail_chars(s: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match s.char_indices().nth_back(n - 1) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}
