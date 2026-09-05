use super::command::tail_chars;
use super::*;
use crate::config::ApiConfig;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn 使えるプロバイダの綴り() {
    let cases = [
        ApiConfig {
            provider: "gemini".to_string(),
            ..Default::default()
        },
        ApiConfig {
            provider: "GEMINI".to_string(),
            ..Default::default()
        },
        ApiConfig {
            provider: "  gemini  ".to_string(),
            ..Default::default()
        },
        ApiConfig::default(),
        ApiConfig {
            provider: "command".to_string(),
            command: argv(&["cat"]),
            ..Default::default()
        },
    ];
    for cfg in cases {
        assert!(
            build_caller(&cfg, &TaskEnv::default()).is_ok(),
            "{}",
            cfg.provider
        );
    }
}

/// claude は Conductor が自分で起動しないので、Claude を使う経路である command を
/// 指し示さねばならない。
#[test]
fn 使えないプロバイダは値と行き先を返す() {
    let cases = [
        (
            ApiConfig {
                provider: "claude".to_string(),
                ..Default::default()
            },
            ["claude", "command"],
        ),
        (
            ApiConfig {
                provider: "ollama".to_string(),
                ..Default::default()
            },
            ["ollama", "gemini"],
        ),
        (
            ApiConfig {
                provider: "command".to_string(),
                command: Vec::new(),
                ..Default::default()
            },
            ["command", "command"],
        ),
    ];
    for (cfg, wants) in cases {
        let err = build_caller(&cfg, &TaskEnv::default()).err().unwrap();
        for want in wants {
            assert!(err.contains(want), "{err}");
        }
    }
}

#[test]
fn tail_charsは末尾n文字を取る() {
    let cases = [
        ("hello", 3, "llo"),
        ("hi", 5, "hi"),
        ("hi", 0, ""),
        ("あいうえお", 2, "えお"),
    ];
    for (s, n, want) in cases {
        assert_eq!(tail_chars(s, n), want, "{s} {n}");
    }
}

#[cfg(unix)]
mod external_command {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use super::*;

    fn cmd_caller(cmd: &[&str], timeout_secs: u64) -> CommandCaller {
        CommandCaller {
            cmd: argv(cmd),
            timeout_secs,
            working_dir: None,
        }
    }

    fn run(caller: &CommandCaller) -> Result<String, String> {
        caller.complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn プロンプトの渡し方() {
        let cases = [
            (
                "プレースホルダが無ければstdin",
                vec!["sh", "-c", "cat"],
                "SYS\n\nUSER",
            ),
            (
                "引数の途中でも置換する",
                vec!["printf", "%s", "PRE[{prompt}]POST"],
                "PRE[SYS\n\nUSER]POST",
            ),
            // argv に載せたプロンプトが stdin にも届くとモデルが 2 回読む。
            // cat が後ろに足すので、空であることがそのまま見える。
            (
                "argvで渡したらstdinは空",
                vec![
                    "sh",
                    "-c",
                    "printf 'argv=%s;' \"$1\"; printf 'stdin='; cat",
                    "sh",
                    "{prompt}",
                ],
                "argv=SYS\n\nUSER;stdin=",
            ),
        ];
        for (name, cmd, want) in cases {
            assert_eq!(run(&cmd_caller(&cmd, 5)).unwrap(), want, "{name}");
        }
    }

    /// エージェント型のコマンドが問われている当のコードへ辿り着く唯一の手段なので、
    /// 実装の細部ではなくプロトコルの一部。
    #[test]
    fn コマンドはタスクのディレクトリを見る() {
        let dir = tempfile::tempdir().unwrap();
        // macOS は /var… の一時ディレクトリを /private/var… と報告する。
        let want = std::fs::canonicalize(dir.path()).unwrap();
        for cmd in [vec!["sh", "-c", "pwd"], vec!["printf", "%s", "{workdir}"]] {
            let caller = CommandCaller {
                cmd: argv(&cmd),
                timeout_secs: 5,
                working_dir: Some(dir.path().to_path_buf()),
            };
            let out = run(&caller).unwrap();
            assert_eq!(std::fs::canonicalize(out.trim()).unwrap(), want, "{cmd:?}");
        }
    }

    #[test]
    fn 失敗の理由を表に出す() {
        let cases = [
            (
                vec!["sh", "-c", "echo boom >&2; exit 1"],
                ["boom", "failed"],
            ),
            (vec!["sh", "-c", "exit 0"], ["empty", "empty"]),
            (
                vec!["definitely_not_a_real_binary_xyzzy"],
                ["definitely_not_a_real_binary_xyzzy", "Failed to spawn"],
            ),
        ];
        for (cmd, wants) in cases {
            let err = run(&cmd_caller(&cmd, 5)).unwrap_err();
            for want in wants {
                assert!(err.contains(want), "{cmd:?}: {err}");
            }
        }
    }

    /// 大きいプロンプトの事例は、書き込みを待ってから時間を見る作りだとここで
    /// 止まったままになることを固定する。レビューのプロンプトは実際にこの大きさ。
    #[test]
    fn 走り続けるコマンドはタイムアウトとキャンセルで止まる() {
        let big = "x".repeat(1 << 20);
        let cases = [
            (
                "コマンドの終了を待たない",
                "sleep 5",
                1,
                false,
                "s",
                "timed out",
            ),
            (
                "stdinを読まないコマンド",
                "sleep 30",
                1,
                false,
                big.as_str(),
                "timed out",
            ),
            ("先にキャンセル済み", "sleep 5", 0, true, "s", "Cancelled"),
        ];
        for (name, script, timeout_secs, cancelled, prompt, want) in cases {
            let caller = cmd_caller(&["sh", "-c", script], timeout_secs);
            let start = Instant::now();
            let err = caller
                .complete(prompt, prompt, &Arc::new(AtomicBool::new(cancelled)))
                .unwrap_err();
            assert!(err.contains(want), "{name}: {err}");
            assert!(start.elapsed() < Duration::from_secs(4), "{name}");
        }
    }

    /// 組み立てた caller を覗くのではなく振る舞いで見る。設定側はタイムアウトを
    /// 無効にしてあるので、kill されるのはタスク自身の値が届いたときだけ。
    #[test]
    fn タスク側のタイムアウトが設定値を上書きする() {
        let cfg = ApiConfig {
            provider: "command".to_string(),
            command: argv(&["sh", "-c", "sleep 30"]),
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
}
