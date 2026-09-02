//! conductor cc-hook: Claude Code の SessionStart フック本体と、cc-notify ソケットの電文。
//!
//! フックはパネル自身の Claude プロセスの子として走るので、spawn 時に PTY へ注入した
//! CONDUCTOR_PANEL_ID / CONDUCTOR_NOTIFY_SOCK がそのまま見える。stdin の payload から
//! session_id を取り出し、[Notification::Session] を 1 行ソケットへ書いて終わる。
//! これで「どのパネルがいまどの .jsonl を書いているか」がログの推測でなく事実として届く。
//!
//! バイナリに同居させるのは、プラグイン側に置くと別リリースチャネルになり、ずれた
//! 組み合わせで黙って効かなくなるため (mcp-serve と同じ理由)。
//!
//! stdout には書かない。SessionStart の stdout はセッションへの追加コンテキストになる。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

mod settings;

pub use settings::{install_settings, socket_path};

pub const PANEL_ID_ENV: &str = "CONDUCTOR_PANEL_ID";
pub const NOTIFY_SOCK_ENV: &str = "CONDUCTOR_NOTIFY_SOCK";

/// cc-notify ソケットを流れる 1 行。書く側 (フック) と聞く側 (リスナ) の共通語彙。
///
/// どの id も UUID、cwd は行末まで丸ごとなので、空白区切りで曖昧にならない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    Active {
        cwd: PathBuf,
    },
    Waiting {
        cwd: PathBuf,
    },
    /// このパネルが書き込んでいる Claude セッションが変わった (起動、/resume、/clear)。
    Session {
        panel_id: String,
        session_id: String,
    },
}

impl Notification {
    pub fn parse(line: &str) -> Option<Self> {
        let (verb, rest) = line.trim().split_once(' ')?;
        match verb {
            "active" => Some(Self::Active {
                cwd: PathBuf::from(rest),
            }),
            "waiting" => Some(Self::Waiting {
                cwd: PathBuf::from(rest),
            }),
            "session" => {
                let (panel_id, session_id) = rest.split_once(' ')?;
                let (panel_id, session_id) = (panel_id.trim(), session_id.trim());
                (!panel_id.is_empty() && !session_id.is_empty()).then(|| Self::Session {
                    panel_id: panel_id.to_string(),
                    session_id: session_id.to_string(),
                })
            }
            _ => None,
        }
    }

    pub fn to_line(&self) -> String {
        match self {
            Self::Active { cwd } => format!("active {}\n", cwd.display()),
            Self::Waiting { cwd } => format!("waiting {}\n", cwd.display()),
            Self::Session {
                panel_id,
                session_id,
            } => format!("session {panel_id} {session_id}\n"),
        }
    }
}

/// stdin のフック payload を読み、このパネルの session id を Conductor へ通知する。
///
/// 常に Ok。Conductor が動いていない、payload が壊れている、環境変数が無い、のどれも
/// Claude の起動を止める理由にならない。
pub fn run() -> anyhow::Result<()> {
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return Ok(());
    }
    let (Ok(panel_id), Ok(sock_path)) =
        (std::env::var(PANEL_ID_ENV), std::env::var(NOTIFY_SOCK_ENV))
    else {
        return Ok(());
    };
    let Some(session_id) = session_id_from_payload(&payload) else {
        log::warn!("cc-hook: no session_id in hook payload");
        return Ok(());
    };

    // payload の source (startup / resume / clear) は見ない。どれも「このパネルはいま
    // この session を書いている」という同じ事実で、startup は pin 済みの id と同じ値になる。
    let line = Notification::Session {
        panel_id,
        session_id,
    }
    .to_line();
    // 1 回の write で送りきる。write! はフォーマット断片ごとに write を呼ぶので、
    // 受け手が 1 回の read で拾うと "session" だけ届くことがあった。
    if let Ok(mut stream) = UnixStream::connect(&sock_path) {
        let _ = stream.write_all(line.as_bytes());
        let _ = stream.flush();
    }
    Ok(())
}

fn session_id_from_payload(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let id = value.get("session_id")?.as_str()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloadからsession_idを取り出す() {
        // 実測した SessionStart (source=clear) の payload。
        let raw = r#"{"hook_event_name":"SessionStart","source":"clear",
            "session_id":"6c235e9c-6872-4ffc-a765-813a96c4e471",
            "cwd":"/tmp/wt","transcript_path":"/tmp/x.jsonl"}"#;
        assert_eq!(
            session_id_from_payload(raw).as_deref(),
            Some("6c235e9c-6872-4ffc-a765-813a96c4e471")
        );
    }

    #[test]
    fn 使えないpayloadはnone() {
        for raw in [
            "not json",
            r#"{"source":"clear"}"#,
            r#"{"session_id":""}"#,
            r#"{"session_id":42}"#,
        ] {
            assert!(session_id_from_payload(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn 電文は往復する() {
        let cases = [
            (
                "waiting /tmp/wt",
                Notification::Waiting {
                    cwd: PathBuf::from("/tmp/wt"),
                },
            ),
            (
                "active /tmp/my worktree",
                Notification::Active {
                    cwd: PathBuf::from("/tmp/my worktree"),
                },
            ),
            (
                "session panel-1 sess-2",
                Notification::Session {
                    panel_id: "panel-1".to_string(),
                    session_id: "sess-2".to_string(),
                },
            ),
        ];
        for (line, want) in cases {
            assert_eq!(Notification::parse(line).as_ref(), Some(&want), "{line}");
            assert_eq!(want.to_line(), format!("{line}\n"));
        }
    }

    #[test]
    fn 壊れた電文は拒む() {
        for line in [
            "session",
            "session only-one-id",
            "session  sess",
            "bogus /tmp/wt",
        ] {
            assert!(Notification::parse(line).is_none(), "{line}");
        }
    }
}
