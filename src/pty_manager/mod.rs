//! PTY セッション管理。
//!
//! portable-pty を使い、ユーザーが TUI 内でシェルコマンドや Claude Code を直接
//! 実行できるよう疑似端末セッションを起動・管理する。
//!
//! 各セッションは実際の疑似端末に支えられ、バックグラウンドの reader スレッドが
//! 出力を有界の行バッファへ取り込む。
//!
//! ここには PtySession/PtyManager 型とライフサイクルメソッドだけを置き、
//! それ以外の振る舞いは責務ごとにサブモジュールへ分割している。

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

/// リフロー時の巻き戻し再生のためセッションごとに保持する、PTY の生出力
/// バイト数の上限。端末幅が変わると vt100 は既存の内容をリフローできない
/// ため、このバイト履歴を新しい幅で再生してパーサを作り直す。
///
/// この再生は resize_session 内でメインスレッド上で同期的に実行されるため、
/// そのコストは幅が変わるたびに(パネル最大化、共有右カラムをリサイズする
/// フォーカス切り替え、Tab、tmux式リサイズなど)UIのストールとして
/// 発生する。そのため上限は控えめに設定している: 512 KiBあれば、典型的な
/// シェルの行長でのデフォルトのアクティブスクロールバック(10000行)を
/// 十分にカバーしつつ、最悪でも再生コストを数十msではなく1フレーム分の
/// ストールに抑えられる。上限を超えたバイトは行境界で切り捨てる —
/// リフローで失われるのは、すでに画面外に出ている古い履歴だけである。
const MAX_RAW_HISTORY_BYTES: usize = 512 * 1024;

// SessionKind

/// PTY セッション内で動いているプロセスの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    ClaudeCode,
    Shell,
    /// 単一ファイルに対して起動する使い捨ての外部エディタ ($VISUAL / $EDITOR)。
    /// エディタプロセスの終了とともに破棄され、Claude 出力のスキャナ
    /// (waiting/active 検出) の対象外。
    Editor,
}

// PtySession

/// 対応する reader/writer ハンドルを持つ単一の PTY セッション。
pub struct PtySession {
    /// UUID v4。
    pub id: String,
    /// 人間可読なラベル (例: "Auth logic implementation")。
    pub label: String,
    pub kind: SessionKind,
    pub worktree: String,
    pub working_dir: PathBuf,
    /// このパネルを支える Claude Code セッション id (プロジェクトディレクトリ配下の
    /// <id>.jsonl)。判明している場合のみ設定する。ClaudeCode セッションでは、
    /// 新規起動時は --session-id で生成した id を強制し、resume 起動時は
    /// resume した id を記録する。Shell/Editor セッション、および id を特定
    /// できなかった Claude セッションでは None。これにより reflow トランスクリプト
    /// ビューが worktree の最新セッションではなく *このパネル自身* のログを開ける —
    /// 1つの worktree に複数の Claude パネル (CC:1, CC:2, …) がある場合に必須。
    pub claude_session_id: Option<String>,
    /// 子プロセスを起動した時刻。
    ///
    /// /clear によるセッションログのローテーション追跡で、起動より前に
    /// 始まっていたログを後続と誤認しないための下限として使う (古いセッションを
    /// --resume した直後は pin したログの mtime が何日も前になりうる)。
    pub spawned_at: std::time::SystemTime,
    pub is_active: bool,

    // PTY のハンドル
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// reader スレッドと共有する。端末クエリ (カーソル位置レポートなど) へ
    /// 最小レイテンシで応答するため。
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,

    // 出力バッファ (リーダースレッドと共有)
    output_buffer: Arc<Mutex<Vec<String>>>,
    max_buffer_lines: usize,

    // vt100 のターミナルエミュレータ
    screen: Arc<Mutex<vt100::Parser>>,
    /// 追記のみ(有界)の PTY 生出力バイト履歴。reader スレッドと共有する。
    /// vt100 自体は既存内容をリフローしないため、リサイズ時にこの履歴を
    /// 新しい幅で再生して vt100 パーサを作り直すのに使う。追記は常に
    /// screen のロックを保持したまま行い、parser.process とアトミックに
    /// 保ち、resize_session の再構築とも整合させている。
    ///
    /// 再生ベースのリフローが恩恵をもたらさないセッションでは None。
    /// 再生が再ラップするのは、端末の自動折り返し(ソフトラップ)に頼っていた
    /// 内容 — 通常のシェル出力など — だけである。Claude Code のような
    /// その場描画型のアプリは、絶対カーソル列エスケープと現在の幅で
    /// 焼き込まれたハード改行で全行をレイアウトするため、新しい幅で
    /// バイトを再生しても旧幅と同一のレイアウトが再現されるだけで、
    /// リフローにはならず、メモリと CPU の無駄になる。そうしたセッションは
    /// 記録自体を丸ごとスキップする。
    raw_history: Option<Arc<Mutex<VecDeque<u8>>>>,

    // 入力待ちの検出
    /// reader スレッドと共有する。
    pub last_output_time: Arc<Mutex<Instant>>,

    // 代替スクリーンの検出
    /// オルタネートスクリーンモードへの遷移を検出すると reader スレッドが
    /// true にする。メインループはこのフラグを見て、子プロセスに再描画を
    /// 促す no-op リサイズ(SIGWINCH)を送れる。
    pub alt_screen_entered: Arc<AtomicBool>,

    /// メインループが alt_screen_entered を最初に観測したときに設定する。
    alt_screen_nudge_until: Option<Instant>,
    /// スロットリング用。
    last_nudge_time: Option<Instant>,
}

// PtyManager

/// 1つ以上の PTY セッションを管理する。
pub struct PtyManager {
    pty_system: NativePtySystem,
    sessions: Vec<PtySession>,
    /// sessions と同じインデックスで対応する。reader スレッドと共有する。
    buffer_limits: Vec<Arc<Mutex<usize>>>,
    active_scrollback: usize,
    inactive_scrollback: usize,
    /// 新しい PTY 出力が届くと reader スレッドがセットするフラグ。
    /// メインループはこれを見て poll のタイムアウトを飛ばし即座に描画する。
    output_notify: Arc<AtomicBool>,
}

impl PtyManager {
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        Self {
            pty_system: NativePtySystem::default(),
            sessions: Vec::new(),
            buffer_limits: Vec::new(),
            active_scrollback,
            inactive_scrollback,
            output_notify: Arc::new(AtomicBool::new(false)),
        }
    }

    /// PTY 出力通知フラグを確認してクリアする。
    ///
    /// 前回の呼び出し以降にいずれかの reader スレッドが新しい出力を生成して
    /// いれば true を返す。メインループが poll のタイムアウトを飛ばして
    /// PTY の変化を即座に描画するために使う。
    pub fn take_output_notify(&self) -> bool {
        self.output_notify.swap(false, Ordering::Relaxed)
    }

    /// 他のセッションを非アクティブにせずに、あるセッションをアクティブ化する。
    /// Claude と Shell のセッションが同時にアクティブになりうる統合レイアウトで使う。
    pub fn activate_session(&mut self, idx: usize) {
        if let Some(session) = self.sessions.get_mut(idx) {
            session.is_active = true;
            session.max_buffer_lines = self.active_scrollback;
        }
        if let Some(limit) = self.buffer_limits.get(idx) {
            let mut l = limit.lock().unwrap_or_else(|e| e.into_inner());
            *l = self.active_scrollback;
        }
    }

    /// セッション数を返す。
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// idx のセッションが id 判明済みの Claude パネルである場合、その
    /// Claude セッション id・作業ディレクトリ・起動時刻を返す。reflow
    /// トランスクリプトビューが、worktree の最新セッションではなく
    /// *特定の* パネルのログを開くために使う。範囲外のインデックス、
    /// Claude 以外のセッション、id 不明の Claude セッションでは None。
    pub fn claude_session_ref(&self, idx: usize) -> Option<(PathBuf, String, SystemTime)> {
        let session = self.sessions.get(idx)?;
        let id = session.claude_session_id.as_ref()?;
        Some((session.working_dir.clone(), id.clone(), session.spawned_at))
    }

    /// panel_id のパネルが書き込んでいる Claude セッションの id を差し替える。
    ///
    /// 呼ぶのは SessionStart フック経由の通知 ([crate::cc_hook]) だけ。
    /// フックはそのパネル自身の Claude プロセスの中で走るので、これは推測では
    /// なく事実。/clear や /resume でログがローテーションしても、以降の
    /// トランスクリプト解決が正しいファイルを指す。
    ///
    /// 該当パネルが無い (すでに閉じた等) 場合や、値が変わらない場合は false。
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
    ///
    /// /clear のローテーション追跡で、他パネルが自分のログとして持っている
    /// セッションを後続候補から外すために使う。
    pub fn other_claude_session_ids(&self, idx: usize) -> HashSet<String> {
        self.sessions
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .filter_map(|(_, s)| s.claude_session_id.clone())
            .collect()
    }

    /// sessions スライスへの読み取り専用アクセス。
    pub fn sessions(&self) -> &[PtySession] {
        &self.sessions
    }

    /// 指定インデックスのセッションの子プロセスを kill する。
    pub fn kill_session(&mut self, idx: usize) -> Result<()> {
        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        session
            .child
            .kill()
            .map_err(|e| anyhow::anyhow!("Failed to kill session child process: {e}"))?;
        Ok(())
    }

    /// idx のセッションを削除し、リソースを片付ける。
    ///
    /// セッションを drop すると PTY マスターが閉じられ、バックグラウンドの
    /// reader スレッドは EOF を検知して終了する。
    pub fn remove_session(&mut self, idx: usize) {
        if idx < self.sessions.len() {
            self.sessions.remove(idx);
            self.buffer_limits.remove(idx);
        }
    }

    /// idx のセッションの子プロセスがまだ動作しているかを確認する。
    pub fn is_session_alive(&mut self, idx: usize) -> bool {
        self.sessions
            .get_mut(idx)
            .map(|s| {
                match s.child.try_wait() {
                    Ok(Some(_exit_status)) => false, // 終了済み
                    Ok(None) => true,                // まだ動作中
                    Err(_) => false,                 // エラーは死んでいる扱い
                }
            })
            .unwrap_or(false)
    }
}
