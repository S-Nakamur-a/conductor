//! SQLite を使ったレビュー/注釈データベース。
//!
//! rusqlite でコードレビューコメント、セッションのメタデータ、worktree の UI
//! 状態を保存し、アプリの再起動をまたいでレビュー状態を永続化する。
//!
//! データベースの実体は <git-root>/.conductor/conductor.db に置かれる。
//!
//! 責務ごとに分割している。[schema] はテーブル作成とバージョンベースの
//! マイグレーションを担当し、それ以外の各サブモジュールはそれぞれ1領域
//! （レビュー、返信、テンプレート、セッション履歴、worktree/PR メタデータ、
//! walkthrough、表示状態、ゲーミフィケーション統計）に対応する
//! impl ReviewStore ブロックを1つ持つ。このモジュール自体は ReviewStore
//! 構造体本体、db_path、そして外部から見えるパス crate::review_store::X を
//! 変えないための公開の再エクスポートだけを持つ。

mod comments;
mod model;
mod replies;
mod schema;
mod session_history;
mod stats;
mod templates;
#[cfg(test)]
mod test_support;
mod view_state;
mod walkthroughs;
mod worktree_metadata;

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

// PrReviewMeta、SessionStatsSnapshot、StreakInfo はモジュール外からこの
// re-export 経由でしか名指しされない（呼び出し側は型をインポートせずに
// get_pr_review_meta/end_stats_session/calculate_streak の戻り値に対して
// match するだけ）ため、rustc には re-export 自体が使われていると見えない。
#[allow(unused_imports)]
pub use model::{
    Author, CommentKind, CommentStatus, CommentTemplate, DailyStats, PrReviewMeta,
    ReviewComment, ReviewReply, SessionHistory, SessionStatsSnapshot, StreakInfo,
};

/// 指定したリポジトリルートに対する conductor データベースのパスを返す。
/// .conductor ディレクトリがまだなければ作成する。
pub fn db_path(repo_root: &Path) -> PathBuf {
    let dir = repo_root.join(".conductor");
    // ディレクトリ作成はベストエフォート。失敗した場合はデータベースファイルを
    // 開こうとした時点でエラーが表面化する。
    let _ = fs::create_dir_all(&dir);
    dir.join("conductor.db")
}

/// レビュー、セッション、worktree 状態を保持する SQLite データベースを管理する。
pub struct ReviewStore {
    conn: Connection,
}

#[cfg(test)]
mod tests {
    use super::db_path;

    #[test]
    fn db_path_creates_directory() {
        let tmp = std::env::temp_dir().join("conductor_test_db_path");
        // 前回実行の残骸を掃除する
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let path = db_path(&tmp);
        assert_eq!(path, tmp.join(".conductor").join("conductor.db"));
        assert!(tmp.join(".conductor").is_dir());

        // 後片付け
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
