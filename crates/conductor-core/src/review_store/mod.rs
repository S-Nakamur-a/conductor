//! SQLite (<git-root>/.conductor/conductor.db) に置くレビュー DB。
//!
//! インラインコメントとその返信、ブランチ単位のメタ情報、ターミナル出力の
//! スナップショット、再起動時に復元する表示位置を持つ。TUI と mcp-serve が
//! 同じファイルを同時に開くので WAL で運用する。
//!
//! [schema] がテーブル作成とマイグレーション、他のサブモジュールが 1 領域ずつ
//! impl ReviewStore を持つ。

mod comments;
mod model;
mod replies;
mod schema;
mod session_history;
mod templates;
mod view_state;
mod worktree_metadata;

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

pub use comments::MIN_ID_PREFIX_LEN;
pub use model::{
    Author, CommentKind, CommentStatus, CommentTemplate, NewReview, PrReviewMeta, ReviewComment,
    ReviewReply, SessionHistory,
};

/// リポジトリルートに対する conductor DB のパス。.conductor が無ければ作る。
pub fn db_path(repo_root: &Path) -> PathBuf {
    let dir = repo_root.join(".conductor");
    let _ = fs::create_dir_all(&dir);
    dir.join("conductor.db")
}

/// レビュー DB への接続。
pub struct ReviewStore {
    conn: Connection,
}
