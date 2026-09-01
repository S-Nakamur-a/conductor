//! アプリ全体で共有する語彙。
//!
//! ここに置くのは、どのサブシステムからも名前で参照される型だけに限る。`app` に置くと、
//! 型を 1 つ借りたいだけの下位モジュールが `app` 全体に依存して循環を作る。
//!
//! このモジュールは `keymap` 以外のクレート内モジュールを参照しない。参照が必要に
//! なった型は、ここではなく持ち主のモジュールへ置くこと。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// 現在キーボードフォーカスを持っているパネル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Worktree,
    Explorer,
    Viewer,
    TerminalClaude,
    TerminalShell,
    /// 埋め込みエディタパネル（PTY内のvim/emacs）。マージされた
    /// Explorer+Viewer領域を占有する。[App::editor] がSomeの間のみ到達可能。
    Editor,
    /// revidere の 2 列レビュービュー。他の 3 列とは並ばず画面全体を占有する
    /// ので、Tab の輪には入れていない — 入る口は w とパレットだけ、出る口は
    /// Esc/q と、Tab が Explorer へ抜けること。
    Revidere,
}

impl Focus {
    /// このパネルの基本キーマップコンテキスト。両方のターミナルは
    /// Terminalコンテキストを共有する（diff/コメントリストのようなサブ
    /// モードはパネル自身が別に追跡する）。
    pub fn key_context(self) -> crate::keymap::KeyContext {
        use crate::keymap::KeyContext;
        match self {
            Focus::Worktree => KeyContext::Worktree,
            Focus::Explorer => KeyContext::Explorer,
            Focus::Viewer => KeyContext::Viewer,
            Focus::TerminalClaude | Focus::TerminalShell => KeyContext::Terminal,
            Focus::Editor => KeyContext::Editor,
            Focus::Revidere => KeyContext::Revidere,
        }
    }

    /// コマンドパレットなど、パネルを名指しする場所で使う表示名。
    ///
    /// パネル番号オーバーレイ (ui::panel_overlay) はここを使わない。
    /// あちらは Explorer の下半分を「Diff List」として独立に数えるので、
    /// Focus と 1 対 1 で対応しない。
    pub fn label(self) -> &'static str {
        match self {
            Focus::Worktree => "Worktree",
            Focus::Explorer => "Explorer",
            Focus::Viewer => "Viewer",
            Focus::TerminalClaude => "Claude Code",
            Focus::TerminalShell => "Shell",
            Focus::Editor => "Editor",
            Focus::Revidere => "Review",
        }
    }

    /// Tab の輪で次に来るパネル。
    ///
    /// Revidere は画面全体を占有するので輪には並ばず、抜ける先だけを持つ。
    pub fn next_in_cycle(self) -> Focus {
        match self {
            Focus::Worktree | Focus::TerminalShell => Focus::Explorer,
            Focus::Explorer => Focus::Viewer,
            Focus::Viewer | Focus::Editor => Focus::TerminalClaude,
            Focus::TerminalClaude => Focus::TerminalShell,
            Focus::Revidere => Focus::Explorer,
        }
    }

    /// [Self::next_in_cycle] の逆回り。
    pub fn prev_in_cycle(self) -> Focus {
        match self {
            Focus::Worktree | Focus::Explorer | Focus::Editor => Focus::TerminalShell,
            Focus::Viewer => Focus::Explorer,
            Focus::TerminalClaude => Focus::Viewer,
            Focus::TerminalShell => Focus::TerminalClaude,
            Focus::Revidere => Focus::Explorer,
        }
    }

    /// このパネルがPTYをホストし、その内部のプログラム（Claude Code、
    /// シェル、エディタ）が生のキー入力を受け取るべきかどうか。イベント
    /// ディスパッチャはこれらのパネルをPTY転送経路に通す。キーマップが
    /// 奪い返すのは、[ターミナル内で発火する](crate::keymap::Action) コード
    /// だけだ。
    pub fn is_pty(self) -> bool {
        matches!(
            self,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        )
    }
}

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
