//! PTY セッションの保管庫。
//!
//! portable-pty で Claude Code / シェル / $EDITOR を起動し、reader スレッドが出力を
//! vt100 パーサと行バッファへ流し込む。
//!
//! ここだけ [crate::Services] の Event 経路に乗せていない。PTY のバイト列は描画より
//! 高頻度で届くのでチャネルが詰まる。UI は描画のたびに [PtyStore::screen] を直接
//! ロックし、届いたことの通知だけ [PtyStore::take_output_notify] で受け取る。

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result};
use portable_pty::NativePtySystem;

mod io;
mod locale;
mod reader;
mod screen;
mod spawn;
#[cfg(test)]
mod tests;

pub use io::{encode_mouse_wheel, sanitize_pasted_text, scroll_arrow_sequence};
pub use spawn::{Launch, Spawn};

/// リフローの再生のためにセッションごとに残す PTY 生出力の上限。
///
/// 再生は [PtyStore::resize_session] の中を同期で走るので、幅が変わるたび UI が止まる。
/// 512 KiB は既定のスクロールバックを覆いつつ最悪 1 フレームに収まる量。
const MAX_RAW_HISTORY_BYTES: usize = 512 * 1024;

/// 毒された Mutex でも中身は読める。PTY の状態は途中で壊れる類ではないので、
/// パニックの巻き添えで端末が固まる方を避ける。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// PTY セッションで動いているプロセスの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    ClaudeCode,
    Shell,
    /// 1 ファイルに対して起動する使い捨ての外部エディタ。プロセスの終了とともに破棄する。
    Editor,
}

/// reader スレッドとセッションが共有する状態。
#[derive(Clone)]
struct SharedIo {
    lines: Arc<Mutex<Vec<String>>>,
    /// 行バッファの上限。[PtyStore::activate_session] が後から動かすので共有の箱で持つ。
    line_limit: Arc<Mutex<usize>>,
    screen: Arc<Mutex<vt100::Parser>>,
    /// リサイズ時に新しい幅へ再生するための生バイト。
    ///
    /// 再生でリフローできるセッションだけが持つ。Claude やエディタのようなその場描画型は
    /// 絶対カーソル列と焼き込まれた改行でレイアウトするので、再生しても旧幅と同じ絵が
    /// 出るだけで、メモリと CPU を捨てることになる。
    raw_history: Option<Arc<Mutex<VecDeque<u8>>>>,
    last_output: Arc<Mutex<Instant>>,
    /// オルタネートスクリーンへの遷移を reader スレッドが立てる。
    alt_screen_entered: Arc<AtomicBool>,
    /// store 全体で 1 つ。新しい出力が届いたことをメインループへ知らせる。
    output_notify: Arc<AtomicBool>,
}

/// 1 つの PTY セッション。
pub struct PtySession {
    /// UUID v4。spawn 時に CONDUCTOR_PANEL_ID として子へ渡す値でもある。
    pub id: String,
    /// 人間可読なラベル (例: "CC:1")。
    pub label: String,
    pub kind: SessionKind,
    pub worktree: String,
    pub working_dir: PathBuf,

    /// このパネルを支える Claude Code セッション id (`<id>.jsonl`)。判明している場合のみ。
    claude_session_id: Option<String>,
    /// 子プロセスの起動時刻。古いセッションを --resume すると pin したログの mtime が
    /// 何日も前になりうるので、ローテーション追跡はこれを下限に使う。
    spawned_at: SystemTime,

    master: Box<dyn portable_pty::MasterPty + Send>,
    /// reader スレッドとも共有する。端末クエリへ最小レイテンシで応答するため。
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    io: SharedIo,

    /// オルタネートスクリーン突入を観測してからナッジを送り続ける期限。
    nudge_until: Option<Instant>,
    last_nudge: Option<Instant>,
}

impl PtySession {
    /// 最後に PTY 出力を受け取った時刻。
    pub fn last_output(&self) -> Instant {
        *lock(&self.io.last_output)
    }
}

/// PTY セッションの一覧を持ち、その寿命を握る。
pub struct PtyStore {
    pty_system: NativePtySystem,
    sessions: Vec<PtySession>,
    active_scrollback: usize,
    inactive_scrollback: usize,
    output_notify: Arc<AtomicBool>,
}

impl PtyStore {
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        Self {
            pty_system: NativePtySystem::default(),
            sessions: Vec::new(),
            active_scrollback,
            inactive_scrollback,
            output_notify: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 前回の呼び出し以降に新しい出力があったかを確認してクリアする。
    /// メインループはこれを見て poll のタイムアウトを飛ばし、即座に描画する。
    pub fn take_output_notify(&self) -> bool {
        self.output_notify.swap(false, Ordering::Relaxed)
    }

    pub fn sessions(&self) -> &[PtySession] {
        &self.sessions
    }

    /// idx のセッションの行バッファ上限を前面用へ引き上げる。
    /// 統合レイアウトでは Claude とシェルが同時に前面なので、他を引き下げはしない。
    pub fn activate_session(&self, idx: usize) {
        if let Some(session) = self.sessions.get(idx) {
            *lock(&session.io.line_limit) = self.active_scrollback;
        }
    }

    /// id 判明済みの Claude パネルなら、作業ディレクトリ・Claude セッション id・起動時刻。
    /// worktree の最新ではなく *このパネル自身* のログを開くために使う。
    pub fn claude_session_ref(&self, idx: usize) -> Option<(PathBuf, String, SystemTime)> {
        let session = self.sessions.get(idx)?;
        let id = session.claude_session_id.as_ref()?;
        Some((session.working_dir.clone(), id.clone(), session.spawned_at))
    }

    /// panel_id のパネルが書き込んでいる Claude セッションの id を差し替える。
    ///
    /// 呼ぶのは SessionStart フック経由の通知だけ。フックはそのパネル自身の Claude
    /// プロセスの中で走るので、これは推測ではなく事実。変わらなければ false。
    pub fn set_claude_session_id(&mut self, panel_id: &str, session_id: String) -> bool {
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.id == panel_id && s.kind == SessionKind::ClaudeCode)
        else {
            return false;
        };
        if session.claude_session_id.as_deref() == Some(session_id.as_str()) {
            return false;
        }
        session.claude_session_id = Some(session_id);
        true
    }

    /// idx 以外の Claude パネルが pin している session id の集合。
    /// /clear のローテーション追跡で、他パネルのログを後続候補から外すために使う。
    pub fn other_claude_session_ids(&self, idx: usize) -> HashSet<String> {
        self.sessions
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .filter_map(|(_, s)| s.claude_session_id.clone())
            .collect()
    }

    pub fn kill_session(&mut self, idx: usize) -> Result<()> {
        let session = self
            .sessions
            .get_mut(idx)
            .context("session index out of bounds")?;
        session
            .child
            .kill()
            .map_err(|e| anyhow::anyhow!("failed to kill session child process: {e}"))
    }

    /// idx のセッションを取り除く。drop で PTY マスターが閉じ、reader スレッドは
    /// EOF を見て終わる。
    pub fn remove_session(&mut self, idx: usize) {
        if idx < self.sessions.len() {
            self.sessions.remove(idx);
        }
    }

    pub fn is_session_alive(&mut self, idx: usize) -> bool {
        self.sessions
            .get_mut(idx)
            .is_some_and(|s| matches!(s.child.try_wait(), Ok(None)))
    }
}
