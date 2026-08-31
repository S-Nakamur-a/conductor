//! [App] のワークツリー切り替えの中核。
//!
//! ワークツリーの選択（インデックスまたはパスによる）、on_worktree_changed による
//! 一連のリフレッシュ処理（ビュー・セッションの後始末に加え、バックグラウンドの
//! ファイルツリー・diff・ブランチ詳細の処理をディスパッチする）、それらの
//! バックグラウンド結果のポーリング、そして他の worktree_* サブモジュールと共有する
//! 小さなヘルパー（PR URL の取得、gh の可用性チェック、worktree 操作用チャンネル）
//! を扱う。

pub mod bar;
pub mod decoration;
pub mod input;
pub mod mouse;
pub mod ops;
pub mod render;
pub mod state;
mod worktree_branches;
mod worktree_commands;
mod worktree_crud;
mod worktree_grab;
mod worktree_pr;
mod worktree_smart;

use std::sync::mpsc;

use crate::app::*;
use crate::diff_state::DiffState;
use crate::explorer::Explorer;
use crate::git_engine;
use crate::git_engine::status_map::GitStatusMap;
use crate::viewer::ViewerState;

impl App {
    /// file watcher が監視すべきパス: 通常は各 worktree のパス。worktree が
    /// 1 つもない場合 (例: 素の非 git ディレクトリ) はリポジトリのパス自身にする。
    /// これにより Explorer はそこでのファイル変更でも自動更新され続ける。
    pub fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        if self.worktrees.is_empty() {
            vec![self.repo.path.clone()]
        } else {
            self.worktrees.iter().map(|w| w.path.clone()).collect()
        }
    }

    /// [worktree_grab] と [worktree_pr] で共有する。
    fn select_worktree_by_path(&mut self, path: &std::path::Path) {
        if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
            self.worktrees.select(idx);
            self.on_worktree_changed();
        }
    }

    /// 選択中のワークツリーが変わったときに呼ばれる — viewer・diff・セッションを更新する。
    ///
    /// 重い処理（ファイルツリーの走査、diff の計算、ブランチ詳細の取得）はバックグラウンド
    /// スレッドにディスパッチし、UI の応答性を保つ。結果は poll_worktree_switch_ops()
    /// で反映される。
    /// 選択を次のワークツリーへ切り替える（末尾で先頭に戻る）。ストリップのクリックと
    /// 同じ効果を持つ（on_worktree_changed 経由でビューとアクティブセッションを更新し、
    /// ストリップも追従する）が、ターミナルへフォーカスを移さず現在のパネルフォーカスを
    /// 維持する点が異なる。ワークツリーが1つ以下なら何もしない。
    pub fn select_next_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.worktrees
            .select((self.worktrees.selected_index() + 1) % n);
        self.on_worktree_changed();
    }

    /// 選択を前のワークツリーへ切り替える (先頭で末尾に戻る)。[Self::select_next_worktree] を参照。
    pub fn select_prev_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.worktrees
            .select((self.worktrees.selected_index() + n - 1) % n);
        self.on_worktree_changed();
    }

    pub fn on_worktree_changed(&mut self) {
        // reflow トランスクリプトは前のワークツリーのセッションに属するので、新しいセッション
        // 状態を読み込む前にリセットする。
        if self.reflow.active {
            self.close_reflow();
        }

        // 埋め込みエディタは開いたワークツリーに属しているので、離れる前に閉じる。誤ったツリーを
        // 編集し続けたまま取り残される。
        self.discard_editor_on_worktree_change();

        // 次の描画で新たに選択したチップをバーに表示する。ユーザ操作による選択変更のときだけ
        // 立ててよい — ストリップを自由にスクロールして覗いている最中にバックグラウンドの
        // イベントが選択を動かすと、バーが強制的に引き戻される。
        self.wtbar.reveal_selected = true;

        if let Some(outgoing) = self.view_restore.current_branch.clone() {
            self.save_view_for(&outgoing);
        }

        self.explorer = Explorer::default();
        self.viewer = ViewerState::default();

        // 今見ているツリーに対してシンボルインデックスを再構築する。ワークツリーはリポジトリ
        // ルートの兄弟なので、別のワークツリー上で構築した索引はここを見られない。しないと
        // ファイルは正しくても行番号が別ブランチのものになる。
        //
        // selected_worktree への代入ではなくこのメソッドにぶら下げている。代入の中には切り替えと
        // 言えないもの (一時的な退避、ハイライト移動) があり、2 箇所は 3 秒ポーリングとホイールの
        // 全ティックで走るので、そこで再構築すると積み上がる。
        self.start_symbol_index_build();
        // 走っている生成は前のツリーを索引している。止めないと、その結果が
        // 新しいツリーの索引として置かれる。
        let repo_root = self.repo.path.clone();
        self.code_nav.semantic.abort_regeneration(&repo_root);
        self.start_semantic_index_load();

        // ファイル一覧は diff が届くまで意図的に残す (差し替えるとちらつく)。エラーは残さない —
        // 離れた側の失敗を、これから入るワークツリーのものと誤認させる。
        self.diff_state.error = None;

        // ファイルツリーが届き次第、保存済みのファイル・スクロール位置を再度開くために持たせる。
        let new_branch = self.selected_worktree_branch();
        self.view_restore.pending = None;
        self.view_restore.current_branch = if new_branch.is_empty() {
            None
        } else {
            Some(new_branch.clone())
        };
        if let Some(store) = &self.review_store {
            if !new_branch.is_empty() {
                let _ = store.set_selected_worktree(&new_branch);
            }
            if let Ok(Some((Some(file), line))) = store.get_view_state(&new_branch) {
                self.view_restore.pending = Some(crate::types::PendingViewRestore {
                    file,
                    scroll: line.max(0) as usize,
                });
            }
        }

        if let Some(wt) = self.worktrees.selected() {
            self.new_worktree_paths.remove(&wt.path);
        }

        // レビューは高速（SQLite）なので同期のままにする。
        self.refresh_reviews();

        // 次のポーリングサイクルで冗長なリフレッシュが起きないよう、基準値をスナップショットしておく。
        if let Some(wt) = self.worktrees.selected() {
            self.change_watch.head_oid = self.change_watch.heads.get(&wt.branch).cloned();
            self.change_watch.status = Some((wt.added, wt.modified, wt.deleted, wt.staged));
        }

        let wt_name = self.selected_worktree_branch();
        for focus in [Focus::TerminalClaude, Focus::TerminalShell] {
            let Some(kind) = self.terminal.pane(focus).map(|p| p.kind) else {
                continue;
            };
            let first = self
                .current_worktree_sessions(kind)
                .first()
                .map(|(idx, _)| *idx);
            if let Some(pane) = self.terminal.pane_mut(focus) {
                pane.active_session = first;
                pane.scroll = 0;
                pane.cache = Default::default();
            }
            if let Some(idx) = first {
                self.terminal.pty_manager.activate_session(idx);
            }
        }

        if let Some(wt) = self.worktrees.selected() {
            let wt_path = wt.path.clone();
            let wt_branch = wt.branch.clone();

            {
                let path = wt_path.clone();
                self.bg.file_tree.start(move |tx| {
                    // ツリー走査と同時に計算し、git status 取得だけの別の停止が入るのを避ける。空のマップだと
                    // UI はすべてが追跡・コミット済みだと主張するので、失敗を黙って見逃さずログに残す。
                    let git_status = GitStatusMap::load(&path).unwrap_or_else(|e| {
                        log::warn!(
                            "git status unavailable for {} during worktree switch — tree and Changed files will render as if everything is tracked and committed: {e}",
                            path.display()
                        );
                        GitStatusMap::default()
                    });
                    let mut entries = Vec::new();
                    Explorer::walk_dir(&path, &path, 0, &mut entries, &git_status);
                    let _ = tx.send((path, entries, git_status));
                });
            }

            {
                let path = wt_path.clone();
                // refresh_diff と同じベースを使う。別のベースだと切り替え直後に目の前でファイル一覧が
                // 変わる。diff_base_for がこの判断を行う唯一の場所。
                let base_branch = self.diff_base_for(&wt_branch);
                let word_diff = self.config.diff.word_diff;
                let tab_width = self.config.viewer.tab_width;
                self.bg.diff.start(move |tx| {
                    let _ = tx.send(compute_bg_diff(&path, &base_branch, word_diff, tab_width));
                });
            }

            self.start_bg_branch_details();
        }

        self.set_status(
            format!("Switched to worktree: {wt_name}"),
            StatusLevel::Success,
        );
    }

    /// バックグラウンドでのブランチ詳細計算を起動する。
    fn start_bg_branch_details(&mut self) {
        let Some(wt) = self.worktrees.selected() else {
            self.branch_details = Default::default();
            return;
        };
        let branch = wt.branch.clone();
        let is_main = wt.is_main;
        let repo_path = self.repo.path.clone();
        let main_branch = self.config.general.main_branch.clone();
        let worktree_branches: Vec<String> = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main && w.branch != branch)
            .map(|w| w.branch.clone())
            .collect();

        // スレッドを起動する前に、DB にキャッシュされた親・子ブランチがないか確認する。
        let db_initial_branch = if !is_main {
            self.review_store
                .as_ref()
                .and_then(|store| store.get_worktree_base_branch(&branch).ok().flatten())
        } else {
            None
        };

        let active_branches: std::collections::HashSet<String> =
            self.worktrees.iter().map(|w| w.branch.clone()).collect();
        let db_children: Vec<String> = self
            .review_store
            .as_ref()
            .and_then(|store| store.get_worktree_children(&branch).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| active_branches.contains(c))
            .collect();

        // branch_details をリセットし、PR の取得を開始する（すでに非同期）。
        self.branch_details = Default::default();
        if !is_main && self.gh_available {
            self.branch_details.pr_loading = true;
            self.start_pr_url_lookup(&branch);
        }

        self.bg.branch_details.start(move |tx| {
            let mut details = git_engine::BranchDetails::default();

            if !is_main {
                details.initial_branch = db_initial_branch.or_else(|| {
                    git_engine::GitEngine::open(&repo_path)
                        .ok()
                        .and_then(|engine| {
                            engine.detect_parent_branch(&branch, &main_branch, &worktree_branches)
                        })
                });
            }

            if !db_children.is_empty() {
                details.derived_branches = db_children;
            } else if let Ok(engine) = git_engine::GitEngine::open(&repo_path)
                && let Ok(derived) =
                    engine.find_derived_branches(&branch, &main_branch, &worktree_branches)
            {
                details.derived_branches = derived;
            }

            let _ = tx.send(details);
        });
    }

    /// バックグラウンドで進むワークツリー切り替え処理（ファイルツリー、diff、ブランチ詳細）をポーリングする。
    pub fn poll_worktree_switch_ops(&mut self) {
        if let Some((root, entries, git_status)) = self.bg.file_tree.poll() {
            // 3 つまとめて差し替える。根だけ先に新しくなると、古いエントリを指しているクリックが
            // 別ブランチの同名ファイルを黙って開く。
            let root_changed = self.explorer.replace_tree(root, entries, git_status);
            // 相対パスの指す先が変わるので、新しい根に無いファイルのタブは閉じる。同じ根への再走査では
            // 触らない — 一時的に消えたファイルのタブまで閉じてしまう。
            if root_changed {
                self.viewer
                    .prune_tabs_to_root(self.explorer.root(), self.config.viewer.tab_width);
            }
            // ファイルツリーが揃ったので、以前見ていたファイルとスクロール位置を復元する (一度だけ)。
            self.consume_pending_view_restore();
            self.rehighlight_viewer();
        }

        if let Some(result) = self.bg.diff.poll() {
            apply_bg_diff_result(&mut self.diff_state, result);
        }

        if let Some(details) = self.bg.branch_details.poll() {
            // すでに実行中の PR 取得から pr_url と pr_loading を保持する。
            let pr_url = self.branch_details.pr_url.take();
            let pr_loading = self.branch_details.pr_loading;
            self.branch_details = details;
            self.branch_details.pr_url = pr_url;
            self.branch_details.pr_loading = pr_loading;
        }
    }

    /// このシステムで gh CLI が利用可能かどうかを確認する。
    ///
    /// pub(crate) — [crate::app::lifecycle] の起動処理から呼ばれる。
    pub(crate) fn check_gh_available() -> bool {
        std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// 無ければ遅延生成する。[worktree_crud] と [worktree_smart] で共有する。
    fn worktree_op_sender(&mut self) -> mpsc::Sender<WorktreeOpResult> {
        if self.worktree_mgr.bg_worktree_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.worktree_mgr.bg_worktree_tx = Some(tx);
            self.worktree_mgr.bg_worktree_rx = Some(rx);
        }
        self.worktree_mgr.bg_worktree_tx.as_ref().unwrap().clone()
    }

    /// gh pr view 経由で PR の URL を取得するバックグラウンドスレッドを起動する。
    fn start_pr_url_lookup(&mut self, branch: &str) {
        let branch = branch.to_string();
        let repo_path = self.repo.path.clone();

        self.bg.pr_url.start(move |tx| {
            let result = std::process::Command::new("gh")
                .args([
                    "pr", "view", "--head", &branch, "--json", "url", "-q", ".url",
                ])
                .current_dir(&repo_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if url.is_empty() { None } else { Some(url) }
                    } else {
                        None
                    }
                });
            let _ = tx.send(result);
        });
    }

    /// バックグラウンドで進む PR URL の取得結果をポーリングする。
    pub fn poll_pr_url(&mut self) {
        if let Some(result) = self.bg.pr_url.poll() {
            self.branch_details.pr_url = result;
            self.branch_details.pr_loading = false;
        }
    }
}

/// ワーカーのクロージャから切り出してあるのは直接テストするため。同期版は
/// [DiffState::load_diff]。
fn compute_bg_diff(
    path: &std::path::Path,
    base_branch: &str,
    word_diff: bool,
    tab_width: usize,
) -> BgDiffResult {
    match DiffState::compute_changed_files(path, base_branch, word_diff, tab_width) {
        Ok((files, base_error)) => BgDiffResult {
            files,
            error: base_error,
        },
        Err(e) => BgDiffResult {
            files: Vec::new(),
            error: Some(format!("{e:#}")),
        },
    }
}

/// ファイル一覧は error があっても無条件に反映する。ベース ref を解決できない
/// ときは HEAD 基準の一覧が入っており、捨てるとクリーンなツリーと区別できない。
fn apply_bg_diff_result(diff_state: &mut DiffState, result: BgDiffResult) {
    diff_state.files = result.files;
    diff_state.error = result.error;
    diff_state.rebuild_display_list();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::{DiffViewMode, FileDiff};

    fn file(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            added_lines: 1,
            deleted_lines: 0,
            hunks: Vec::new(),
        }
    }

    /// 解決できないベース ref を指定しても手元の変更は消えないこと (17 個の変更ファイルが
    /// (0) と表示される不具合の回帰防止)。
    #[test]
    fn エラー付きの結果でもファイル一覧は残す() {
        let mut ds = DiffState::new("origin/main", DiffViewMode::Unified);
        apply_bg_diff_result(
            &mut ds,
            BgDiffResult {
                files: vec![file("CLAUDE.md"), file("src/config.rs")],
                error: Some("base ref 'origin/main' not found".to_string()),
            },
        );

        assert_eq!(
            ds.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["CLAUDE.md", "src/config.rs"],
        );
        assert!(ds.error.is_some(), "the failure must stay visible");

        // display list を通してすべての File エントリを解決する。diff_list.rs は file_index で
        // files を参照するので、files を差し替えたあとに display list を再構築し忘れると次の
        // 描画で out-of-bounds パニックになる。「常に再構築する」という不変条件の検査。
        let listed: Vec<&str> = (0..ds.display_list.len())
            .filter_map(|idx| ds.resolve_file(idx))
            .map(|f| f.path.as_str())
            .collect();
        // 入力順ではなくディレクトリでグループ化した順序: ディレクトリノード配下の
        // ファイルはトップレベルのファイルより前に来る。
        assert_eq!(listed, vec!["src/config.rs", "CLAUDE.md"]);
    }

    /// 以下のテストはどれも手組みの構造体ではなく実際のリポジトリを要る。
    /// tempdir は呼び出し側が生存させ続けること。
    fn repo_with_uncommitted_change() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let blob = repo.blob(b"a").unwrap();
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert("a.txt", blob, 0o100644).unwrap();
        let tree = repo.find_tree(tb.write().unwrap()).unwrap();
        let oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let commit = repo.find_commit(oid).unwrap();
        repo.branch("feature", &commit, true).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        std::fs::write(dir.path().join("dirty.txt"), b"uncommitted").unwrap();
        dir
    }

    /// ベースを解決できなくてもワーカーが HEAD 基準の diff を返し続けること
    /// (apply_bg_diff_result は「反映」側しか証明しない)。
    #[test]
    fn ベースが解決できなくても背景のdiffはファイルを返す() {
        let dir = repo_with_uncommitted_change();

        let result = compute_bg_diff(dir.path(), "no-such-base", false, 4);

        let err = result
            .error
            .as_deref()
            .expect("base failure must be recorded");
        assert!(err.contains("no-such-base"), "error was: {err}");
        assert_eq!(
            result
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["dirty.txt"],
        );
    }

    /// 解決可能なベースならエラーは残らず、パネルにバナーは表示されない。
    #[test]
    fn 解決できるベースならエラーは出ない() {
        let dir = repo_with_uncommitted_change();

        let result = compute_bg_diff(dir.path(), "main", false, 4);

        assert_eq!(result.error, None);
        assert_eq!(
            result
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["dirty.txt"],
        );
    }

    /// あとから来た成功結果は、古いエラーをクリアしなければならない。そうしないと
    /// パネルにエラーマーカーが残り続けてしまう。
    #[test]
    fn エラー無しの結果は古いエラーを消す() {
        let mut ds = DiffState::new("main", DiffViewMode::Unified);
        ds.error = Some("previous failure".to_string());
        apply_bg_diff_result(
            &mut ds,
            BgDiffResult {
                files: vec![file("src/main.rs")],
                error: None,
            },
        );

        assert!(ds.error.is_none());
        assert_eq!(ds.files.len(), 1);
    }
}
