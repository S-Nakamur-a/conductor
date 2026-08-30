use super::*;
use crate::viewer::ViewerState;

/// [ExplorerState::load_file_tree] は自身では Viewer 側を書き換えない
/// ([TreeReload] を返すだけ)ので、テストでも本番の App 配線層と同じ後始末を
/// ここで行う。
fn reload(explorer: &mut ExplorerState, vs: &mut ViewerState, root: &Path, tab_width: usize) {
    let reload = explorer.load_file_tree(root, vs.content.current_file.as_deref());
    if reload.root_changed {
        vs.prune_tabs_to_root(explorer.root(), tab_width);
    }
    if let Some(rel) = &reload.reopen {
        vs.reload_active_file(explorer.root(), rel, tab_width);
    }
}

/// Explorer は git の状態とは無関係に、純粋にファイルシステムからファイルを
/// 一覧しなければならない。.gitignore で無視されている（つまり git 管理外の）
/// ディレクトリと、その中に入っているファイルも到達可能でなければならない。
#[test]
fn walk_includes_gitignored_directories_and_recurses() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();

    // generated/（と *.log）を git 管理から除外する .gitignore。generated は
    // 重い SKIP_DIRS のいずれでもないよう意図的に選んでいる。隠れる理由があるとすれば
    // gitignore しかない。
    std::fs::write(root.join(".gitignore"), "/generated\n*.log\n").unwrap();
    std::fs::create_dir_all(root.join("generated/sub")).unwrap();
    std::fs::write(root.join("generated/out.txt"), "x").unwrap();
    std::fs::write(root.join("generated/sub/inner.txt"), "x").unwrap();
    std::fs::write(root.join("generated/debug.log"), "x").unwrap();

    // トップレベルの走査で、gitignore されたディレクトリ自体が現れなければならない。
    let mut top = Vec::new();
    // ここには本物の git リポジトリが無い — 空のステータスマップ（全て Tracked と
    // 読める）で構わない。このテストは純粋なファイルシステム走査についてであり、
    // git 状態の分類についてではない（そちらは git_status_map のテストを参照）。
    let git_status = GitStatusMap::default();
    ExplorerState::walk_dir(root, root, 0, &mut top, &git_status);
    assert!(
        top.iter().any(|e| e.name == "generated" && e.is_dir),
        "gitignored directory should still be listed: {:?}",
        top.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // それを展開すると、gitignore されたものを含め、中のファイルが現れなければならない。
    let mut children = Vec::new();
    ExplorerState::walk_dir(root, &root.join("generated"), 1, &mut children, &git_status);
    let names: Vec<&str> = children.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"out.txt"), "files: {names:?}");
    assert!(names.contains(&"sub"), "files: {names:?}");
    assert!(
        names.contains(&"debug.log"),
        "gitignored file should be listed: {names:?}"
    );

    // さらに1階層深く再帰が続く。
    let mut deep = Vec::new();
    ExplorerState::walk_dir(root, &root.join("generated/sub"), 2, &mut deep, &git_status);
    assert!(
        deep.iter().any(|e| e.name == "inner.txt"),
        "deep files: {:?}",
        deep.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
}

/// 重いビルド/依存関係ディレクトリは相変わらずスキップされる — このガードは
/// パフォーマンス上の都合であり、git 管理とは無関係。
#[test]
fn walk_still_skips_heavy_dirs() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();

    let mut top = Vec::new();
    ExplorerState::walk_dir(root, root, 0, &mut top, &GitStatusMap::default());
    assert!(top.iter().any(|e| e.name == "src"));
    assert!(
        !top.iter().any(|e| e.name == "node_modules"),
        "node_modules should be skipped"
    );
}

/// 同名のファイルを持つ 2 つの worktree があるとき、開く先は「今表示している
/// ツリーを歩いた根」で決まる。
///
/// worktree の切り替えはツリーの走査を裏のスレッドに回すので、選択が B に移って
/// からエントリが届くまでの間、選択は B・表示しているエントリは A、という状態が
/// 実在する。根を Viewer が持ち、エントリと一緒に差し替えることでこの隙間を潰す。
/// 呼び出し側が開くたびに「今どの worktree か」を引き直していた頃は、その隙間の
/// クリックが A の相対パスを B の根に繋いで、別ブランチの同名ファイルを黙って
/// 開いていた。
#[test]
fn tree_root_and_entries_switch_together() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("shared.txt"), "FROM_A\n").unwrap();
    std::fs::write(b.path().join("shared.txt"), "FROM_B\n").unwrap();

    let mut explorer = ExplorerState::default();
    let mut vs = ViewerState::default();
    reload(&mut explorer, &mut vs, a.path(), 4);
    vs.open_file(explorer.root(), "shared.txt", 4);
    assert_eq!(vs.content.file_content, vec!["FROM_A"]);
    assert_eq!(explorer.root(), a.path(), "load_file_tree が根を確定させる");

    // 裏の走査が返ってきたのと同じ形で B のツリーを適用する。
    let mut entries = Vec::new();
    ExplorerState::walk_dir(
        b.path(),
        b.path(),
        0,
        &mut entries,
        &GitStatusMap::default(),
    );
    let root_changed =
        explorer.replace_tree(b.path().to_path_buf(), entries, GitStatusMap::default());
    if root_changed {
        vs.prune_tabs_to_root(explorer.root(), 4);
    }

    // 相対パスは同じでも、読むのは差し替え後の根の下のファイル。
    vs.open_file(explorer.root(), "shared.txt", 4);
    assert_eq!(vs.content.file_content, vec!["FROM_B"]);
    assert_eq!(explorer.root(), b.path());
}

/// 定期的な、あるいはファイルウォッチャーによるツリーリフレッシュは、ディスク上の
/// 編集を反映するため以前開いていたファイルを再度開く。その open_file は
/// exit_diff_mode を経由し、全ての viewer モードフラグをクリアしてしまう。
/// SUMMARY 疑似ファイル表示はこの往復を生き延びなければならない — さもなければ、
/// Changed files 一覧で SUMMARY を選んでも数秒で最後に開いたファイルに黙って
/// 戻ってしまう。
#[test]
fn tree_refresh_preserves_summary_view() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();

    let mut explorer = ExplorerState::default();
    let mut vs = ViewerState::default();
    reload(&mut explorer, &mut vs, root, 4);
    vs.open_file(explorer.root(), "a.txt", 4);
    vs.enter_summary_view();
    vs.summary_scroll = 7;

    reload(&mut explorer, &mut vs, root, 4);

    assert!(
        vs.is_summary(),
        "summary view must survive a tree refresh (was kicked back to a.txt)"
    );
    assert_eq!(vs.summary_scroll, 7, "summary scroll must be preserved");
}

/// unified diff 表示についても同様の保証があり、summary の修正がこれを退行
/// させてはならない: リフレッシュしても diff モードはオンのまま維持される。
#[test]
fn tree_refresh_preserves_diff_mode() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::write(root.join("a.txt"), "hello\n").unwrap();

    let mut explorer = ExplorerState::default();
    let mut vs = ViewerState::default();
    reload(&mut explorer, &mut vs, root, 4);
    vs.open_file(explorer.root(), "a.txt", 4);
    vs.diff_view.diff_mode = true;
    vs.diff_view.diff_view_scroll = 3;

    reload(&mut explorer, &mut vs, root, 4);

    assert!(vs.diff_view.diff_mode, "diff mode must survive a refresh");
    assert!(!vs.is_summary(), "diff mode must not turn on summary view");
    assert_eq!(vs.diff_view.diff_view_scroll, 3);
}

/// レンダリング済み markdown 表示についても同じ保証がある。open_file は
/// md_scroll をリセットする — ユーザー自身がファイルを開いたときは正しい挙動だが、
/// リフレッシュ経路では誤り。この経路はウォッチャー発火や3秒ポーリングのたびに
/// 現在のファイルを再度開く。保存/復元が無ければ、長いレンダリング済み README を
/// 読んでいても数秒ごとに先頭へ巻き戻されてしまう。
#[test]
fn tree_refresh_preserves_rendered_markdown_scroll() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::write(root.join("README.md"), "# t\n\nbody\n".repeat(50)).unwrap();

    let mut explorer = ExplorerState::default();
    let mut vs = ViewerState::default();
    reload(&mut explorer, &mut vs, root, 4);
    vs.open_file(explorer.root(), "README.md", 4);
    vs.md_rendered = true;
    vs.md_scroll = 40;
    vs.content.file_scroll = 40;

    reload(&mut explorer, &mut vs, root, 4);

    assert_eq!(vs.content.file_scroll, 40, "raw scroll is preserved");
    assert_eq!(vs.md_scroll, 40, "rendered scroll must be preserved too");
    assert!(vs.md_rendered, "the mode itself must survive a refresh");
}
