//! App が抱えるバックグラウンド操作のハンドルと、その結果型。
//!
//! 共有語彙 (StatusLevel / Focus / worktree 操作の型など) は [crate::types] にある。

use crate::background::BackgroundOp;
use crate::git_engine;

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
    pub reflow_load: BackgroundOp<Vec<crate::reflow::log::LogEntry>>,
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
