//! ウィンドウと PTY ホストのあいだを流れる電文。
//!
//! 1 フレームは `[u32 長さ][u8 種別][本体]`。種別 0 は JSON の制御メッセージ、
//! 1 はセッション宛てのバイト列 (`[u8 id 長][id][データ]`)。打鍵と画面の差分は
//! 頻度が高いので JSON に包まない。

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::SessionKind;

/// 電文の形を変えたら上げる。ホストはウィンドウより長生きするので、
/// 版の違う相手と話していることを最初の挨拶で知る必要がある。
pub(super) const PROTOCOL: u32 = 1;

/// 壊れた長さを信じてメモリを確保しないための歯止め。
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

const CONTROL: u8 = 0;
const BYTES: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(super) enum ToHost {
    Hello {
        version: String,
        protocol: u32,
    },
    Spawn(SpawnSpec),
    Resize {
        id: String,
        rows: u16,
        cols: u16,
    },
    Kill {
        id: String,
    },
    Remove {
        id: String,
    },
    /// セッションが 1 つも無いときに、版の古いホストを退かせる。
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SpawnSpec {
    pub info: SessionInfo,
    pub argv: Vec<String>,
    /// 子へ渡す環境のすべて。ホストの環境は最初のウィンドウのものなので継がせない。
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SessionInfo {
    pub id: String,
    pub label: String,
    pub kind: SessionKind,
    pub worktree: String,
    pub working_dir: PathBuf,
    pub claude_session_id: Option<String>,
    pub spawned_at_ms: u64,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(super) enum ToWindow {
    /// 挨拶への返事。直後に各セッションの画面がバイト列で 1 つずつ続く。
    Welcome {
        version: String,
        protocol: u32,
        sessions: Vec<SessionInfo>,
        /// ウィンドウが居ない間に届いた cc-notify のうち、今も効いているもの。
        notifications: Vec<String>,
    },
    Spawned {
        id: String,
        error: Option<String>,
    },
    Exited {
        id: String,
    },
    Notify {
        line: String,
    },
    /// 別のウィンドウがつながっている。
    Busy,
}

pub(super) enum Frame<T> {
    Control(T),
    Bytes { id: String, data: Vec<u8> },
}

/// 1 フレームを 1 回の write で送る。複数のスレッドが同じ接続へ書くので、
/// 途中で他のフレームに割り込まれないようにする。
pub(super) fn write_control(w: &mut impl Write, msg: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write_frame(w, CONTROL, &body)
}

pub(super) fn write_bytes(w: &mut impl Write, id: &str, data: &[u8]) -> io::Result<()> {
    let id_len = u8::try_from(id.len()).map_err(io::Error::other)?;
    let mut body = Vec::with_capacity(1 + id.len() + data.len());
    body.push(id_len);
    body.extend_from_slice(id.as_bytes());
    body.extend_from_slice(data);
    write_frame(w, BYTES, &body)
}

fn write_frame(w: &mut impl Write, kind: u8, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len() + 1).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(5 + body.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.push(kind);
    frame.extend_from_slice(body);
    w.write_all(&frame)?;
    w.flush()
}

pub(super) fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Frame<T>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len}"),
        ));
    }
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame)?;
    let (kind, body) = (frame[0], &frame[1..]);
    match kind {
        CONTROL => serde_json::from_slice(body)
            .map(Frame::Control)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        BYTES => {
            let id_len = *body.first().ok_or(io::ErrorKind::InvalidData)? as usize;
            let id = body
                .get(1..1 + id_len)
                .and_then(|id| std::str::from_utf8(id).ok())
                .ok_or(io::ErrorKind::InvalidData)?
                .to_string();
            Ok(Frame::Bytes {
                id,
                data: body[1 + id_len..].to_vec(),
            })
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame kind {kind}"),
        )),
    }
}
