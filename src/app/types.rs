//! App の各所で共有する小さな状態型群: ステータスメッセージ、worktree 一覧の行、
//! worktree の入力/操作状態、ビュー復元用の記録、パネルの dirty 管理、バックグラウンド
//! 操作のハンドル。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::background::BackgroundOp;
use crate::git_engine;

/// ステータスメッセージの重要度・種別。スタイリングに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Success,
    Error,
    Warning,
    Info,
}

/// スタイル付き・時限表示のためのメタデータを持つステータスメッセージ。
#[derive(Debug, Clone)]
pub struct StatusMessage {
    /// メッセージ本文。
    pub text: String,
    /// 重要度レベル(色とアイコンを決める)。
    pub level: StatusLevel,
    /// このメッセージが作られた時点の ui_tick。
    pub created_at_tick: u64,
}

impl StatusMessage {
    pub fn new(text: String, level: StatusLevel, tick: u64) -> Self {
        Self {
            text,
            level,
            created_at_tick: tick,
        }
    }

    /// このメッセージレベルに対応するアイコン接頭辞を返す。
    pub fn icon(&self) -> &'static str {
        match self.level {
            StatusLevel::Success => "\u{2713} ", // ✓
            StatusLevel::Error => "\u{2717} ",   // ✗
            StatusLevel::Warning => "\u{26A1} ", // ⚡
            StatusLevel::Info => "\u{2139} ",    // ℹ
        }
    }
}

impl From<String> for StatusMessage {
    fn from(text: String) -> Self {
        Self {
            text,
            level: StatusLevel::Info,
            created_at_tick: 0,
        }
    }
}

/// ステータスバーへ流したい通知。App から切り出したサブシステムが、
/// set_status を呼ぶ代わりにこれを返す。
pub type Notice = (String, StatusLevel);

/// フラット化された worktree 一覧の1行(worktree 見出し行 + セッションのインライン行)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeListRow {
    /// worktrees[idx] にある worktree エントリ。
    Worktree(usize),
    /// ある worktree 配下の Claude Code セッション。
    Session { wt_idx: usize, pty_idx: usize },
}

/// worktree 操作時の入力モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeInputMode {
    /// 通常のナビゲーション。
    Normal,
    /// 新規 worktree のブランチ名を入力中(作成の1段階目)。
    CreatingWorktree,
    /// 新規 worktree のベースブランチを入力中(作成の2段階目)。
    CreatingWorktreeBase,
    /// worktree 削除の確認中(y/n)。
    ConfirmingDelete,
    /// ungrab の確認中(y/n)。
    ConfirmingUngrab,
    /// main を origin にハードリセットする確認中(y/n) — ローカルコミットが失われる。
    ConfirmingReset,
    /// Smart Worktree: 複数行のタスク説明を入力中。
    SmartDescription,
}

/// 保留中の worktree バックグラウンド操作の種別。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingWorktreeOp {
    Creating,
    Deleting,
    /// Smart worktree: LLM生成 + worktree作成をバックグラウンドで実行中。
    SmartCreating,
}

/// バックグラウンドスレッドで現在実行中の worktree 操作。
#[derive(Debug, Clone)]
pub struct PendingWorktree {
    pub branch: String,
    pub op: PendingWorktreeOp,
    pub base_ref: String,
    pub worktree_path: Option<PathBuf>,
    pub auto_spawn: bool,
    pub smart_prompt: String,
    /// Claude Code の --name フラグに渡すセッション名(smart worktree では LLM が生成)。
    pub session_name: Option<String>,
    pub delete_branch_after: bool,
    /// smart worktree のタスク説明(LLM 生成中の表示に使う)。
    pub description: String,
    /// この保留エントリが作られた時刻(タイムアウト検出用)。
    pub created_at: std::time::Instant,
    /// キャンセルトークン: true にするとバックグラウンドスレッドへキャンセルを要求する。
    pub cancel_token: Arc<AtomicBool>,
}

/// バックグラウンド worktree 操作の結果。
#[derive(Debug)]
pub enum WorktreeOpResult {
    Created {
        path: PathBuf,
        pending: PendingWorktree,
    },
    CreateFailed {
        error: String,
        pending: PendingWorktree,
    },
    Deleted {
        branch: String,
    },
    DeleteFailed {
        error: String,
        branch: String,
    },
    /// Smart worktree: LLM がブランチ名を解決した(UI更新用)。
    SmartBranchResolved {
        description: String,
        branch: String,
        prompt: String,
        session_name: Option<String>,
    },
    /// Smart worktree: 操作全体が失敗した。
    SmartFailed {
        description: String,
        error: String,
    },
}

/// smart worktree の LLM 生成結果。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SmartGenResult {
    pub branch: String,
    pub prompt: String,
    #[serde(default)]
    pub session_name: Option<String>,
}

/// grab したブランチの情報(main とのブランチチェックアウト入れ替え)。
#[derive(Debug, Clone)]
pub struct GrabbedBranch {
    /// 元のブランチ名(例: "feature-x")。
    pub branch: String,
    /// このブランチを元々持っていた worktree のパス。
    pub source_worktree: PathBuf,
    /// grab 元 worktree の Claude Code セッションID(grab 後の resume 用)。
    pub claude_session_id: Option<String>,
}

/// 保留中のビュー復元: 現在の worktree のファイルツリー読み込み完了後に、この
/// ファイルを開いてこの行までスクロールする。再起動後や worktree 切り替え時に
/// ユーザがいた位置へ戻すために使う。
#[derive(Debug, Clone)]
pub struct PendingViewRestore {
    /// 再度開くファイルの worktree 相対パス。
    pub file: String,
    /// スクロール先の一番上に表示する行(0始まり)。
    pub scroll: usize,
}

/// どの UI パネルが再描画を必要としているかを追跡する。
#[derive(Default, Clone, Copy)]
pub struct DirtyPanels(u8);

impl DirtyPanels {
    pub const WORKTREE: u8 = 0b0000_0001;
    pub const EXPLORER: u8 = 0b0000_0010;
    pub const VIEWER: u8 = 0b0000_0100;
    pub const TERMINAL: u8 = 0b0000_1000;
    pub const ALL: u8 = 0b0000_1111;

    /// 全パネルを dirty にする — App を新規構築した直後の初期値で、
    /// これにより最初のフレームで全パネルが描画される。
    pub fn all() -> Self {
        Self(Self::ALL)
    }

    pub fn mark(&mut self, bits: u8) {
        self.0 |= bits;
    }
    pub fn mark_all(&mut self) {
        self.0 = Self::ALL;
    }
    pub fn any(&self) -> bool {
        self.0 != 0
    }
    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// 60fps のイベントループが駆動するバックグラウンド操作群。App に非同期タスクごとの
/// フラットなフィールドを並べずに済むよう、ここにまとめている。各操作はメインループ
/// (または worktree 切り替えのハンドラ)からポーリングされ、結果を対応する App の
/// 状態へ書き戻す。
#[derive(Default)]
pub struct BackgroundOps {
    /// バックグラウンドの更新チェック(最新リリースの取得)。
    pub update_check: BackgroundOp<Option<crate::update_checker::UpdateInfo>>,
    /// バックグラウンドの ccusage(トークン/コスト)取得。
    pub ccusage: BackgroundOp<CcusageInfo>,
    /// バックグラウンドのブランチ一覧取得(ブランチ切り替えオーバーレイ用)。
    pub branch: BackgroundOp<Vec<String>>,
    /// バックグラウンドの pull 操作。
    pub pull: BackgroundOp<Result<String, String>>,
    /// バックグラウンドの gh pr view 参照。
    pub pr_url: BackgroundOp<Option<String>>,
    /// バックグラウンドの diff 計算(worktree 切り替え時)。
    pub diff: BackgroundOp<BgDiffResult>,
    /// バックグラウンドのファイルツリー走査(worktree 切り替え時): 走査したルート、
    /// エントリ一覧、それらと同時に取得した git ステータスのスナップショット。
    /// これにより poll_worktree_switch_ops() は FileTreeEntry::git_state を
    /// メインスレッドで再度 statuses() を呼ばずに埋められる。
    ///
    /// 根を結果に含めるのは、走査を始めた時点の選択と、結果が届いた時点の選択が
    /// 一致するとは限らないため。届いたエントリの相対パスを解決できるのは、
    /// それを歩いた根だけ。
    pub file_tree: BackgroundOp<(
        std::path::PathBuf,
        Vec<crate::viewer::FileTreeEntry>,
        crate::git_engine::status_map::GitStatusMap,
    )>,
    /// バックグラウンドのブランチ詳細計算(worktree 切り替え時)。
    pub branch_details: BackgroundOp<git_engine::BranchDetails>,
    /// バックグラウンドのシンボルインデックス構築。
    pub symbol_index: BackgroundOp<Result<usize, String>>,
    /// バックグラウンドの意味索引ロード。要求した時点の向き先を添えて返すのは、
    /// 読んでいる間に worktree が動いたかを受け取る側が判定できるようにするため。
    pub semantic_index: BackgroundOp<(
        std::path::PathBuf,
        crate::semantic_index::Survey,
        Option<sheaf_core::Store>,
    )>,
    /// リフロー式トランスクリプトビュー用のバックグラウンドセッションログ解析。
    pub reflow_load: BackgroundOp<Vec<crate::claude_log::LogEntry>>,
}

/// バックグラウンド diff 計算の結果。
pub struct BgDiffResult {
    pub files: Vec<crate::diff_state::FileDiff>,
    pub error: Option<String>,
}

/// ccusage から集計したトークン使用量とコスト。
#[derive(Debug, Clone)]
pub struct CcusageInfo {
    pub total_tokens: u64,
    pub total_cost: f64,
}
