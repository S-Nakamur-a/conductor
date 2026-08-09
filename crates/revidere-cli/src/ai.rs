// AI の呼び出し。ここが唯一の継ぎ目で、revidere が持つのは
// 「(システムプロンプト, ユーザーメッセージ) を渡して補完テキストを受け取る」
// までとする。どのモデルをどう動かすかは revidere の関心事ではない。
//
// 組み込みのプロバイダは無い。何を起動するかは設定ファイルの [ai] command が
// 決める（config.rs）。ラッパースクリプトを挟まずに済むよう、外部コマンドに
// 求めるのは次だけにしてある。
//
//   起動        設定された argv をそのまま実行する（シェルは介さない）
//   {prompt}    どの引数にも書ける。組み立て済みのプロンプトに置き換わる
//   {workdir}   同上。レビュー対象のリポジトリのパスに置き換わる
//   入力        {prompt} があればそこへ入り、stdin は即座に閉じる。
//               無ければプロンプトを stdin へ書いて閉じる
//   出力        stdout が補完。整形も JSON の抽出もコマンド側では行わない
//               （コードフェンスや地の文は parse.rs が許容する）
//   終了コード  0 が成功。非ゼロは失敗で、stderr の末尾をエラーに載せる
//
// 出力形式やツールの使用可否はプロンプト側（prompt.rs）の責務で、コマンドの
// 責務ではない。

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 組み立て済みのプロンプトに置き換わるプレースホルダ。これがあると、
/// プロンプトの渡し方が stdin から argv へ切り替わる。
const PROMPT_PLACEHOLDER: &str = "{prompt}";

/// レビュー対象のリポジトリのパスに置き換わるプレースホルダ。
const WORKDIR_PLACEHOLDER: &str = "{workdir}";

/// 子プロセスの終了を確認しに起きる間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub struct AiError(pub String);

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AiError {}

/// argv のプレースホルダを展開する。
///
/// 展開後の argv と、{prompt} が見つかったかどうかを返す。argv と stdin の
/// どちらでプロンプトを渡すかの判断に後者が要る。「どこにも書かれていない」は
/// 「プロンプトを受け取らないコマンド」ではなく stdin を意味しなければならない。
fn expand_argv(argv: &[String], prompt: &str, workdir: &Path) -> (Vec<String>, bool) {
    let dir = workdir.to_string_lossy();
    let mut saw_prompt = false;
    let expanded = argv
        .iter()
        .map(|arg| {
            let mut out = arg.clone();
            if out.contains(PROMPT_PLACEHOLDER) {
                saw_prompt = true;
                out = out.replace(PROMPT_PLACEHOLDER, prompt);
            }
            out.replace(WORKDIR_PLACEHOLDER, &dir)
        })
        .collect();
    (expanded, saw_prompt)
}

/// プロンプトを渡して補完テキストを受け取る。
///
/// 子プロセスの作業ディレクトリはレビュー対象のリポジトリ。モデルが自分で
/// git を叩いてファイルを読むので、ここが合っていないと別のリポジトリを
/// 読んで答えることになる。
pub fn run(
    argv: &[String],
    workdir: &Path,
    system: &str,
    user: &str,
    timeout: Duration,
) -> Result<String, AiError> {
    let payload = format!("{system}\n\n{user}");
    let (argv, prompt_in_argv) = expand_argv(argv, &payload, workdir);
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| AiError("AI コマンドが空".into()))?;

    let mut child = Command::new(program)
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AiError(format!("{program} を起動できない: {e}")))?;

    // stdout/stderr は別スレッドで最後まで吸い出す。進捗を stderr に出すような
    // よく喋るコマンドがパイプのバッファを埋めて、終了する前に止まらないように。
    let stdout = child.stdout.take().map(spawn_pipe_reader);
    let stderr = child.stderr.take().map(spawn_pipe_reader);

    // stdin も別スレッドで書く。プロンプトはパイプのバッファ（64KB 程度）を
    // 平気で超えるので、ここで待つと stdin を読まないコマンドを指したときに
    // 打ち切りの判定へ辿り着けないまま止まる。書き込みが遅れても下の loop が
    // 時間を見ていられるように、待つ場所を 1 か所に寄せる。
    //
    // argv で渡した場合でも閉じるのは、stdin を読むコマンドに EOF を見せる
    // ため。プロンプトを 2 回渡すのは 1 回も渡さないより悪い。
    let stdin = child.stdin.take().map(|mut pipe| {
        let payload = if prompt_in_argv {
            Vec::new()
        } else {
            payload.into_bytes()
        };
        // 書けなかったこと自体は失敗にしない。相手が途中で読むのをやめた
        // （EPIPE）のは正常にもなり得るので、成否は終了コードと stdout で見る。
        std::thread::spawn(move || {
            let _ = pipe.write_all(&payload);
        })
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if !timeout.is_zero() && started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AiError(format!(
                        "{} 秒で応答が無いので打ち切った",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(AiError(format!("子プロセスを待てない: {e}"))),
        }
    };

    // 子が終わっている以上、書き手も EOF か EPIPE で必ず解ける。
    if let Some(h) = stdin {
        let _ = h.join();
    }
    let out = join_pipe_reader(stdout);
    let err = join_pipe_reader(stderr);

    if !status.success() {
        return Err(AiError(format!(
            "{program} が異常終了した（{status}）: {}",
            tail_chars(err.trim(), 500)
        )));
    }
    if out.trim().is_empty() {
        return Err(AiError(format!(
            "{program} が何も返さなかった: {}",
            tail_chars(err.trim(), 500)
        )));
    }
    Ok(out)
}

/// 子プロセスのパイプをワーカースレッドで最後まで読み、UTF-8 として寛容に解く。
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

/// s の末尾 n 文字。文字境界を壊さない。
fn tail_chars(s: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match s.char_indices().nth_back(n - 1) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_placeholder_means_no_expansion() {
        let (out, saw) = expand_argv(&argv(&["cat"]), "P", Path::new("/repo"));
        assert_eq!(out, argv(&["cat"]));
        assert!(!saw);
    }

    #[test]
    fn placeholder_expands_inside_an_argument() {
        let (out, saw) = expand_argv(&argv(&["x", "pre{prompt}post"]), "P", Path::new("/repo"));
        assert_eq!(out, argv(&["x", "prePpost"]));
        assert!(saw);
    }

    #[test]
    fn workdir_expands_without_switching_the_input_channel() {
        let (out, saw) = expand_argv(&argv(&["x", "-w", "{workdir}"]), "P", Path::new("/repo"));
        assert_eq!(out, argv(&["x", "-w", "/repo"]));
        assert!(!saw, "{{workdir}} はプロンプトの受け渡しに影響しない");
    }

    #[test]
    fn tail_chars_keeps_char_boundaries() {
        assert_eq!(tail_chars("hello", 3), "llo");
        assert_eq!(tail_chars("hi", 5), "hi");
        assert_eq!(tail_chars("hi", 0), "");
        assert_eq!(tail_chars("あいうえお", 2), "えお");
    }

    // 実際に子プロセスを起こす側。sh に頼るので Unix だけ。
    #[cfg(unix)]
    mod subprocess {
        use super::*;

        fn sh(script: &str) -> Vec<String> {
            argv(&["sh", "-c", script])
        }

        #[test]
        fn without_a_placeholder_the_prompt_goes_to_stdin() {
            let out = run(
                &sh("cat"),
                Path::new("."),
                "SYS",
                "USER",
                Duration::from_secs(5),
            )
            .unwrap();
            assert_eq!(out, "SYS\n\nUSER");
        }

        /// プロンプトを位置引数で受け取るツールは、設定で直接指定する。
        /// ラッパースクリプト無しでそれを可能にするのが {prompt}。
        #[test]
        fn prompt_placeholder_delivers_via_argv_and_leaves_stdin_empty() {
            let out = run(
                &argv(&[
                    "sh",
                    "-c",
                    "printf 'argv=%s;' \"$1\"; printf 'stdin='; cat",
                    "sh",
                    "{prompt}",
                ]),
                Path::new("."),
                "SYS",
                "USER",
                Duration::from_secs(5),
            )
            .unwrap();
            assert_eq!(out, "argv=SYS\n\nUSER;stdin=");
        }

        /// モデルが自分でリポジトリを読みに行けるのは、ここが合っているから。
        #[test]
        fn runs_in_the_target_repository() {
            let dir = std::env::temp_dir();
            let out = run(&sh("pwd"), &dir, "s", "u", Duration::from_secs(5)).unwrap();
            assert_eq!(
                std::fs::canonicalize(out.trim()).unwrap(),
                std::fs::canonicalize(&dir).unwrap()
            );
        }

        #[test]
        fn nonzero_exit_surfaces_stderr() {
            let e = run(
                &sh("echo boom >&2; exit 1"),
                Path::new("."),
                "s",
                "u",
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(e.0.contains("boom"), "{}", e.0);
        }

        #[test]
        fn empty_success_is_an_error() {
            let e = run(
                &sh("exit 0"),
                Path::new("."),
                "s",
                "u",
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(e.0.contains("何も返さなかった"), "{}", e.0);
        }

        /// stdin を読まないコマンドに、パイプのバッファを超えるプロンプトを
        /// 渡しても打ち切れる。書き込みを待ってから時間を見る作りだと、
        /// ここで止まったまま timeout_secs が一度も効かない。
        #[test]
        fn a_command_that_never_reads_stdin_still_times_out() {
            let big = "x".repeat(1 << 20);
            let started = Instant::now();
            let e = run(
                &sh("sleep 30"),
                Path::new("."),
                &big,
                &big,
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert!(e.0.contains("打ち切った"), "{}", e.0);
            assert!(started.elapsed() < Duration::from_secs(10));
        }

        #[test]
        fn times_out_without_waiting_for_the_command() {
            let started = Instant::now();
            let e = run(
                &sh("sleep 5"),
                Path::new("."),
                "s",
                "u",
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert!(e.0.contains("打ち切った"), "{}", e.0);
            assert!(started.elapsed() < Duration::from_secs(4));
        }

        #[test]
        fn missing_program_names_the_program() {
            let e = run(
                &argv(&["revidere_no_such_binary_xyzzy"]),
                Path::new("."),
                "s",
                "u",
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(e.0.contains("revidere_no_such_binary_xyzzy"), "{}", e.0);
        }
    }
}
