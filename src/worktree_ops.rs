//! worktree 管理の状態。
//!
//! worktree の作成・削除に関する UI 状態と、バックグラウンド処理のチャネルを
//! まとめたもの。以前は App 構造体に散らばっていた。

use std::sync::mpsc;

use crate::app::{GrabbedBranch, PendingWorktree, WorktreeInputMode, WorktreeOpResult};
use crate::text_input::TextInput;

/// worktree 管理の状態。
pub struct WorktreeManager {
    /// worktree の作成・削除ダイアログの状態機械。
    pub input_mode: WorktreeInputMode,
    /// worktree 名の入力バッファ。
    pub input_buffer: TextInput,
    /// worktree の空白部分を最後にクリックした時刻 (ダブルクリック判定用)。
    pub blank_last_click: std::time::Instant,
    /// worktree バーの空白部分を最後にクリックした時刻 (ダブルクリックでの作成の
    /// 判定用)。blank_last_click と分けてあるのは、カラムの空白へのクリックと
    /// バーの空白へのクリックが誤ってダブルクリックとして結合しないようにするため。
    pub wtbar_blank_last_click: std::time::Instant,
    /// worktree 一覧の項目を最後にクリックした時刻 (ダブルクリック判定用)。
    pub item_last_click: std::time::Instant,
    /// 最後にクリックした worktree 一覧項目の添字。
    pub item_last_click_idx: usize,
    /// ステップ 1 で入力されたブランチ名。ステップ 2 (ベースブランチ選択) の
    /// あいだ保持する。
    pub pending_branch: String,
    /// worktree 作成のベースに選べるブランチの全一覧。
    pub base_branch_list: Vec<String>,
    /// ベースブランチ選択で現在選ばれている添字。
    pub base_branch_selected: usize,
    /// ベースブランチ一覧を絞り込むフィルタ文字列。
    pub base_branch_filter: TextInput,
    /// 現在 grab 中のブランチ情報 (ブランチ名と取得元 worktree のパス)。
    pub grabbed_branch: Option<GrabbedBranch>,
    /// ローカルブランチ一覧のキャッシュ (worktree と一緒に更新する)。
    pub local_branches: Vec<String>,
    /// バックグラウンドスレッドで実行中の worktree 操作。
    pub pending_worktrees: Vec<PendingWorktree>,
    /// worktree 操作の結果を送る側 (遅延生成)。
    pub bg_worktree_tx: Option<mpsc::Sender<WorktreeOpResult>>,
    /// worktree 操作の結果を受け取る側。
    pub bg_worktree_rx: Option<mpsc::Receiver<WorktreeOpResult>>,

    // Smart Worktree 関連のフィールド
    /// スマート worktree 作成で使う、複数行のタスク説明バッファ。
    pub smart_description_buffer: TextInput,
}

impl Default for WorktreeManager {
    fn default() -> Self {
        Self {
            input_mode: WorktreeInputMode::Normal,
            input_buffer: TextInput::new(),
            blank_last_click: std::time::Instant::now(),
            wtbar_blank_last_click: std::time::Instant::now(),
            item_last_click: std::time::Instant::now(),
            item_last_click_idx: usize::MAX,
            pending_branch: String::new(),
            base_branch_list: Vec::new(),
            base_branch_selected: 0,
            base_branch_filter: TextInput::new(),
            grabbed_branch: None,
            local_branches: Vec::new(),
            pending_worktrees: Vec::new(),
            bg_worktree_tx: None,
            bg_worktree_rx: None,
            smart_description_buffer: TextInput::new_multiline(),
        }
    }
}
