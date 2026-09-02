use git2::Oid;

use super::*;
use crate::test_support::{TestRepo, Tree};

fn file(path: &str) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        added_lines: 0,
        deleted_lines: 0,
        hunks: Vec::new(),
    }
}

fn diff_state_with(paths: &[&str]) -> DiffState {
    let mut ds = DiffState::new("main");
    ds.files = paths.iter().map(|p| file(p)).collect();
    ds.rebuild_display_list();
    ds
}

/// (変更パス, ベースを解決できなかった理由)。
fn changed(tree: &Tree, base: &str) -> (Vec<String>, Option<String>) {
    let (files, error) = DiffState::compute_changed_files(&tree.path, base, false, 4).unwrap();
    (files.into_iter().map(|f| f.path).collect(), error)
}

fn loaded(tree: &Tree, base: &str) -> DiffState {
    let mut ds = DiffState::new(base);
    ds.load_diff(&tree.path, base, false, 4);
    ds
}

fn listed_paths(ds: &DiffState) -> Vec<&str> {
    ds.files.iter().map(|f| f.path.as_str()).collect()
}

/// a.txt のベースコミットの上に b.txt を足したコミットを feature としてチェックアウトする。
/// ref はそれ以外作らないので、ベースをどう指せるかはテストが決める。
fn base_then_feature(repo: &TestRepo) -> Oid {
    let base = repo.commit_tree(None, &[("a.txt", b"a")]);
    let head = repo.commit_tree(Some(base), &[("b.txt", b"b")]);
    repo.checkout_at("feature", head);
    base
}

/// a.txt を main にコミットしただけの状態。
fn on_main(repo: &TestRepo) -> Oid {
    let oid = repo.commit_tree(None, &[("a.txt", b"a")]);
    repo.checkout_at("main", oid);
    oid
}

#[test]
fn ベースrefはローカルブランチが無くてもリモート追跡refとタグとoidで解決する() {
    type Case = (&'static str, fn(&Tree, Oid), fn(Oid) -> String);
    let cases: [Case; 6] = [
        (
            "リモート追跡 ref",
            |t, oid| {
                t.remote_ref("origin/main", oid);
            },
            |_| "origin/main".to_string(),
        ),
        (
            "裸の名前は origin/ を補う",
            |t, oid| {
                t.remote_ref("origin/develop", oid);
            },
            |_| "develop".to_string(),
        ),
        (
            "軽量タグ",
            |t, oid| {
                t.tag("v-lw", oid);
            },
            |_| "v-lw".to_string(),
        ),
        (
            "注釈タグ",
            |t, oid| {
                t.annotated_tag("v-ann", oid);
            },
            |_| "v-ann".to_string(),
        ),
        ("完全 OID", |_, _| {}, |oid| oid.to_string()),
        (
            "短縮 OID",
            |_, _| {},
            |oid| oid.to_string()[..7].to_string(),
        ),
    ];
    for (label, setup, base) in cases {
        let repo = TestRepo::new();
        let base_oid = base_then_feature(&repo);
        setup(&repo, base_oid);
        let base = base(base_oid);
        assert!(
            repo.repo
                .find_branch(&base, git2::BranchType::Local)
                .is_err(),
            "{label}: ローカルブランチ経由で解決してはならない"
        );

        assert_eq!(
            changed(&repo, &base),
            (vec!["b.txt".to_string()], None),
            "{label}"
        );
    }
}

/// リモート追跡 ref は worktree ごとではなく共有の commondir にある。
#[test]
fn リンクされたworktreeからもリモート追跡refを解決する() {
    let repo = TestRepo::new();
    let base = base_then_feature(&repo);
    repo.remote_ref("origin/main", base);
    let linked = repo.linked_worktree("wt");

    assert_eq!(
        changed(&linked, "origin/main"),
        (vec!["b.txt".to_string()], None)
    );
}

/// git rev-parse と同じ順で解決しないと、TUI とシェルが同じリポジトリで違うベースを選ぶ。
#[test]
fn gitと同じくタグをブランチより優先する() {
    let repo = TestRepo::new();
    let root = repo.commit_tree(None, &[("base.txt", b"base")]);
    let on_branch = repo.commit_tree(Some(root), &[("from_branch.txt", b"a")]);
    repo.branch_at("dup", on_branch);
    let on_tag = repo.commit_tree(Some(root), &[("from_tag.txt", b"b")]);
    repo.tag("dup", on_tag);
    let head = repo.commit_tree(Some(on_tag), &[("head.txt", b"c")]);
    repo.checkout_at("feature", head);

    assert_eq!(
        changed(&repo, "dup"),
        (vec!["head.txt".to_string()], None),
        "ブランチ側に解決すると merge-base が root になり from_tag.txt も混ざる"
    );
}

#[test]
fn 解決できないベースは利用者が書いた綴りで理由を返す() {
    type Case = (
        &'static str,
        fn(&Tree, Oid),
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    );
    let cases: [Case; 5] = [
        ("無い ref", |_, _| {}, "nonexistent", &["nonexistent"], &[]),
        (
            "origin/ 付きは origin/origin/ に落とさない",
            |t, oid| {
                t.remote_ref("origin/origin/weird", oid);
            },
            "origin/weird",
            &["origin/weird"],
            &["origin/origin"],
        ),
        (
            "blob を指すタグ",
            |t, _| {
                let blob = t.repo.blob(b"not a commit").unwrap();
                t.tag("blob-tag", blob);
            },
            "blob-tag",
            &["blob-tag"],
            &[],
        ),
        ("空文字", |_, _| {}, "", &[], &[]),
        (
            "共通の祖先が無い",
            |t, _| {
                let other = t.commit_tree(None, &[("other.txt", b"other")]);
                t.branch_at("other", other);
            },
            "other",
            &["merge-base", "other"],
            &[],
        ),
    ];
    for (label, setup, base, must_contain, must_not_contain) in cases {
        let repo = TestRepo::new();
        let oid = on_main(&repo);
        setup(&repo, oid);

        let (_, error) = changed(&repo, base);

        let error = error.unwrap_or_else(|| panic!("{label}: error should be set"));
        for needle in must_contain {
            assert!(error.contains(needle), "{label}: {error}");
        }
        for needle in must_not_contain {
            assert!(!error.contains(needle), "{label}: {error}");
        }
    }
}

/// ベース設定のミスで手元の変更が丸ごと見えなくなってはならない。
#[test]
fn ベースが解決できなくても手元の変更は一覧に残る() {
    type Case = (&'static str, fn(&Tree), &'static str);
    let cases: [Case; 2] = [
        ("無い ref", |_| {}, "nonexistent"),
        (
            "共通の祖先が無い",
            |t| {
                let other = t.commit_tree(None, &[("other.txt", b"other")]);
                t.branch_at("other", other);
            },
            "other",
        ),
    ];
    for (label, setup, base) in cases {
        let repo = TestRepo::new();
        on_main(&repo);
        setup(&repo);
        repo.file("c.txt", "new file");

        let ds = loaded(&repo, base);

        assert!(
            ds.error.as_deref().is_some_and(|e| e.contains(base)),
            "{label}: {:?}",
            ds.error
        );
        assert_eq!(listed_paths(&ds), ["c.txt"], "{label}");
        assert!(ds.display_index_for_path("c.txt").is_some(), "{label}");
    }
}

/// merge-base(base, HEAD) == HEAD なら、コミット済みの変更は無く作業ツリーの変更だけが出る。
/// "HEAD" をベースとして保存させないのは書き込み側の責務で、ここで特別扱いしない。
#[test]
fn headがmerge_baseと同じなら未コミット分だけ出る() {
    type Case = (&'static str, fn(&Tree, Oid), &'static str);
    let cases: [Case; 2] = [
        (
            "HEAD と同じコミットのブランチ",
            |t, oid| {
                t.branch_at("release", oid);
            },
            "release",
        ),
        ("HEAD そのもの", |_, _| {}, "HEAD"),
    ];
    for (label, setup, base) in cases {
        let repo = TestRepo::new();
        let oid = on_main(&repo);
        setup(&repo, oid);
        repo.file("c.txt", "new file");

        let ds = loaded(&repo, base);

        assert_eq!(ds.error, None, "{label}");
        assert_eq!(listed_paths(&ds), ["c.txt"], "{label}");
    }
}

/// 元のバグ報告の形: ベースは origin/main (ローカルの main は無い)、そこから 1 コミット
/// 遅れた worktree に変更済みと未追跡が混在。ベース解決の失敗が一覧を消して 0 件と出ていた。
#[test]
fn 無音で0件になる不具合を再現する() {
    let repo = TestRepo::new();
    let root = repo.commit_tree(None, &[("base.txt", b"base")]);
    let head = repo.commit_tree(
        Some(root),
        &[("tracked1.txt", b"orig1"), ("tracked2.txt", b"orig2")],
    );
    repo.checkout_at("feature", head);
    let origin_main = repo.commit_tree(Some(head), &[("future.txt", b"ahead")]);
    repo.remote_ref("origin/main", origin_main);
    repo.file("tracked1.txt", "modified1")
        .file("tracked2.txt", "modified2")
        .file("untracked1.txt", "new1")
        .file("untracked2.txt", "new2");

    let ds = loaded(&repo, "origin/main");

    assert_eq!(ds.error, None);
    let want = [
        "tracked1.txt",
        "tracked2.txt",
        "untracked1.txt",
        "untracked2.txt",
    ];
    assert_eq!(listed_paths(&ds), want);
    for path in want {
        assert!(ds.display_index_for_path(path).is_some(), "{path}");
    }
}

/// git init 直後の unborn HEAD。比較対象のツリーが無いので 0 件ではなくエラーを出す。
#[test]
fn コミット前のheadでも落ちずにエラーを返す() {
    let repo = TestRepo::new();
    repo.file("untracked.txt", "new");

    let ds = loaded(&repo, "main");

    assert!(
        ds.error.as_deref().is_some_and(|e| e.contains("HEAD")),
        "{:?}",
        ds.error
    );
    assert!(ds.files.is_empty());
    assert!(ds.display_list.is_empty());
}

/// コミット済みと未コミットを別々に数えていた頃は、コミット後に再編集したファイルが 2 行に
/// 分かれていた。1 本の diff なので行数もベースからの合計になる。
#[test]
fn コミット後に編集したファイルも1エントリのまま() {
    let repo = TestRepo::new();
    let base = repo.commit_tree(None, &[("a.txt", b"base\n")]);
    repo.branch_at("main", base);
    let head = repo.commit_tree(Some(base), &[("a.txt", b"base\ncommitted\n")]);
    repo.checkout_at("feature", head);
    repo.file("a.txt", "base\ncommitted\nedited\n");

    let ds = loaded(&repo, "main");

    assert_eq!(listed_paths(&ds), ["a.txt"]);
    assert_eq!((ds.files[0].added_lines, ds.files[0].deleted_lines), (2, 0));
    let rows: Vec<&str> = (0..ds.display_list.len())
        .filter_map(|idx| ds.resolve_file(idx))
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(rows, ["a.txt"]);
}

/// 読めなかったことは削除された証拠ではない。読めない内容を空文字列に落とすと
/// 1 行直しただけのファイルが +0 -<全行数> と出て git diff と食い違う。
#[test]
fn 読めないファイルを全行削除にでっち上げない() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let oid = repo.commit_tree(None, &[("a.txt", original.as_bytes())]);
    repo.checkout_at("main", oid);
    repo.file("a.txt", original.replace("line 5\n", "line five\n"));
    let path = repo.path.join("a.txt");
    let set_mode = |mode| {
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(&path, perms).unwrap();
    };
    set_mode(0o000);

    let (files, _) = DiffState::compute_changed_files(&repo.path, "main", false, 4).unwrap();

    set_mode(0o644);
    if let Some(f) = files.iter().find(|f| f.path == "a.txt") {
        assert!(
            f.deleted_lines < 20,
            "read failure was reported as a full-file deletion: +{} -{}",
            f.added_lines,
            f.deleted_lines
        );
    }
}

#[test]
fn 本当に消えたファイルは全行削除として出す() {
    let repo = TestRepo::new();
    let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let oid = repo.commit_tree(None, &[("a.txt", original.as_bytes())]);
    repo.checkout_at("main", oid);
    std::fs::remove_file(repo.path.join("a.txt")).unwrap();

    let (files, _) = DiffState::compute_changed_files(&repo.path, "main", false, 4).unwrap();

    let f = files
        .iter()
        .find(|f| f.path == "a.txt")
        .expect("deleted file should still be listed");
    assert_eq!((f.added_lines, f.deleted_lines), (0, 20));
}

/// git2 の Patch::from_diff はバイナリでも Some (ハンク 0) を返す。「変更なし」と同じ
/// 扱いで落とすと、変更したバイナリが一覧から消える。
#[test]
fn バイナリは行数無しで一覧に残る() {
    let repo = TestRepo::new();
    let oid = repo.commit_tree(None, &[("logo.png", &[0u8, 1, 2, 0, 3, 4][..])]);
    repo.checkout_at("main", oid);
    repo.file("logo.png", [0u8, 9, 9, 0, 7, 7, 7]);

    let (files, _) = DiffState::compute_changed_files(&repo.path, "main", false, 4).unwrap();

    let f = files
        .iter()
        .find(|f| f.path == "logo.png")
        .expect("a changed binary file must stay in the list");
    assert_eq!((f.added_lines, f.deleted_lines), (0, 0));
    assert!(f.hunks.is_empty());
}

/// 大文字小文字を区別しない FS では同じファイルなので、内容まで同じなら変更ではない。
#[test]
fn 大小だけのリネームは内容が同じときだけ隠す() {
    for (label, renamed_content, want_listed) in [
        ("内容が同じ", &b"image data"[..], false),
        ("内容も変わった", &b"image data v2"[..], true),
    ] {
        let repo = TestRepo::new();
        let first = repo.commit_tree(None, &[("Photo.png", b"image data")]);
        repo.branch_at("main", first);
        let mut builder = repo.repo.treebuilder(None).unwrap();
        let blob = repo.repo.blob(renamed_content).unwrap();
        builder.insert("photo.png", blob, 0o100644).unwrap();
        let tree = repo.repo.find_tree(builder.write().unwrap()).unwrap();
        let sig = crate::test_support::signature();
        let parent = repo.repo.find_commit(first).unwrap();
        let second = repo
            .repo
            .commit(None, &sig, &sig, "rename case", &tree, &[&parent])
            .unwrap();
        repo.checkout_at("feature", second);

        let (paths, _) = changed(&repo, "main");

        assert_eq!(!paths.is_empty(), want_listed, "{label}: {paths:?}");
    }
}

/// ケース違いの 2 エントリに実ファイルが 1 つしか無い状態を git 本体は clean と報告する。
/// cfg で外してあるのは、走らない環境で「テストが無い」ことが一覧から見えるようにするため。
#[cfg(target_os = "macos")]
#[test]
fn 大小が衝突するエントリを削除として出さない() {
    let repo = TestRepo::new();
    if !crate::test_support::fs_ignores_case(&repo.path) {
        eprintln!("skipped: 大文字小文字を区別するファイルシステムでは再現しない");
        return;
    }
    let oid = repo.commit_tree(
        None,
        &[
            ("Instagram.png", b"image data\n"),
            ("instagram.png", b"image data\n"),
        ],
    );
    repo.checkout_at("main", oid);

    let (paths, _) = changed(&repo, "main");

    assert!(
        paths.is_empty(),
        "case-colliding entry reported as a change: {paths:?}"
    );
}

#[test]
fn ハンクには直上の関数ヘッダーが付く() {
    let repo = TestRepo::new();
    let source = "use std::fmt;\n\nfn greet() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    let e = 5;\n}\n";
    let oid = repo.commit_tree(None, &[("a.rs", source.as_bytes())]);
    repo.checkout_at("main", oid);
    repo.file("a.rs", source.replace("let e = 5;", "let e = 50;"));

    let (files, _) = DiffState::compute_changed_files(&repo.path, "main", false, 4).unwrap();

    assert_eq!(
        files[0].hunks[0].func_header.as_deref(),
        Some("fn greet() {")
    );
}

#[test]
fn 単語diffは並び順で対応する削除と追加の組にだけ付く() {
    let repo = TestRepo::new();
    let oid = repo.commit_tree(None, &[("a.txt", b"hello world\nextra\n")]);
    repo.checkout_at("main", oid);
    repo.file("a.txt", "hello rust\n");

    let (files, _) = DiffState::compute_changed_files(&repo.path, "main", true, 4).unwrap();

    type Shape<'a> = (DiffLineTag, &'a str, Vec<(&'a str, bool)>);
    let lines = &files[0].hunks[0].lines;
    let shape: Vec<Shape> = lines
        .iter()
        .map(|l| {
            (
                l.tag,
                l.content.as_str(),
                l.inline_segments
                    .iter()
                    .map(|s| (s.text.as_str(), s.emphasized))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (
                DiffLineTag::Delete,
                "hello world",
                vec![("hello", false), (" ", false), ("world", true)]
            ),
            (DiffLineTag::Delete, "extra", vec![]),
            (
                DiffLineTag::Insert,
                "hello rust",
                vec![("hello", false), (" ", false), ("rust", true)]
            ),
        ]
    );
}

#[test]
fn 表示リストはディレクトリを先にファイルを後に深さ付きで並べる() {
    let mut ds = diff_state_with(&["src/b.rs", "top.txt", "src/a.rs", "src/deep/c.rs"]);
    ds.has_summary = true;
    ds.rebuild_display_list();

    let dir = |path: &str, name: &str, depth| DiffListEntry::Directory {
        path: path.to_string(),
        name: name.to_string(),
        depth,
        collapsed: false,
    };
    let file_at = |path: &str, depth| DiffListEntry::File {
        file_index: ds.files.iter().position(|f| f.path == path).unwrap(),
        depth,
    };
    assert_eq!(
        ds.display_list,
        vec![
            DiffListEntry::Summary,
            dir("src", "src", 0),
            dir("src/deep", "deep", 1),
            file_at("src/deep/c.rs", 2),
            file_at("src/a.rs", 1),
            file_at("src/b.rs", 1),
            file_at("top.txt", 0),
        ]
    );
}

#[test]
fn パスから表示行の位置を引ける() {
    let ds = diff_state_with(&["src/a.rs", "src/b.rs"]);

    assert_eq!(ds.display_index_for_path("src/a.rs"), Some(1));
    assert_eq!(ds.display_index_for_path("src/b.rs"), Some(2));
    assert_eq!(ds.display_index_for_path("src/missing.rs"), None);
}

/// ./src/b.rs や git diff の b/ 接頭辞付きで保存された節は diff 自身の綴りに解決する。
/// 寛容さが「この diff には無い」を別ファイルへのジャンプに変えてはならない。
#[test]
fn 変更パスの解決() {
    let cases: [(&str, &[&str], &str, Option<&str>); 15] = [
        ("完全一致", &["src/a.rs"], "src/a.rs", Some("src/a.rs")),
        ("./ 付き", &["src/a.rs"], "./src/a.rs", Some("src/a.rs")),
        (
            "連続スラッシュ",
            &["src/a.rs"],
            "src//a.rs",
            Some("src/a.rs"),
        ),
        (
            "前後の空白",
            &["src/a.rs"],
            "  src/a.rs  ",
            Some("src/a.rs"),
        ),
        (
            "末尾スラッシュ",
            &["src/a.rs"],
            "src/a.rs/",
            Some("src/a.rs"),
        ),
        (
            "git diff の b/",
            &["src/a.rs"],
            "b/src/a.rs",
            Some("src/a.rs"),
        ),
        ("git diff の a/", &["top.txt"], "a/top.txt", Some("top.txt")),
        (
            "サブディレクトリからの相対",
            &["src/deep/c.rs"],
            "deep/c.rs",
            Some("src/deep/c.rs"),
        ),
        ("diff に無い", &["src/a.rs"], "src/untouched.rs", None),
        ("空", &["src/a.rs"], "", None),
        ("./ だけ", &["src/a.rs"], "./", None),
        (
            "末尾一致が曖昧",
            &["src/app/mod.rs", "src/ui/mod.rs"],
            "mod.rs",
            None,
        ),
        (
            "末尾一致が一意",
            &["src/app/mod.rs", "src/ui/mod.rs"],
            "ui/mod.rs",
            Some("src/ui/mod.rs"),
        ),
        (
            "実在する b/ は接頭辞落としより先",
            &["b/src/a.rs", "src/a.rs"],
            "b/src/a.rs",
            Some("b/src/a.rs"),
        ),
        (
            "b/ があっても素のパスはそのまま",
            &["b/src/a.rs", "src/a.rs"],
            "src/a.rs",
            Some("src/a.rs"),
        ),
    ];
    for (label, paths, input, want) in cases {
        let ds = diff_state_with(paths);
        assert_eq!(ds.resolve_changed_path(input).as_deref(), want, "{label}");
    }
}

/// 折りたたんだディレクトリの中のファイルには行が無い。ジャンプ先が無いのと区別するため
/// 途中まで展開する。
#[test]
fn revealは畳まれた親を開く() {
    let mut ds = diff_state_with(&["src/deep/nested/c.rs"]);
    ds.collapsed_dirs.insert("src".to_string());
    ds.rebuild_display_list();
    assert_eq!(
        ds.display_index_for_path("src/deep/nested/c.rs"),
        None,
        "precondition"
    );

    let idx = ds
        .reveal_path("src/deep/nested/c.rs")
        .expect("row after reveal");

    assert_eq!(
        ds.resolve_file(idx).map(|f| f.path.as_str()),
        Some("src/deep/nested/c.rs")
    );
    assert_eq!(ds.reveal_path("src/missing.rs"), None);
}

#[test]
fn 折りたたみはディレクトリ行だけを変えファイル行では何もしない() {
    let mut ds = diff_state_with(&["src/a.rs", "top.txt"]);
    let rows_open = ds.display_list.clone();

    assert!(!ds.toggle_section(1), "ファイル行");
    assert!(!ds.toggle_section(9), "範囲外");
    ds.collapse_section(1);
    ds.expand_section(0);
    assert_eq!(ds.display_list, rows_open, "no-op は表示リストを変えない");

    assert!(ds.toggle_section(0));
    assert_eq!(ds.display_index_for_path("src/a.rs"), None);
    ds.collapse_section(0);
    assert_eq!(
        ds.display_index_for_path("src/a.rs"),
        None,
        "畳み済みを畳んでも落ちない"
    );

    assert!(ds.toggle_section(0));
    ds.expand_section(0);
    assert_eq!(ds.display_list, rows_open, "展開済みを開いても落ちない");
}
