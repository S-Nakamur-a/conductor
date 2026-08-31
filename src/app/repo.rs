//! リポジトリの選択とキャッシュされたworktree一覧: 既知のリポジトリ間の
//! 切り替え、任意パスのオープン、gitからのworktree/ブランチ状態のリフレッシュ。

use crate::explorer::Explorer;
use crate::git_engine;
use crate::review_store::{self, ReviewStore};
use crate::viewer::ViewerState;

use super::{App, StatusLevel};

impl App {
    /// repo_list 内のインデックスで別のリポジトリへ切り替える。
    pub fn switch_repo(&mut self, index: usize) {
        if index >= self.repo.known.len() {
            return;
        }
        // ストアを差し替える前に、離脱するリポジトリのビューを永続化する。
        self.persist_view_state();
        // 走っている意味索引の生成は離脱するリポジトリのもの。repo.path を
        // 差し替える前に止める。あとで止めると、そのリポジトリの成果物を
        // 切り替え先の .conductor へ置きに行く。
        let leaving = self.repo.path.clone();
        self.code_nav.semantic.abort_regeneration(&leaving);
        self.repo.known_index = index;
        self.repo.path = self.repo.known[index].clone();

        // 新しいリポジトリパス用にレビューストアを開き直す。
        let db = review_store::db_path(&self.repo.path);
        self.review_store = match ReviewStore::open(&db) {
            Ok(store) => Some(store),
            Err(e) => {
                log::warn!("failed to open review store for new repo: {e}");
                None
            }
        };

        // 新しいリポジトリ用にmainリポジトリ名を更新する。
        self.repo.main_name = git_engine::GitEngine::open(&self.repo.path)
            .and_then(|engine| engine.main_worktree_path())
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| {
                self.repo
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| self.repo.path.display().to_string())
            });

        // worktreeとレビューを即座にリフレッシュする。viewer/diffは遅延読み込みする。
        self.worktrees.select(0);
        self.refresh_worktrees();
        self.explorer = Explorer::default();
        self.viewer = ViewerState::default();
        // ツリーの走査は遅延させる (上のコメントのとおり) が、根だけは今ここで
        // 入れておく。空のままだと、ツリーを歩く前に開いたファイル名検索が
        // カレントディレクトリを歩いてしまう。
        self.explorer.set_root(self.selected_worktree_path());
        self.diff_state = crate::diff_state::DiffState::new(
            &self.config.general.main_branch,
            self.diff_state.view_mode,
        );
        // 新しいリポジトリの最後に選択していたworktree + 開いていたファイル/
        // スクロールを復元する。
        self.restore_selected_worktree_and_view();
        // worktreeの選択が新しいリポジトリに落ち着いたので、シンボル索引を
        // 再照準する。これをしないと、索引はいま離れたリポジトリからの回答を
        // 返し続ける: src/app/mod.rs のようなパスは両方に存在するので、ジャンプは
        // 新しいリポジトリのファイルなのに古いリポジトリの行番号に着地してしまい、
        // ホバーポップアップはそのテキストを完全に古いツリーから読んでしまう。
        // worktree切り替えは on_worktree_changed を経由するが、リポジトリの
        // 切り替えは決してそこを通らない。
        self.start_symbol_index_build();
        // 意味索引も同じ理由で照準し直す。worktree 切替は on_worktree_changed を
        // 通るが、リポジトリの切替はそこを通らない。読み直さないと、離れた
        // リポジトリのストアを抱えたまま構文層に落ち続ける。
        self.start_semantic_index_load();
        self.refresh_reviews();
        self.terminal.claude.active_session = None;
        self.terminal.shell.active_session = None;

        self.set_status(
            format!("Switched to repository: {}", self.repo.main_name),
            StatusLevel::Success,
        );
    }

    /// 任意のファイルシステムパスからリポジトリを開く。
    pub fn open_repo_from_path(&mut self, path: &str) {
        // ~ をホームディレクトリに展開する。
        let expanded = if let Some(stripped) = path.strip_prefix('~') {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped.strip_prefix('/').unwrap_or(stripped))
            } else {
                std::path::PathBuf::from(path)
            }
        } else {
            std::path::PathBuf::from(path)
        };

        // 可能なら正規化し、できなければそのまま使う。
        let canonical = expanded.canonicalize().unwrap_or(expanded);

        if !canonical.is_dir() {
            self.set_status(
                format!("Not a directory: {}", canonical.display()),
                StatusLevel::Error,
            );
            return;
        }

        // このパスにgitリポジトリがあるか調べる。
        match git_engine::GitEngine::open(&canonical) {
            Ok(_engine) => {
                // 有効なgitリポジトリ — それに切り替える。
                self.repo.path = canonical.clone();

                // 新しいリポジトリパス用にレビューストアを開き直す。
                let db = review_store::db_path(&self.repo.path);
                self.review_store = match ReviewStore::open(&db) {
                    Ok(store) => Some(store),
                    Err(e) => {
                        log::warn!("failed to open review store for new repo: {e}");
                        None
                    }
                };

                self.worktrees.select(0);
                self.refresh_worktrees();
                self.explorer = Explorer::default();
                self.viewer = ViewerState::default();
                // 同上。ツリーは遅延させるが根は今決まっている。
                self.explorer.set_root(self.selected_worktree_path());
                // このリポジトリにはビュー復元が無いので、*前の*リポジトリ用にまだ
                // 有効な復元があれば破棄する — そうしないと、ここで発火して新しく
                // 開いたツリー内の同名パスを開いてしまう可能性がある。
                self.view_restore.pending = None;
                self.diff_state = crate::diff_state::DiffState::new(
                    &self.config.general.main_branch,
                    self.diff_state.view_mode,
                );
                self.refresh_reviews();
                self.terminal.claude.active_session = None;
                self.terminal.shell.active_session = None;

                // まだ無ければrepo_listに追加する。
                if !self.repo.known.contains(&canonical) {
                    self.repo.known.push(canonical.clone());
                }
                // repo_list_indexがこのリポジトリを指すように更新する。
                self.repo.known_index = self
                    .repo
                    .known
                    .iter()
                    .position(|p| p == &canonical)
                    .unwrap_or(0);

                let repo_name = canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| canonical.display().to_string());
                self.set_status(
                    format!("Opened repository: {repo_name}"),
                    StatusLevel::Success,
                );
            }
            Err(e) => {
                self.set_status(
                    format!("Not a git repository: {} ({e})", canonical.display()),
                    StatusLevel::Error,
                );
            }
        }
    }

    /// リポジトリからキャッシュされたworktreeリストをリフレッシュする。
    ///
    /// worktreeリストが実際に変わった場合（件数、ブランチ名、ステータス件数の
    /// いずれかが異なる場合）に true を返す。呼び出し側は何も変わっていない
    /// ときに再描画をスキップできる。
    pub fn refresh_worktrees(&mut self) -> bool {
        let mut changed = false;
        // リストを差し替える*前に*どのブランチが選択されているかを覚えておき、
        // 後でその同一性に選択をピン留めできるようにする（worktreeが追加/削除
        // されるとリストの順序がずれることがある）。
        let prev_selected_branch = self.selected_worktree_branch();
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => {
                match engine.list_worktrees() {
                    Ok(worktrees) => {
                        // 差し替える前にworktreeリストが変わったかどうかを検出する。
                        if worktrees.len() != self.worktrees.len() {
                            changed = true;
                        } else {
                            for (old, new) in self.worktrees.iter().zip(worktrees.iter()) {
                                if old.branch != new.branch
                                    || old.added != new.added
                                    || old.modified != new.modified
                                    || old.deleted != new.deleted
                                    || old.is_clean != new.is_clean
                                {
                                    changed = true;
                                    break;
                                }
                            }
                        }
                        self.worktrees.replace(worktrees);
                        // リスト位置ではなく*ブランチの同一性*で選択を保つ: worktreeが追加・
                        // 削除されるとインデックスはずれる。ブランチを再検索することで、
                        // 選択が黙って隣のworktreeへスライドするのではなく、同じworktreeへ
                        // ピン留めされ続ける。ブランチが無くなった場合（そのworktreeが削除
                        // された場合）のみクランプへフォールバックする。
                        if let Some(idx) = reselect_worktree_index(
                            &self.worktrees,
                            &prev_selected_branch,
                            self.worktrees.selected_index(),
                        ) {
                            self.worktrees.select(idx);
                        }
                        // HEAD oidの変化からコミットを検出する。oidは list_worktrees が
                        // 各リポジトリを開いていた時点で取得済みなので、ここでworktreeごとに
                        // Repository::open を再度呼ぶ必要はない。
                        let head_updates: Vec<(String, String)> = self
                            .worktrees
                            .iter()
                            .filter_map(|wt| {
                                wt.head_oid.clone().map(|oid| (wt.branch.clone(), oid))
                            })
                            .collect();
                        for (branch, head_oid) in head_updates {
                            if let Some(old) = self.change_watch.heads.get(&branch)
                                && old != &head_oid
                            {
                                self.record_stat("commits_made");
                                changed = true;
                            }
                            self.change_watch.heads.insert(branch, head_oid);
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to list worktrees: {e}");
                    }
                }
                // 詳細ゾーン用にローカルブランチをリフレッシュする。
                if let Ok(branches) = engine.list_local_branches() {
                    if branches != self.worktree_mgr.local_branches {
                        changed = true;
                    }
                    self.worktree_mgr.local_branches = branches;
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository: {e}");
            }
        }
        self.rebuild_worktree_list_rows();
        // 選択中worktreeのブランチが足元で変わっていたら（そのworktreeが
        // 削除され、選択が別のブランチ — 多くはmain worktree — へフォールバック
        // した場合）、レビュー状態を再読み込みする。そうしないと、前のブランチの
        // 変更サマリとコメントが残ったまま、間違ったブランチに対して表示され
        // 続けてしまう（例: マージ済みPRのサマリが main に表示されるなど）。
        //
        // *空*のブランチは除外する: list_worktrees は検査に失敗したworktreeを
        // ログに残してスキップするので（git_engine::worktree_ops 参照）、
        // 一時的なgitエラーで1回のポーリングだけリストが空になることがある。
        // それは読み取り失敗であって選択の変更ではなく、"" に対してレビューを
        // 再読み込みすると、復旧するまで数秒おきにパネルが空白になってしまう。
        let new_branch = self.selected_worktree_branch();
        if !new_branch.is_empty() && new_branch != prev_selected_branch {
            self.refresh_reviews();
        }
        changed
    }

    /// デコレーションアニメーションを1ティック進める。アニメーションが
    /// 実際に更新された場合（つまりmodeが None でない場合）に true を返す。
    pub fn tick_decoration(&mut self, width: u16, height: u16) -> bool {
        use crate::worktree::decoration::{DecorationActivity, DecorationMode};
        let mode = DecorationMode::from_str(&self.config.general.decoration);
        if !mode.has_animation() {
            return false;
        }
        self.ticks.advance_decoration();
        let activity = if self.terminal.cc_waiting_worktrees.is_empty() {
            DecorationActivity::Calm
        } else {
            DecorationActivity::Active
        };
        crate::worktree::decoration::tick_decoration(
            &mut self.decoration_states,
            self.ticks.decoration(),
            width,
            height,
            activity,
            mode,
        );
        true
    }

    /// 現在のセッションと日次合計の両方に統計イベントを記録する。
    pub(crate) fn record_stat(&self, field: &str) {
        if let Some(store) = &self.review_store {
            let _ = store.increment_daily_stat(field);
            if let Some(ref sid) = self.stats.session_id {
                let _ = store.increment_session_stat(sid, field);
            }
        }
    }

    /// 現在選択中のworktreeの (worktree_name, working_dir) を返す。
    pub(crate) fn selected_worktree_info(&self) -> (String, std::path::PathBuf) {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| (w.branch.clone(), w.path.clone()))
            .unwrap_or_else(|| ("default".to_string(), self.repo.path.clone()))
    }
}

/// リストの順序は安定せず、追加・削除で以降のインデックスがずれる。古い添字で選ぶと
/// 選択が黙って別ブランチを指すので、まずブランチ名で貼り直し、無い場合だけ丸める。
fn reselect_worktree_index(
    worktrees: &[git_engine::WorktreeInfo],
    prev_branch: &str,
    old_index: usize,
) -> Option<usize> {
    if worktrees.is_empty() {
        return None;
    }
    if !prev_branch.is_empty()
        && let Some(idx) = worktrees.iter().position(|w| w.branch == prev_branch)
    {
        return Some(idx);
    }
    Some(old_index.min(worktrees.len() - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(branch: &str) -> git_engine::WorktreeInfo {
        git_engine::WorktreeInfo {
            path: std::path::PathBuf::from(format!("/tmp/{branch}")),
            branch: branch.to_string(),
            is_main: branch == "main",
            added: 0,
            modified: 0,
            deleted: 0,
            staged: 0,
            is_clean: true,
            ahead: None,
            behind: None,
            head_oid: None,
            head_time: None,
        }
    }

    #[test]
    fn reselect_pins_to_branch_when_order_shifts() {
        // 選択は "feat-b"（インデックス2）を指している。より前に新しいworktreeが
        // 挿入されるとインデックスがずれる。選択は "feat-b" に追従しなければ
        // ならず、インデックス2（今は別のブランチを保持している）に留まっては
        // いけない。
        let after = [wt("main"), wt("feat-a"), wt("feat-aa"), wt("feat-b")];
        assert_eq!(reselect_worktree_index(&after, "feat-b", 2), Some(3));
    }

    #[test]
    fn reselect_falls_back_when_branch_removed() {
        // "feat-a"（インデックス1）が削除され、"main" だけが残っている。古い
        // インデックス1は範囲外なので、最後の有効なインデックス（main）に
        // クランプしなければならない。
        let after = [wt("main")];
        assert_eq!(reselect_worktree_index(&after, "feat-a", 1), Some(0));
    }

    #[test]
    fn reselect_keeps_index_when_branch_unchanged() {
        let after = [wt("main"), wt("feat-a")];
        assert_eq!(reselect_worktree_index(&after, "feat-a", 1), Some(1));
    }

    #[test]
    fn reselect_returns_none_for_empty_list() {
        assert_eq!(reselect_worktree_index(&[], "main", 0), None);
    }

    #[test]
    fn reselect_clamps_when_prev_branch_empty() {
        // 以前選択していたブランチが無い場合（例: 初回読み込み時）: インデックスを
        // 範囲内に保つだけ。
        let after = [wt("main"), wt("feat-a")];
        assert_eq!(reselect_worktree_index(&after, "", 5), Some(1));
    }
}
