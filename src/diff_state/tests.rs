//! diff_state モジュールのテスト。インラインセグメントの強調、大文字小文字だけの
//! リネームのフィルタリング、display list ナビゲーションのエッジケース、
//! ベース ref の解決(リモート追跡 ref・タグ・OID、および解決不能なベースが
//! 変更一覧を消し去らないこと)を検証する。

use similar::{ChangeTag, TextDiff};

// 以下のベース ref 解決テストで共有する git リポジトリ構築ヘルパー

/// 使い捨てのコミット署名。これらのテストでは identity は問題にならない。
fn test_signature() -> git2::Signature<'static> {
    git2::Signature::now("test", "test@test.com").unwrap()
}

/// parent のツリーにあるファイルは引き継ぐ。ref は一切更新しないので、どの ref が
/// 存在するかはテストが完全に制御できる。
fn commit_files(
    repo: &git2::Repository,
    parent: Option<&git2::Commit>,
    files: &[(&str, &[u8])],
) -> git2::Oid {
    let base_tree = parent.map(|p| p.tree().unwrap());
    let mut tb = repo.treebuilder(base_tree.as_ref()).unwrap();
    for (path, content) in files {
        let oid = repo.blob(content).unwrap();
        tb.insert(*path, oid, 0o100644).unwrap();
    }
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let signature = test_signature();
    let parents: Vec<&git2::Commit> = parent.into_iter().collect();
    repo.commit(None, &signature, &signature, "test commit", &tree, &parents)
        .unwrap()
}

/// ローカルブランチ name を oid に作成(または移動)し、HEAD にした上で
/// checkout して workdir がそのコミットのツリーを反映するようにする。
fn checkout_branch(repo: &git2::Repository, name: &str, oid: git2::Oid) {
    let commit = repo.find_commit(oid).unwrap();
    repo.branch(name, &commit, true).unwrap();
    repo.set_head(&format!("refs/heads/{name}")).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
}

/// 実際のリモートを設定せずに、oid を指すリモート追跡 ref refs/remotes/<name>
/// を作る。
fn set_remote_tracking_ref(repo: &git2::Repository, name: &str, oid: git2::Oid) {
    repo.reference(&format!("refs/remotes/{name}"), oid, true, "test")
        .unwrap();
}

/// ベースからの変更ファイル一覧。ベース解決に失敗した理由は [base_error] で見る。
fn changed_files(dir: &std::path::Path, base: &str) -> Vec<super::FileDiff> {
    super::DiffState::compute_changed_files(dir, base, false, 4)
        .unwrap()
        .0
}

/// 変更ファイルのパス一覧。
fn changed_paths(dir: &std::path::Path, base: &str) -> Vec<String> {
    changed_files(dir, base)
        .iter()
        .map(|f| f.path.clone())
        .collect()
}

/// ベースを解決できず HEAD にフォールバックした理由(成功時は None)。
fn base_error(dir: &std::path::Path, base: &str) -> Option<String> {
    super::DiffState::compute_changed_files(dir, base, false, 4)
        .unwrap()
        .1
}

#[test]
fn a_replaced_line_carries_inline_segments() {
    let old = "hello world\n";
    let new = "hello rust\n";
    let diff = TextDiff::from_lines(old, new);

    for op in diff.ops() {
        for change in diff.iter_inline_changes(op) {
            if change.tag() == ChangeTag::Insert {
                let has_emphasis = change.values().iter().any(|(e, _)| *e);
                assert!(has_emphasis, "Insert line should have emphasized segments");
            }
        }
    }
}

/// パスが大文字小文字だけ異なり内容が同一な場合、フィルタで除外されることを検証する。
///
/// ツリーに大文字小文字だけが異なるエントリ(例: Photo.png と photo.png)を
/// 持つ git リポジトリを作る。大文字小文字を区別しないファイルシステムでは
/// これらは同じファイルを指すので、blob の内容が同一なら compute_diff_range
/// はこれらを除外すべきである。
#[test]
fn a_case_only_rename_is_not_shown_as_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // "main" 上に "Photo.png" を持つ最初のコミット
    let blob_oid = repo.blob(b"image data").unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("Photo.png", blob_oid, 0o100644).unwrap();
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    let commit1 = repo
        .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    let commit1 = repo.find_commit(commit1).unwrap();

    // "feature" 上に "photo.png" を持つ2番目のコミット(大文字小文字のみ変更、blob は同一)
    let mut tb2 = repo.treebuilder(None).unwrap();
    tb2.insert("photo.png", blob_oid, 0o100644).unwrap();
    let tree2_oid = tb2.write().unwrap();
    let tree2 = repo.find_tree(tree2_oid).unwrap();

    let commit2 = repo
        .commit(
            Some("refs/heads/feature"),
            &sig,
            &sig,
            "rename case",
            &tree2,
            &[&commit1],
        )
        .unwrap();

    // HEAD を feature に向ける。
    repo.set_head_detached(commit2).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    // compute_diff_range が見つけられるよう、ローカルブランチ ref も作っておく。
    repo.branch("feature", &repo.find_commit(commit2).unwrap(), true)
        .unwrap();
    repo.set_head("refs/heads/feature").unwrap();

    let files = changed_files(dir.path(), "main");

    // 内容が同一の大文字小文字だけのリネームは除外されるべき。
    assert!(
        files.is_empty(),
        "case-only rename with same content should be excluded, got: {:?}",
        files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// 内容変更を伴う大文字小文字リネームはフィルタで除外されないことを検証する。
#[test]
fn a_case_rename_with_edits_is_still_shown() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // "main" 上に "Photo.png" を持つ最初のコミット
    let blob1 = repo.blob(b"image data v1").unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("Photo.png", blob1, 0o100644).unwrap();
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();

    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    let commit1 = repo
        .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    let commit1 = repo.find_commit(commit1).unwrap();

    // 2番目のコミット: 大文字小文字の変更 + 内容変更
    let blob2 = repo.blob(b"image data v2 -- updated").unwrap();
    let mut tb2 = repo.treebuilder(None).unwrap();
    tb2.insert("photo.png", blob2, 0o100644).unwrap();
    let tree2_oid = tb2.write().unwrap();
    let tree2 = repo.find_tree(tree2_oid).unwrap();

    let commit2 = repo
        .commit(
            Some("refs/heads/feature"),
            &sig,
            &sig,
            "rename + edit",
            &tree2,
            &[&commit1],
        )
        .unwrap();

    repo.set_head_detached(commit2).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    repo.branch("feature", &repo.find_commit(commit2).unwrap(), true)
        .unwrap();
    repo.set_head("refs/heads/feature").unwrap();

    let files = changed_files(dir.path(), "main");

    // 実際に内容変更を伴うリネームは表示されるべき。
    assert!(
        !files.is_empty(),
        "case rename with content change should NOT be filtered out"
    );
}

/// リグレッション: 既に折りたたまれているディレクトリを折りたたむ (または既に展開されて
/// いるものを展開する) 操作は no-op であるべきで panic してはならない。内側の if を match
/// ガードに変えると、これらのケースが unreachable!() アームまで落ちる。
#[test]
fn collapse_already_collapsed_dir_does_not_panic() {
    use super::*;
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.display_list = vec![DiffListEntry::Directory {
        path: "src".to_string(),
        name: "src".to_string(),
        depth: 0,
        collapsed: true,
    }];
    ds.collapse_section(0); // panic してはならない
}

#[test]
fn expand_already_expanded_dir_does_not_panic() {
    use super::*;
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.display_list = vec![DiffListEntry::Directory {
        path: "src".to_string(),
        name: "src".to_string(),
        depth: 0,
        collapsed: false,
    }];
    ds.expand_section(0); // panic してはならない
}

/// display_index_for_path は resolve_file の逆であり、ファイルがリストの
/// インデックスではなくパス(節が指す位置へのジャンプなど)で開かれた
/// 際に diff リストのカーソルを再同期するために使う。
#[test]
fn display_index_for_path_finds_files() {
    use super::*;
    let file = |path: &str| FileDiff {
        path: path.to_string(),
        added_lines: 0,
        deleted_lines: 0,
        hunks: Vec::new(),
    };
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.files = vec![file("src/a.rs"), file("src/b.rs")];
    ds.display_list = vec![
        DiffListEntry::File {
            file_index: 0,
            depth: 0,
        },
        DiffListEntry::File {
            file_index: 1,
            depth: 0,
        },
    ];

    assert_eq!(ds.display_index_for_path("src/a.rs"), Some(0));
    assert_eq!(ds.display_index_for_path("src/b.rs"), Some(1));
    assert_eq!(ds.display_index_for_path("src/missing.rs"), None);
}

// 寛容なパス解決(revidere の節 / コメントのアンカー)

/// diff が paths に触れる DiffState を作り、display list を構築する。
fn diff_state_with(paths: &[&str]) -> super::DiffState {
    use super::*;
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.files = paths
        .iter()
        .map(|p| FileDiff {
            path: (*p).to_string(),
            added_lines: 1,
            deleted_lines: 0,
            hunks: Vec::new(),
        })
        .collect();
    ds.rebuild_display_list();
    ds
}

/// この変更全体の存在理由となったバグ: ./src/b.rs(あるいは git diff の b/
/// プレフィックス付き、または連続スラッシュ)として保存されたステップは、diff に
/// 実際に含まれるファイルを指しており、diff 自身の表記に解決されなければならない。
#[test]
fn resolve_changed_path_accepts_alternate_spellings() {
    let ds = diff_state_with(&["src/a.rs", "src/deep/c.rs", "top.txt"]);

    for spelling in [
        "src/a.rs",
        "./src/a.rs",
        "src//a.rs",
        "  src/a.rs  ",
        "src/a.rs/",
    ] {
        assert_eq!(
            ds.resolve_changed_path(spelling).as_deref(),
            Some("src/a.rs"),
            "spelling: {spelling}"
        );
    }
    // git diff の a/ と b/ プレフィックス。生成側がパスに残したもの。
    assert_eq!(
        ds.resolve_changed_path("b/src/a.rs").as_deref(),
        Some("src/a.rs")
    );
    assert_eq!(
        ds.resolve_changed_path("a/top.txt").as_deref(),
        Some("top.txt")
    );
    // リポジトリルートではなくサブディレクトリからの相対パスで書かれたもの。
    assert_eq!(
        ds.resolve_changed_path("deep/c.rs").as_deref(),
        Some("src/deep/c.rs")
    );
}

/// 本当に diff に含まれないファイルは未解決のままでなければならない。
/// 上記の寛容さが「この diff にはない」を別のファイルへのジャンプに
/// 変えてしまってはならない。
#[test]
fn resolve_changed_path_refuses_files_outside_the_diff() {
    let ds = diff_state_with(&["src/a.rs", "src/deep/c.rs"]);
    assert_eq!(ds.resolve_changed_path("src/untouched.rs"), None);
    assert_eq!(ds.resolve_changed_path(""), None);
    assert_eq!(ds.resolve_changed_path("./"), None);
}

/// あいまいなサフィックスを推測してはならない。同じ末尾を持つ2つのファイルが
/// あれば、どちらが意図されたか分からず、どちらかを選ぶとレビュアーを黙って
/// 誤ったファイルに着地させてしまう。
#[test]
fn resolve_changed_path_refuses_an_ambiguous_suffix() {
    let ds = diff_state_with(&["src/app/mod.rs", "src/ui/mod.rs"]);
    assert_eq!(ds.resolve_changed_path("mod.rs"), None);
    // 曖昧さを解消するのに十分なコンテキストがあれば問題なく解決する。
    assert_eq!(
        ds.resolve_changed_path("ui/mod.rs").as_deref(),
        Some("src/ui/mod.rs")
    );
}

/// リポジトリに実在するトップレベルの b/ は、b/ を diff プレフィックスとして
/// 読む解釈より優先される。完全一致が先に試されるからである。
#[test]
fn resolve_changed_path_prefers_an_exact_match_over_prefix_stripping() {
    let ds = diff_state_with(&["b/src/a.rs", "src/a.rs"]);
    assert_eq!(
        ds.resolve_changed_path("b/src/a.rs").as_deref(),
        Some("b/src/a.rs")
    );
    assert_eq!(
        ds.resolve_changed_path("src/a.rs").as_deref(),
        Some("src/a.rs")
    );
}

/// レビュアーが折りたたんだディレクトリの中にあるファイルには表示行が存在しない。
/// そこへジャンプする際は、途中まで展開すべきであり「この diff にはない」と
/// 読み違えてはならない。
#[test]
fn reveal_path_expands_collapsed_ancestors() {
    let mut ds = diff_state_with(&["src/deep/nested/c.rs"]);
    assert!(ds.display_index_for_path("src/deep/nested/c.rs").is_some());

    ds.collapsed_dirs.insert("src".to_string());
    ds.rebuild_display_list();
    assert_eq!(
        ds.display_index_for_path("src/deep/nested/c.rs"),
        None,
        "precondition: a collapsed ancestor hides the file's row"
    );

    let idx = ds
        .reveal_path("src/deep/nested/c.rs")
        .expect("row after reveal");
    assert_eq!(
        ds.resolve_file(idx).map(|f| f.path.as_str()),
        Some("src/deep/nested/c.rs")
    );
}

// ベース ref の解決

/// ベースが origin/main として保存された worktree(2026-07-29 以降の
/// Conductor の通常のケース)は、ローカルの main ブランチが存在しなくても
/// 解決できなければならない。
#[test]
fn base_ref_resolves_remote_tracking_ref() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    set_remote_tracking_ref(&repo, "origin/main", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    assert!(
        repo.find_branch("main", git2::BranchType::Local).is_err(),
        "test setup invariant: no local 'main' branch should exist"
    );

    assert_eq!(changed_paths(dir.path(), "origin/main"), vec!["b.txt"]);
    assert_eq!(base_error(dir.path(), "origin/main"), None);
}

/// 同じ解決は、メイン worktree だけでなくリンクされた worktree の Repository
/// からも動作しなければならない。リモート追跡 ref は worktree ごとではなく、
/// 共有の commondir に存在するからである。
#[test]
fn base_ref_resolves_from_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    set_remote_tracking_ref(&repo, "origin/main", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    // "feature" と同じコミットを指す新しいブランチで worktree をリンクする
    // ("feature" 自体は既にメイン worktree で checkout 済みなので再利用できない)。
    // まだ存在しない、リポジトリ自身の tempdir 外のパスを対象にする:
    // git worktree add は空でないディレクトリを拒否する。
    let wt_parent = tempfile::tempdir().unwrap();
    let wt_path = wt_parent.path().join("linked-wt");
    let status = std::process::Command::new("git")
        // ユーザのグローバル/システム git 設定(core.hooksPath の
        // post-checkout フックなど)から隔離し、テスト対象と無関係な
        // 理由で失敗しないようにする。
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args([
            "-C",
            dir.path().to_str().unwrap(),
            "worktree",
            "add",
            "-b",
            "wt-branch",
            wt_path.to_str().unwrap(),
            &head_oid.to_string(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "git worktree add failed");

    assert_eq!(changed_paths(&wt_path, "origin/main"), vec!["b.txt"]);
    assert_eq!(base_error(&wt_path, "origin/main"), None);
}

/// 軽量タグ、注釈付きタグ、完全な OID、短縮 OID のいずれも同じベースコミットに
/// 解決しなければならない。
#[test]
fn base_ref_resolves_tag_lightweight_annotated_and_oid() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    let base_commit = repo.find_commit(base_oid).unwrap();
    let base_obj = repo.find_object(base_oid, None).unwrap();

    repo.tag_lightweight("v-lw", &base_obj, false).unwrap();
    let tagger = test_signature();
    repo.tag("v-ann", &base_obj, &tagger, "annotated tag", false)
        .unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    let full_oid = base_oid.to_string();
    let short_oid = &full_oid[..7];

    for base in ["v-lw", "v-ann", full_oid.as_str(), short_oid] {
        assert_eq!(
            base_error(dir.path(), base),
            None,
            "base '{base}' failed to resolve"
        );
        assert_eq!(
            changed_paths(dir.path(), base),
            vec!["b.txt"],
            "base '{base}' produced unexpected diff"
        );
    }
}

/// develop のように、リモート追跡 ref としてのみ存在する(ローカルブランチが
/// ない)設定済みベースは、origin/ フォールバック経由で解決しなければならない。
#[test]
fn base_ref_falls_back_to_origin_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    set_remote_tracking_ref(&repo, "origin/develop", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    let head_oid = commit_files(&repo, Some(&base_commit), &[("b.txt", b"b")]);
    checkout_branch(&repo, "feature", head_oid);

    assert!(
        repo.find_branch("develop", git2::BranchType::Local)
            .is_err(),
        "test setup invariant: no local 'develop' branch should exist"
    );

    assert_eq!(changed_paths(dir.path(), "develop"), vec!["b.txt"]);
    assert_eq!(base_error(dir.path(), "develop"), None);
}

/// 直接にも origin/ 経由にも解決しないベースは、呼び出し側が実際に指定した
/// ベース名を含むエラーを報告しなければならない。
#[test]
fn base_ref_unresolvable_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    let msg = base_error(dir.path(), "nonexistent").expect("expected an error to be set");
    assert!(msg.contains("nonexistent"), "error message was: {msg}");
}

/// 既に origin/<name> として修飾済みのベースは、失敗時に
/// origin/origin/<name> として再試行してはならない。このガードがなければ
/// origin/weird のようなベースは黙って origin/origin/weird に対して
/// 再試行され、その二重化されたパスにたまたま無関係な ref があれば
/// そちらに解決してしまう ─ 呼び出し側が意図していない ref との diff になる。
/// このテストはまさにそのトラップとなる ref を作り、ガードの欠落が
/// エラーメッセージの文言だけでなく予期しない Ok として現れるようにする。
#[test]
fn base_ref_error_does_not_double_the_origin_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);
    // 二重化されたパスにのみ存在する。もし origin/ ガードが欠けていれば、
    // "origin/weird" の解決は "origin/origin/weird" として再試行され、
    // 失敗する代わりにここに着地してしまう。
    set_remote_tracking_ref(&repo, "origin/origin/weird", oid);

    let msg = base_error(dir.path(), "origin/weird").expect("expected an error to be set");
    assert!(msg.contains("origin/weird"), "error message was: {msg}");
    assert!(
        !msg.contains("origin/origin"),
        "error message doubled the origin/ prefix: {msg}"
    );
}

/// revparse_single は git 自身の revspec 解決順序(refs/tags/<name> が
/// refs/heads/<name> より優先される。gitrevisions(7) 参照)に従うので、
/// タグとローカルブランチの両方を指す dup はタグの方に解決される。以前の
/// find_branch(Local) による解決はブランチの方を選んでいたので、これは
/// 意図的な挙動変更である。要点は、Conductor のベース解決が git rev-parse dup
/// の出力と決して食い違わないようにすることにある。ブランチ優先の特別扱いを
/// 加えると、TUI とシェルが同じリポジトリで異なるベースを選ぶことになり、
/// 2つの選択肢のうちより紛らわしい方になってしまう。
#[test]
fn base_ref_prefers_tag_over_branch_like_git_rev_parse() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let root_oid = commit_files(&repo, None, &[("base.txt", b"base")]);
    let root_commit = repo.find_commit(root_oid).unwrap();

    // ローカルブランチ "dup" -> タグのターゲットから分岐したコミット。
    let branch_oid = commit_files(&repo, Some(&root_commit), &[("from_branch.txt", b"a")]);
    repo.branch("dup", &repo.find_commit(branch_oid).unwrap(), false)
        .unwrap();

    // タグ "dup" -> ブランチのものとも分岐した別のコミット。
    let tag_oid = commit_files(&repo, Some(&root_commit), &[("from_tag.txt", b"b")]);
    let tag_commit = repo.find_commit(tag_oid).unwrap();
    let tag_obj = repo.find_object(tag_oid, None).unwrap();
    repo.tag_lightweight("dup", &tag_obj, false).unwrap();

    // HEAD はタグのコミットからのみ派生しているので、2つの解決は異なる
    // merge-base(したがって異なる diff)を生む: ブランチとして解決すると
    // 共有ルートに達し "from_tag.txt" も拾ってしまうが、タグとして解決すると
    // タグのコミット自体を merge-base として扱う。
    let head_oid = commit_files(&repo, Some(&tag_commit), &[("head.txt", b"c")]);
    checkout_branch(&repo, "feature", head_oid);

    assert_eq!(
        changed_paths(dir.path(), "dup"),
        vec!["head.txt"],
        "expected 'dup' to resolve to the tag (only head.txt ahead of it), \
         not the branch (which would also include from_tag.txt)"
    );
}

/// 解決不能なベースは HEAD 基準へのフォールバックで扱う。手元の変更が
/// ベース設定のミスで丸ごと見えなくなってはならない。
#[test]
fn unresolvable_base_falls_back_to_head() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    assert_eq!(changed_paths(dir.path(), "nonexistent"), vec!["c.txt"]);
}

// ベースの解決失敗時に load_diff が手元の変更をクリアしてはならない

/// 解決不能なベースを与えた load_diff は、一覧をクリアするのではなく、
/// 手元の変更とエラーの両方をきちんと表に出さなければならない。
#[test]
fn load_diff_keeps_files_when_base_unresolvable() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let mut ds = DiffState::new("nonexistent", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "nonexistent", false, 4);

    let err = ds.error.as_deref().expect("expected an error to be set");
    assert!(err.contains("nonexistent"), "error message was: {err}");

    let paths: Vec<&str> = ds.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["c.txt"]);
    assert!(
        ds.display_index_for_path("c.txt").is_some(),
        "display_list should still include the file"
    );
}

/// HEAD が設定されたベースとの merge-base に等しい場合(0コミット先行)、
/// エラーは出ず、未コミットの変更だけが表示されるべきである。
#[test]
fn load_diff_head_equals_merge_base_shows_uncommitted_only() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);
    // "release" は HEAD と同じコミットを指すので、
    // merge-base(release, HEAD) == HEAD == release となる。
    let commit = repo.find_commit(oid).unwrap();
    repo.branch("release", &commit, false).unwrap();

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let mut ds = DiffState::new("release", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "release", false, 4);

    assert!(ds.error.is_none());

    let paths: Vec<&str> = ds.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["c.txt"]);
}

/// 元のバグ報告をエンドツーエンドで再現する: ベースが origin/main として
/// 保存され(ローカルの main ブランチはない)、大量の未コミット変更を抱えたまま
/// origin/main から1コミット遅れている worktree。修正前は main が
/// find_branch(Local) 経由での解決に失敗し、その失敗が diff 一覧を消し去って
/// いた ─ 変更済み/未追跡ファイルが実際には全て存在するにもかかわらず
/// "Changed files (0)" と報告されていた。
#[test]
fn load_diff_reproduces_the_silent_zero_files_report() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // C0 -> C1 (HEAD, "feature") -> C2 (origin/main のみ、ローカルブランチなし)。
    let root_oid = commit_files(&repo, None, &[("base.txt", b"base")]);
    let root_commit = repo.find_commit(root_oid).unwrap();

    let head_oid = commit_files(
        &repo,
        Some(&root_commit),
        &[("tracked1.txt", b"orig1"), ("tracked2.txt", b"orig2")],
    );
    checkout_branch(&repo, "feature", head_oid);
    let head_commit = repo.find_commit(head_oid).unwrap();

    let origin_main_oid = commit_files(&repo, Some(&head_commit), &[("future.txt", b"ahead")]);
    set_remote_tracking_ref(&repo, "origin/main", origin_main_oid);

    assert!(
        repo.find_branch("main", git2::BranchType::Local).is_err(),
        "test setup invariant: no local 'main' branch should exist"
    );

    // 変更済みの追跡ファイル + 新規の未追跡ファイル。報告の形に合わせている
    // (元は15件変更+2件未追跡だったが、ここでは2+2で十分)。
    std::fs::write(dir.path().join("tracked1.txt"), b"modified1").unwrap();
    std::fs::write(dir.path().join("tracked2.txt"), b"modified2").unwrap();
    std::fs::write(dir.path().join("untracked1.txt"), b"new1").unwrap();
    std::fs::write(dir.path().join("untracked2.txt"), b"new2").unwrap();

    let mut ds = DiffState::new("origin/main", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "origin/main", false, 4);

    // 修正の効果: ベースが解決したのでエラーなし。
    assert_eq!(ds.error, None);

    let paths: Vec<&str> = ds.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "tracked1.txt",
            "tracked2.txt",
            "untracked1.txt",
            "untracked2.txt"
        ]
    );
    for path in [
        "tracked1.txt",
        "tracked2.txt",
        "untracked1.txt",
        "untracked2.txt",
    ] {
        assert!(
            ds.display_index_for_path(path).is_some(),
            "display_list should include '{path}'"
        );
    }
}

// 計画の NFR チェックリストで挙げられていたがカバレッジがなかったエッジケース

/// ベースと HEAD が共通の履歴を持たない(shallow clone が生む形と同じ、
/// 無関係な2つのルートコミット)場合、repo.merge_base() は失敗する。
/// このときも一覧は HEAD 基準で残り、理由が見える形になっていること。
#[test]
fn load_diff_keeps_files_when_merge_base_is_unrelated() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // 共通の祖先を持たない2つのルートコミット。
    let other_oid = commit_files(&repo, None, &[("other.txt", b"other")]);
    repo.branch("other", &repo.find_commit(other_oid).unwrap(), false)
        .unwrap();

    let head_oid = commit_files(&repo, None, &[("head.txt", b"head")]);
    checkout_branch(&repo, "feature", head_oid);

    std::fs::write(dir.path().join("c.txt"), b"new file").unwrap();

    let mut ds = DiffState::new("other", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "other", false, 4);

    let err = ds.error.as_deref().expect("expected an error to be set");
    // 失敗モードとベースの両方を名指ししなければならない。そうしないと
    // base_ref_unresolvable_reports_error の解決不能 ref エラーと区別が
    // つかず、バナーが「そんな ref はない」と「共通の祖先がない」を
    // 見分けられなくなる。
    assert!(err.contains("merge-base"), "error message was: {err}");
    assert!(err.contains("other"), "error message was: {err}");

    let paths: Vec<&str> = ds.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["c.txt"]);
}

/// コミットが0件の worktree(git init しただけで HEAD が "unborn")は
/// panic してはならず、変更0件と黙って報告するのではなくエラーを表に
/// 出さなければならない。
///
/// ここでの workdir ファイルは技術的には全て未追跡だが、diff はそれでも
/// 比較対象となるツリーを必要とし、それが存在しない。説明のつかない
/// "Changed files (0)" ではなく、理由が error に載る。
#[test]
fn load_diff_on_unborn_head_reports_error_without_panicking() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let _repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("untracked.txt"), b"new").unwrap();

    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "main", false, 4);

    let err = ds.error.as_deref().expect("expected an error to be set");
    assert!(err.contains("HEAD"), "error message was: {err}");
    assert!(ds.files.is_empty());
    assert!(ds.display_list.is_empty());
}

/// ベースとして "HEAD" を指定すると、merge-base(HEAD, HEAD) == HEAD なので
/// コミット済みの変更は何も出ず、作業ツリーの変更だけが残る。エラーにもならない。
/// これは diff エンジンとしては正しい挙動である — "HEAD" はそもそもベースとして
/// 役に立たないというだけの話だ。worktree が "HEAD" をベースとして保存して
/// しまわないようにするのは書き込み側の責務(GitEngine::resolve_base_ref /
/// worktree_crud.rs)であり、この関数が特別扱いすべきことではない。
#[test]
fn base_ref_head_yields_only_the_working_tree_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    assert!(changed_files(dir.path(), "HEAD").is_empty());
    assert_eq!(base_error(dir.path(), "HEAD"), None);
}

/// コミット以外のオブジェクトに解決されるベース(ここでは blob を直接
/// 指す軽量タグ)は、黙ってフォールバックするのではなく理由を報告しな
/// ければならない — 一覧だけを見ている呼び出し側からは両者が同じに見える。
#[test]
fn base_ref_pointing_at_a_blob_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    let blob_oid = repo.blob(b"not a commit").unwrap();
    let blob_obj = repo.find_object(blob_oid, None).unwrap();
    repo.tag_lightweight("blob-tag", &blob_obj, false).unwrap();

    let msg = base_error(dir.path(), "blob-tag").expect("expected an error to be set");
    assert!(msg.contains("blob-tag"), "error message was: {msg}");
}

/// config.general.main_branch = "" は TOML 層では弾かれないため、空文字列の
/// ベースが diff 計算まで届くことがある。これを書く前に git2 で直接確認
/// 済み: revparse_single("") は何かに解決されるのではなく InvalidSpec で
/// エラーになるので、これは既に安全である — このテストはそれを固定する
/// ためのもので、将来の git2 アップグレードでこれが
/// base_ref_head_yields_only_the_working_tree_changes と同じ "HEAD" の罠に
/// 黙って変わってしまわないようにする。
#[test]
fn base_ref_empty_string_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("a.txt", b"a")]);
    checkout_branch(&repo, "main", oid);

    assert!(
        base_error(dir.path(), "").is_some(),
        "empty-string base should not resolve"
    );
}

// 1ファイル = 1エントリ

/// この変更の存在理由: コミット済みと未コミットを別々に計算していた頃は、
/// コミットしたファイルを再編集すると同じファイルが2行に分かれて出ていた。
/// 1本の diff にまとめた今は、行数もベースからの合計になっていること。
#[test]
fn a_file_edited_after_commit_stays_one_entry() {
    use super::*;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let base_oid = commit_files(&repo, None, &[("a.txt", b"base\n")]);
    checkout_branch(&repo, "main", base_oid);
    let base_commit = repo.find_commit(base_oid).unwrap();

    // ブランチ側で1行追加してコミット。
    let head_oid = commit_files(
        &repo,
        Some(&base_commit),
        &[("a.txt", b"base\ncommitted\n")],
    );
    checkout_branch(&repo, "feature", head_oid);

    // コミット済みのファイルをさらに編集する。
    std::fs::write(dir.path().join("a.txt"), b"base\ncommitted\nedited\n").unwrap();

    let files = changed_files(dir.path(), "main");
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["a.txt"],
        "a file changed both in a commit and in the worktree must not be listed twice"
    );
    assert_eq!((files[0].added_lines, files[0].deleted_lines), (2, 0));

    // 表示リストも同じで、行は1つだけ。
    let mut ds = DiffState::new("main", DiffViewMode::Unified);
    ds.load_diff(dir.path(), "main", false, 4);
    let listed: Vec<&str> = (0..ds.display_list.len())
        .filter_map(|idx| ds.resolve_file(idx))
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(listed, vec!["a.txt"]);
}

/// リグレッション: 未ステージ変更の新内容は workdir から読む。読めなかったときに空文字列へ
/// 落とすと旧内容だけが残り、1 行直しただけのファイルが +0 -<全行数> と出て git diff と
/// 食い違う。読めなかったことは削除された証拠ではない。
#[test]
fn unreadable_workdir_file_does_not_fabricate_full_deletion() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let oid = commit_files(&repo, None, &[("a.txt", original.as_bytes())]);
    checkout_branch(&repo, "main", oid);

    // git から見れば +1 -1 でしかない変更。
    let path = dir.path().join("a.txt");
    std::fs::write(&path, original.replace("line 5\n", "line five\n")).unwrap();

    // 読み取りだけを失敗させる。ファイルは存在したままなので git の見え方は変わらない。
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&path, perms).unwrap();

    let files = changed_files(dir.path(), "main");

    // tempdir の後始末が権限で失敗しないよう、判定より先に戻す。
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&path, perms).unwrap();

    if let Some(f) = files.iter().find(|f| f.path == "a.txt") {
        assert!(
            f.deleted_lines < 20,
            "read failure was reported as a full-file deletion: +{} -{}",
            f.added_lines,
            f.deleted_lines
        );
    }
}

/// 上のガードが本物の削除まで隠してしまわないこと。workdir から消えた
/// ファイルは読めなくて当然で、全行削除が正しい表示。
#[test]
fn deleted_workdir_file_still_reports_full_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let oid = commit_files(&repo, None, &[("a.txt", original.as_bytes())]);
    checkout_branch(&repo, "main", oid);

    std::fs::remove_file(dir.path().join("a.txt")).unwrap();

    let files = changed_files(dir.path(), "main");

    let f = files
        .iter()
        .find(|f| f.path == "a.txt")
        .expect("deleted file should still be listed");
    assert_eq!((f.added_lines, f.deleted_lines), (0, 20));
}

/// バイナリファイルは行数を出せないが、一覧から消えてはいけない。
/// libgit2 の Patch::from_diff は「変更なし」でもバイナリでも None を返すので、
/// 区別せずに落とすと、変更したバイナリが Changed files に現れなくなる。
#[test]
fn binary_file_stays_listed_without_line_counts() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let oid = commit_files(&repo, None, &[("logo.png", &[0u8, 1, 2, 0, 3, 4][..])]);
    checkout_branch(&repo, "main", oid);

    std::fs::write(dir.path().join("logo.png"), [0u8, 9, 9, 0, 7, 7, 7]).unwrap();

    let files = changed_files(dir.path(), "main");

    let f = files
        .iter()
        .find(|f| f.path == "logo.png")
        .expect("a changed binary file must stay in the list");
    assert_eq!((f.added_lines, f.deleted_lines), (0, 0));
    assert!(f.hunks.is_empty());
}

#[cfg(target_os = "macos")]
fn fs_ignores_case(dir: &std::path::Path) -> bool {
    let probe = dir.join("CaseProbe");
    std::fs::write(&probe, b"").unwrap();
    let ignores = dir.join("caseprobe").is_file();
    std::fs::remove_file(&probe).unwrap();
    ignores
}

/// リグレッション: ケース違いの2エントリに実ファイルが1つしか無いこの状態を、
/// git 本体は clean と報告する。
// 大文字小文字を区別しないファイルシステムでしか再現しない。cfg で外して
// あるのは、走らない環境では『テストが無い』ことが一覧から見えるようにする
// ため。実行時に return すると、検証していないのに緑になる。
#[cfg(target_os = "macos")]
#[test]
fn a_case_colliding_entry_is_not_reported_as_deleted() {
    let dir = tempfile::tempdir().unwrap();
    if !fs_ignores_case(dir.path()) {
        eprintln!("skipped: 大文字小文字を区別するファイルシステムでは再現しない");
        return;
    }

    let repo = git2::Repository::init(dir.path()).unwrap();
    let blob = repo.blob(b"image data\n").unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("Instagram.png", blob, 0o100644).unwrap();
    tb.insert("instagram.png", blob, 0o100644).unwrap();
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = test_signature();
    repo.commit(
        Some("refs/heads/main"),
        &sig,
        &sig,
        "both cases",
        &tree,
        &[],
    )
    .unwrap();
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    let files = changed_files(dir.path(), "main");

    assert!(
        files.is_empty(),
        "case-colliding index entry should not be reported as a change, got: {:?}",
        files
            .iter()
            .map(|f| (&f.path, f.added_lines, f.deleted_lines))
            .collect::<Vec<_>>()
    );
}
