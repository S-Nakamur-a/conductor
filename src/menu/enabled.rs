//! コマンドが今実行可能かどうか — グレーアウト行の判定に使う。
//!
//! デフォルトは実行可能とする。ここにケースを書くのは、コマンド自身がすでに
//! 具体的な箇所で実行を拒否している場合だけで、各ケースにはその箇所を明記する。
//! この非対称性は意図的である。誤って false にすると、全操作を一覧するはずの
//! この UI から動くはずの操作が黙って消える。一方 true を誤っても今日の挙動
//! 以上の代償はない — コマンドはそのまま実行され、パレットから実行したときと
//! 同じようにステータスバーで結果を報告する。
//!
//! したがってケースを追加する際のルールは、対応する既存のチェック箇所を指し示す
//! ことである。コマンド名から前提条件を推測して作ってはいけない。
//!
//! コマンドの本当の前提条件が I/O(SQLite の読み取りや git の呼び出し)を
//! 要する場合は、そのうち安価な部分だけをここで再現する。この関数は開いている
//! ドロップダウンの表示中の行すべてに対して毎フレーム呼ばれ、アプリは 60fps で
//! 再描画するため、レビュー DB に問い合わせるような判定はレンダーループの中に
//! DB ラウンドトリップを持ち込むことになる。「実行可能」寄りに倒しておけば安全
//! であり、実行できない場合はコマンド自身がその理由を説明する。

use crate::app::App;
use crate::command_palette::CommandId;

/// id が現在のアプリ状態に対して実行可能かどうか。
///
/// アロケーションを抑え副作用を持たないこと — 呼び出される頻度については
/// モジュール冒頭の説明を参照。
pub fn command_enabled(id: CommandId, app: &App) -> bool {
    let selected_worktree = app.worktrees.selected();

    match id {
        // App
        // Action::UpdateAndRestart はリリースが見つかっている場合以外は
        // 何もしない(event/global.rs、if app.update.info.is_some())。
        CommandId::UpdateAndRestart => app.update.info.is_some(),

        // Repository
        // リポジトリ選択は切替先が複数あるときのみ開く
        // (event/global.rs、if app.repo.known.len() > 1)。
        CommandId::SwitchRepo => app.repo.known.len() > 1,

        // Worktree
        // ストリップの削除ボタンは main worktree と削除処理中の worktree を
        // 拒否する(event/mouse/bars.rs)。
        CommandId::DeleteWorktree => selected_worktree
            .is_some_and(|w| !w.is_main && !app.is_worktree_pending_delete(&w.path)),

        // "Cannot merge main into itself."(app/worktree_commands.rs)。
        CommandId::MergeToMain => selected_worktree.is_some_and(|w| !w.is_main),

        // "Already grabbing a branch. Ungrab first (G)."
        // (app/worktree_commands.rs)。後続の「grab 可能な非 main worktree が
        // ない」というチェックはここでは再現しない。オーバーレイ状態を変更する
        // load_grab_branches() が必要になるため。
        CommandId::GrabBranch => app.worktree_mgr.grabbed_branch.is_none(),

        // "Not grabbing — nothing to ungrab."(app/commands.rs)。
        CommandId::UngrabBranch => app.worktree_mgr.grabbed_branch.is_some(),

        // "No worktree selected."(app/worktree_pr.rs)。ブランチに実際に
        // PR があるかどうかは git 呼び出しが必要なので、そこはコマンド側に
        // 任せる。
        CommandId::OpenPullRequest => selected_worktree.is_some(),

        // Viewer
        // "Raw/Rendered applies to a markdown file in the Viewer"
        // (app/view_state.rs) — コマンドが参照するのと同じヘルパー。
        CommandId::ToggleMarkdownRender => app.viewer_state.markdown_toggle_available(),

        // Review
        // レビュー DB と worktree が必要(app/review_publish.rs)。ブランチに
        // 紐づく PR があるかは get_pr_review_meta クエリが必要なので、
        // そこはコマンド側に任せる。
        CommandId::PublishReview => app.review_store.is_some() && selected_worktree.is_some(),

        _ => true,
    }
}
