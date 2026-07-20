//! SQLite-backed review/annotation database.
//!
//! Stores code review comments, session metadata, and worktree UI state using
//! `rusqlite` so that review state persists across application restarts.
//!
//! The database lives at `<git-root>/.conductor/conductor.db`.
//!
//! Split by responsibility: [`schema`] owns table creation and version-based
//! migrations, and each of the other submodules holds one `impl ReviewStore`
//! block for a single area (reviews, replies, templates, session history,
//! worktree/PR metadata, walkthroughs, view state, gamification stats). This
//! module only holds the `ReviewStore` struct itself, `db_path`, and the
//! public re-exports that keep the external path `crate::review_store::X`
//! unchanged.

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

// PrReviewMeta, SessionStatsSnapshot, and StreakInfo are only named via this
// re-export from outside the module (callers match on the return values of
// get_pr_review_meta/end_stats_session/calculate_streak without importing the
// type), so rustc can't see the re-export itself as used.
#[allow(unused_imports)]
pub use model::{
    Author, CommentKind, CommentStatus, CommentTemplate, DailyStats, PrReviewMeta,
    ReviewComment, ReviewReply, SessionHistory, SessionStatsSnapshot, StreakInfo,
};

/// Return the path to the conductor database for a given repository root,
/// creating the `.conductor` directory if it does not yet exist.
pub fn db_path(repo_root: &Path) -> PathBuf {
    let dir = repo_root.join(".conductor");
    // Best-effort directory creation; errors will surface when we try to open
    // the database file.
    let _ = fs::create_dir_all(&dir);
    dir.join("conductor.db")
}

/// Manages the SQLite database for reviews, sessions, and worktree state.
pub struct ReviewStore {
    conn: Connection,
}

#[cfg(test)]
mod tests {
    use super::db_path;

    #[test]
    fn db_path_creates_directory() {
        let tmp = std::env::temp_dir().join("conductor_test_db_path");
        // Clean up from any previous run
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let path = db_path(&tmp);
        assert_eq!(path, tmp.join(".conductor").join("conductor.db"));
        assert!(tmp.join(".conductor").is_dir());

        // Clean up
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
