//! conductor cc-hook — Claude Code の SessionStart フック本体。
//!
//! Conductor が spawn した Claude パネルには --settings で次のフックが
//! 差し込まれる (pty_manager::spawn)。フックはそのパネル自身の Claude
//! プロセスの子として走るので、spawn 時に PTY へ注入した環境変数
//! (CONDUCTOR_PANEL_ID / CONDUCTOR_NOTIFY_SOCK) がそのまま見える。
//!
//! やることは 1 つだけ: stdin に来た JSON から session_id を取り出し、
//! "session <panel id> <session id>" を cc-notify ソケットへ書いて終わる。
//! これで「どのパネルがいまどの .jsonl を書いているか」が、ログの中身から
//! 推測するのではなく事実として Conductor に届く。/clear は新しい session id
//! へログをローテーションするが、旧ログにも新ログにも相互参照が残らないため、
//! この経路が無いと推測に頼るしかない (claude_sessions::rotation がその
//! フォールバック)。
//!
//! シェルスクリプトにも jq にも依存しないのは、バイナリと signal を同じ
//! 成果物に載せるため。プラグイン側に置くと別リリースチャネルになり、
//! バージョンがずれた組み合わせで黙って効かなくなる (MCP サーバをバイナリへ
//! 取り込んだのと同じ理由 — CLAUDE.md 参照)。
//!
//! stdout には何も書かない。SessionStart フックの stdout はセッションへの
//! 追加コンテキストとして扱われるため。失敗しても Claude 側の起動は妨げない
//! ように、常に成功で返す。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

/// 環境変数名。spawn 側と合わせる。
pub const PANEL_ID_ENV: &str = "CONDUCTOR_PANEL_ID";
pub const NOTIFY_SOCK_ENV: &str = "CONDUCTOR_NOTIFY_SOCK";

/// stdin のフック payload を読み、パネルの現在の session id を Conductor へ通知する。
///
/// 常に Ok(())。Conductor が動いていない (ソケットが無い)、payload が壊れて
/// いる、環境変数が無い、のいずれもフックとしては正常系 — Claude の起動を
/// 止める理由にはならない。
pub fn run() -> anyhow::Result<()> {
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return Ok(());
    }

    let (Ok(panel_id), Ok(sock_path)) =
        (std::env::var(PANEL_ID_ENV), std::env::var(NOTIFY_SOCK_ENV))
    else {
        // Conductor 経由で起動されていない Claude。通知先が無いので何もしない。
        return Ok(());
    };

    let Some(session_id) = session_id_from_payload(&payload) else {
        log::warn!("cc-hook: no session_id in hook payload");
        return Ok(());
    };

    // source は見ない。startup/resume/clear のどれであれ「このパネルはいま
    // この session を書いている」という同じ事実を運ぶので、区別する意味がない。
    // startup では spawn 時に pin した id と同じ値が返るだけで、冪等。
    //
    // 1 回の write で送りきる。write!/writeln! はフォーマットの断片ごとに
    // write を呼ぶので、受け手が 1 回の read で拾うと "session" だけ届く
    // ことがある (実際にそうなった)。既存のシェル側フックが echo | nc で
    // 1 回書き込みだったため、この経路まで表面化していなかった。
    let line = format!("session {panel_id} {session_id}\n");
    if let Ok(mut stream) = UnixStream::connect(&sock_path) {
        let _ = stream.write_all(line.as_bytes());
        let _ = stream.flush();
    }
    Ok(())
}

/// フック payload の session_id。
fn session_id_from_payload(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let id = value.get("session_id")?.as_str()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_idを取り出す() {
        // 実測した SessionStart (source=clear) の payload 形。
        let raw = r#"{"hook_event_name":"SessionStart","source":"clear",
            "session_id":"6c235e9c-6872-4ffc-a765-813a96c4e471",
            "cwd":"/tmp/wt","transcript_path":"/tmp/x.jsonl"}"#;
        assert_eq!(
            session_id_from_payload(raw).as_deref(),
            Some("6c235e9c-6872-4ffc-a765-813a96c4e471")
        );
    }

    #[test]
    fn 使えない入力でも落ちない() {
        // 壊れた JSON / 欠けたキー / 空文字。どれもフックを失敗させない。
        assert!(session_id_from_payload("not json").is_none());
        assert!(session_id_from_payload(r#"{"source":"clear"}"#).is_none());
        assert!(session_id_from_payload(r#"{"session_id":""}"#).is_none());
        assert!(session_id_from_payload(r#"{"session_id":42}"#).is_none());
    }
}
