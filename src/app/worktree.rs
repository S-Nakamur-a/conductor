//! [App] のワークツリー切り替えの中核。
//!
//! ワークツリーの選択（インデックスまたはパスによる）、on_worktree_changed による
//! 一連のリフレッシュ処理（ビュー・セッションの後始末に加え、バックグラウンドの
//! ファイルツリー・diff・ブランチ詳細の処理をディスパッチする）、それらの
//! バックグラウンド結果のポーリング、そして他の worktree_* サブモジュールと共有する
//! 小さなヘルパー（PR URL の取得、gh の可用性チェック、worktree 操作用チャンネル）
//! を扱う。

use std::sync::mpsc;

use crate::git_engine::status_map::GitStatusMap;

use super::*;

impl App {
    // ワークツリー作成・削除のヘルパー

    /// パスを指定してワークツリーを選択し、UI の更新をトリガーする。
    ///
    /// pub(super) — [super::worktree_grab] と [super::worktree_pr] で共有する。
    pub(super) fn select_worktree_by_path(&mut self, path: &std::path::Path) {
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
        self.worktrees.select((self.worktrees.selected_index() + 1) % n);
        self.on_worktree_changed();
    }

    /// 選択を前のワークツリーへ切り替える（先頭で末尾に戻る）。
    /// [Self::select_next_worktree] を参照。
    pub fn select_prev_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.worktrees.select((self.worktrees.selected_index() + n - 1) % n);
        self.on_worktree_changed();
    }

    pub fn on_worktree_changed(&mut self) {
        // reflow トランスクリプトは前のワークツリーのセッションに属するものなので、
        // ワークツリーを切り替えるときは新しいセッション状態を読み込む前にリセットする
        // 必要がある。
        if self.reflow.active {
            self.close_reflow();
        }

        // 埋め込みエディタはそれを開いたワークツリーに属している。そのワークツリーを
        // 離れると誤ったツリーを編集し続けたまま取り残されるので、先に閉じる。
        // 以降のビュー再読み込みが新しいワークツリーをカバーする。
        self.discard_editor_on_worktree_change();

        // 次の描画で、新たに選択したワークツリーのチップをバーに表示する（幅に依存した
        // パンニングはそこで行う。エリアのサイズがそこで分かるため）。これはユーザ操作
        // による選択変更のときだけ安全に立ててよいフラグである。ユーザがストリップを
        // 自由にスクロールして他を覗いている最中に、バックグラウンドのイベントが選択を
        // 動かした場合にこれを立てると、バーが強制的に引き戻されてしまう。
        self.wtbar.reveal_selected = true;

        // 消去する前に、離れるワークツリーのビューを保存しておく。
        if let Some(outgoing) = self.view_restore.current_branch.clone() {
            self.save_view_for(&outgoing);
        }

        self.viewer_state = ViewerState::default();

        // 今ユーザが見ているツリーに対してシンボルインデックスを再構築する。ワークツリーは
        // リポジトリルートの兄弟ディレクトリなので、あるワークツリー上で構築したインデックス
        // は他のワークツリーを見ることができない。これをしないとナビゲーションは常に前の
        // ワークツリーを基準に答え続け、ファイル自体は正しくても行番号が別ブランチのものに
        // なってしまう。ブランチの乖離が大きいほどそのズレも大きくなり、ちょうど diff を
        // 読む価値が最も高い箇所でエラーが起きることになる。
        //
        // あえて selected_worktree への代入ではなく、このメソッドにぶら下げている。
        // selected_worktree への代入の中にはワークツリーの切り替えとは言えないものが
        // いくつかある（セッション起動中の一時的な退避、削除プロンプトを開くためのハイライト
        // 移動）し、さらに2箇所は3秒ごとのポーリングとマウスホイールのすべてのティックで
        // 走るため、そこで再構築すると積み上がってしまう。
        self.start_symbol_index_build();
        // 走っている生成は前のツリーを索引している。止めないと、その結果が
        // 新しいツリーの索引として置かれる。
        self.code_nav.semantic.abort_regeneration();
        self.start_semantic_index_load();

        // ファイル一覧はバックグラウンドの diff が届くまで意図的に残す（空のペインに
        // 差し替えるとちらつくため）。しかしエラーは残してはいけない。それはついさっき
        // 離れたワークツリーのものであり、赤いバナーを出したままにすると離れた側の失敗を
        // これから入るワークツリーのものと誤認させてしまう。
        self.diff_state.error = None;

        // 現在ロード中のワークツリーを記録し、保存済みのファイル・スクロール位置を
        // 種として持たせておく。ファイルツリーが届き次第、それを再度開くために使う。
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
                self.view_restore.pending = Some(crate::app::PendingViewRestore {
                    file,
                    scroll: line.max(0) as usize,
                });
            }
        }

        // 今選択したワークツリーの「new」バッジをクリアする。
        if let Some(wt) = self.worktrees.selected() {
            self.new_worktree_paths.remove(&wt.path);
        }

        // レビューは高速（SQLite）なので同期のままにする。
        self.refresh_reviews();

        // 次のポーリングサイクルで冗長なリフレッシュが起きないよう、基準値をスナップショットしておく。
        if let Some(wt) = self.worktrees.selected() {
            self.last_poll_head_oid = self.worktree_heads.get(&wt.branch).cloned();
            self.last_poll_status = Some((wt.added, wt.modified, wt.deleted, wt.staged));
        }

        // アクティブなセッションを新しいワークツリーに合わせて更新する。
        let wt_name = self.selected_worktree_branch();
        let claude_sessions = self.current_worktree_claude_sessions();
        self.terminal.active_claude_session = claude_sessions.first().map(|(idx, _)| *idx);
        let shell_sessions = self.current_worktree_shell_sessions();
        self.terminal.active_shell_session = shell_sessions.first().map(|(idx, _)| *idx);

        // PTY セッションを有効化する。
        if let Some(idx) = self.terminal.active_claude_session {
            self.terminal.pty_manager.activate_session(idx);
        }
        if let Some(idx) = self.terminal.active_shell_session {
            self.terminal.pty_manager.activate_session(idx);
        }

        self.terminal.scroll_claude = 0;
        self.terminal.scroll_shell = 0;
        self.terminal.cache_claude = Default::default();
        self.terminal.cache_shell = Default::default();

        // 重い処理をバックグラウンドスレッドへディスパッチする。
        if let Some(wt) = self.worktrees.selected() {
            let wt_path = wt.path.clone();
            let wt_branch = wt.branch.clone();

            // バックグラウンドでのファイルツリー走査。
            {
                let path = wt_path.clone();
                self.bg.file_tree.start(move |tx| {
                    // ツリー走査と同時に（メインスレッドではなく）計算することで、ワークツリー
                    // 切り替えのたびに git status 取得だけの別の停止が追加で入るのを避けている。
                    // ViewerState::load_file_tree の同期パスと同じフォールバック＋ログの方針:
                    // 空のマップだと UI はすべてが追跡・コミット済みだと主張してしまうので、
                    // ここでの失敗を黙って見逃してはいけない。
                    let git_status = GitStatusMap::load(&path).unwrap_or_else(|e| {
                        log::warn!(
                            "git status unavailable for {} during worktree switch — tree and Changed files will render as if everything is tracked and committed: {e}",
                            path.display()
                        );
                        GitStatusMap::default()
                    });
                    let mut entries = Vec::new();
                    ViewerState::walk_dir(&path, &path, 0, &mut entries, &git_status);
                    let _ = tx.send((path, entries, git_status));
                });
            }

            // バックグラウンドでの diff 計算。
            {
                let path = wt_path.clone();
                // refresh_diff と同じベースを使う。ここで別のベースを使うと、切り替えた
                // 直後にユーザの目の前でファイル一覧が変わってしまう。diff_base_for が
                // この判断を行う唯一の場所である。
                let base_branch = self.diff_base_for(&wt_branch);
                let word_diff = self.config.diff.word_diff;
                let tab_width = self.config.viewer.tab_width;
                self.bg.diff.start(move |tx| {
                    let _ = tx.send(compute_bg_diff(&path, &base_branch, word_diff, tab_width));
                });
            }

            // バックグラウンドでのブランチ詳細計算。
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
        // ファイルツリーの結果。
        if let Some((root, entries, git_status)) = self.bg.file_tree.poll() {
            // 3 つまとめて差し替える。根だけ先に新しくなると、まだ古いエントリ
            // を指しているクリックが別ブランチの同名ファイルを黙って開く。
            self.viewer_state
                .replace_tree(root, entries, git_status, self.config.viewer.tab_width);
            // このワークツリーのファイルツリーが揃ったので、以前見ていたファイルと
            // スクロール位置を復元する（一度だけ）。
            self.consume_pending_view_restore();
            self.rehighlight_viewer();
        }

        // diff の結果。
        if let Some(result) = self.bg.diff.poll() {
            apply_bg_diff_result(&mut self.diff_state, result);
        }

        // ブランチ詳細の結果。
        if let Some(details) = self.bg.branch_details.poll() {
            // すでに実行中の PR 取得から pr_url と pr_loading を保持する。
            let pr_url = self.branch_details.pr_url.take();
            let pr_loading = self.branch_details.pr_loading;
            self.branch_details = details;
            self.branch_details.pr_url = pr_url;
            self.branch_details.pr_loading = pr_loading;
        }
    }

    // ブランチ詳細（ワークツリー詳細パネル）

    /// このシステムで gh CLI が利用可能かどうかを確認する。
    pub(super) fn check_gh_available() -> bool {
        std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// ワークツリー操作の結果を送る sender を取得する（なければ遅延生成する）。
    ///
    /// pub(super) — [super::worktree_crud] と [super::worktree_smart] で共有する。
    pub(super) fn worktree_op_sender(&mut self) -> mpsc::Sender<WorktreeOpResult> {
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

/// バックグラウンドのワークツリー切り替えワーカーのために diff を計算する。
///
/// 直接テストできるようワーカーのクロージャから切り出してある。同期版の
/// [DiffState::load_diff] と対応する。
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

/// 完了したバックグラウンド diff を [DiffState] へ反映する。
///
/// DiffState のメソッドではなくフリー関数にしているのは、App 全体を構築せずに
/// 単体テストできるようにするためと、diff_state が app::types::BgDiffResult
/// に依存しないで済むようにするため — 依存すればモジュールの依存関係が逆転して
/// しまう。
///
/// ファイル一覧は error が設定されている場合も無条件に反映する。ベース ref を
/// 解決できないときは HEAD 基準にフォールバックした一覧が入っており、一緒に
/// 捨ててしまうと不正なベース ref とクリーンなツリーが見分けつかなくなる。
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

    /// 報告されたバグ: 解決できないベース ref を指定すると、以前は手元の変更まで
    /// 消えてしまい、17個の変更ファイルが (0) と表示されていた。
    #[test]
    fn bg_diff_result_with_error_keeps_the_files() {
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

        // 単に空でないことを確認するのではなく、display list を通してすべての File
        // エントリを解決する。diff_list.rs はエントリの file_index で files を
        // 参照するため、ファイルのベクタを差し替えたあとに display list を再構築し
        // 忘れていると、次の描画で out-of-bounds パニックが起きる。これは「何かが
        // リストされた」ではなく「常に再構築する」という不変条件を確かめるためのもの。
        let listed: Vec<&str> = (0..ds.display_list.len())
            .filter_map(|idx| ds.resolve_file(idx))
            .map(|f| f.path.as_str())
            .collect();
        // 入力順ではなくディレクトリでグループ化した順序: ディレクトリノード配下の
        // ファイルはトップレベルのファイルより前に来る。
        assert_eq!(listed, vec!["src/config.rs", "CLAUDE.md"]);
    }

    /// main に1コミット、HEAD は feature、ワークツリーに未コミットのファイルを
    /// 持つリポジトリを構築する。tempdir を返す（呼び出し側が生存させ続ける）—
    /// 以下のテストはどれも手組みの構造体ではなく実際のリポジトリを必要とする。
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

    /// 修正のうち bg ワーカー側の半分を検証する。apply_bg_diff_result は
    /// 「反映」側しか証明しないので、こちらはベースを解決できなくてもワーカーが
    /// HEAD 基準の diff を返し続けることを証明する。
    #[test]
    fn compute_bg_diff_keeps_the_files_when_base_is_unresolvable() {
        let dir = repo_with_uncommitted_change();

        let result = compute_bg_diff(dir.path(), "no-such-base", false, 4);

        let err = result.error.as_deref().expect("base failure must be recorded");
        assert!(err.contains("no-such-base"), "error was: {err}");
        assert_eq!(
            result.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["dirty.txt"],
        );
    }

    /// 解決可能なベースならエラーは残らず、パネルにバナーは表示されない。
    #[test]
    fn compute_bg_diff_reports_no_error_for_a_resolvable_base() {
        let dir = repo_with_uncommitted_change();

        let result = compute_bg_diff(dir.path(), "main", false, 4);

        assert_eq!(result.error, None);
        assert_eq!(
            result.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["dirty.txt"],
        );
    }

    /// あとから来た成功結果は、古いエラーをクリアしなければならない。そうしないと
    /// パネルにエラーマーカーが残り続けてしまう。
    #[test]
    fn bg_diff_result_without_error_clears_a_stale_one() {
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
