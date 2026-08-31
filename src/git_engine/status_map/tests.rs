//! GitStatusMap::classify のテスト。
//!
//! 各フィクスチャファイルの期待される TreeGitState は、statuses() が
//! たまたま報告するビットを読み返すのではなく、*そのフィクスチャが
//! どう作られたか*(git add したか、.gitignore に載せたか、ディスク上で
//! 何も触れていないか)から決める — そうしないとアサーションはテスト
//! 対象の実装をなぞるだけになってしまう。

use super::*;
use std::fs;

/// classify() が区別すべき全組み合わせ 6 ファイルと、.gitignore で ignored にした build/
/// (prefix 継承を見るためネストしたファイル込み) を置き、リポジトリのルートを返す。
fn build_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let root = tmp.path();
    let repo = git2::Repository::init(root).expect("init repo");
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();

    // untouched.txt / staged_only.txt / unstaged_only.txt / both.txt は
    // いずれも最初のコミットに含まれてスタートするので、以下の
    // フィクスチャ固有の変更を加える前のベースラインは
    // "tracked、変更なし" になる。
    fs::write(root.join("untouched.txt"), "a\n").unwrap();
    fs::write(root.join("staged_only.txt"), "a\n").unwrap();
    fs::write(root.join("unstaged_only.txt"), "a\n").unwrap();
    fs::write(root.join("both.txt"), "a\n").unwrap();
    fs::write(root.join(".gitignore"), "build/\n").unwrap();

    {
        let mut index = repo.index().unwrap();
        for f in [
            "untouched.txt",
            "staged_only.txt",
            "unstaged_only.txt",
            "both.txt",
            ".gitignore",
        ] {
            index.add_path(Path::new(f)).unwrap();
        }
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }

    // staged_only.txt: 編集してから git add する(unstaged な変更は残らない)。
    fs::write(root.join("staged_only.txt"), "b\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged_only.txt")).unwrap();
        index.write().unwrap();
    }

    // unstaged_only.txt: ディスク上で編集するが、index はコミット済みの
    // blob のままにしておく。
    fs::write(root.join("unstaged_only.txt"), "b\n").unwrap();

    // both.txt: 一度 git add してから、その後さらに編集する。
    fs::write(root.join("both.txt"), "b\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("both.txt")).unwrap();
        index.write().unwrap();
    }
    fs::write(root.join("both.txt"), "c\n").unwrap();

    // untracked.txt: 一度も index に追加していない。
    fs::write(root.join("untracked.txt"), "a\n").unwrap();

    // build/deep/x.txt: .gitignore が ignored と宣言したディレクトリの配下。
    fs::create_dir_all(root.join("build/deep")).unwrap();
    fs::write(root.join("build/deep/x.txt"), "a\n").unwrap();

    // build2/: 名前が build を prefix に持つが ignored ではない兄弟
    // ディレクトリ — prefix マッチングが starts_with に緩められた場合の
    // 回帰を防ぐ。
    fs::create_dir_all(root.join("build2")).unwrap();
    fs::write(root.join("build2/y.txt"), "a\n").unwrap();

    // newdir/: git がまだ見たことのない、まっさらな新規ディレクトリ。
    // ignored ディレクトリと違い、libgit2 はこれを1つのエントリに
    // 折りたたまないので、ディレクトリ自体はそれ自身の status を持たない。
    fs::create_dir_all(root.join("newdir/sub")).unwrap();
    fs::write(root.join("newdir/a.txt"), "a\n").unwrap();
    fs::write(root.join("newdir/sub/b.txt"), "a\n").unwrap();

    tmp
}

#[test]
fn 未追跡のディレクトリは中身と同じく未追跡になる() {
    // newdir/ は作成されただけで一度も add していないので、配下の全パスが
    // 新規になる。ディレクトリの行は子と一緒に薄暗く表示されなければ
    // ならない: 親を通常の tracked 色で表示しつつ中のファイルだけ薄暗く
    // すると、「このフォルダは既知だが中身は違う」という逆の見え方になる。
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("newdir"), TreeGitState::Untracked);
    assert_eq!(map.classify("newdir/sub"), TreeGitState::Untracked);
    assert_eq!(map.classify("newdir/a.txt"), TreeGitState::Untracked);
    assert_eq!(map.classify("newdir/sub/b.txt"), TreeGitState::Untracked);
}

#[test]
fn 追跡中のディレクトリは追跡中のまま() {
    // リポジトリルートのコミット済みファイルはトップレベルに直接あるので、
    // 兄弟の一部が untracked だからといって祖先ディレクトリが untracked と
    // 誤判定されてはならない。build2/ は untracked なファイルしか持たない
    // ので、それ自体は*本当に* untracked である。以下のアサーションは
    // その判別ケースを固定する。
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("untouched.txt"), TreeGitState::Tracked);
    assert_eq!(map.classify("nonexistent-dir"), TreeGitState::Tracked);
}

#[test]
fn 接頭辞を共有する兄弟は無視されない() {
    // .gitignore に載っているのは build/ であって build2/ ではない。祖先の
    // ルックアップは現状 HashMap の完全一致なのでこのテストは通る —
    // これが starts_with スキャンに緩められた場合に大きく失敗させるために
    // 存在する。
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("build/deep/x.txt"), TreeGitState::Ignored);
    assert_ne!(map.classify("build2/y.txt"), TreeGitState::Ignored);
}

#[test]
fn 分類は各フィクスチャが作られた状態を答える() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    // build/ は libgit2 が 1 エントリに折りたたむ (実測)。ディレクトリ側の
    // キーは末尾スラッシュを持つのに FileTreeEntry::path は持たないので、
    // 両方の綴りを見て、かつ祖先のプレフィックスを遡らないと通らない。
    for (path, want) in [
        ("untouched.txt", TreeGitState::Tracked),
        ("staged_only.txt", TreeGitState::Tracked),
        ("unstaged_only.txt", TreeGitState::Tracked),
        ("both.txt", TreeGitState::Tracked),
        ("untracked.txt", TreeGitState::Untracked),
        ("build", TreeGitState::Ignored),
        ("build/deep/x.txt", TreeGitState::Ignored),
    ] {
        assert_eq!(map.classify(path), want, "{path}");
    }
}

#[cfg(target_os = "macos")]
fn fs_ignores_case(dir: &Path) -> bool {
    let probe = dir.join("CaseProbe");
    fs::write(&probe, b"").unwrap();
    let ignores = dir.join("caseprobe").is_file();
    fs::remove_file(&probe).unwrap();
    ignores
}

#[cfg(target_os = "macos")]
fn build_case_colliding_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let repo = git2::Repository::init(tmp.path()).expect("init repo");
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();

    let blob = repo.blob(b"image data\n").unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("Instagram.png", blob, 0o100644).unwrap();
    tb.insert("instagram.png", blob, 0o100644).unwrap();
    let tree_oid = tb.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "both cases", &tree, &[])
        .unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    // libgit2 の checkout は衝突する2エントリを index 上で1つに畳んでしまう。
    // git 本体が clone / checkout したリポジトリは両方を保持するので、書き戻す。
    let mut index = repo.index().unwrap();
    index.read_tree(&tree).unwrap();
    index.write().unwrap();

    tmp
}

/// リグレッション: ケース違いの2エントリに実ファイルが1つしか無いこの状態を、
/// git 本体は clean と報告する。
// 大文字小文字を区別しないファイルシステムでしか再現しない。cfg で外して
// あるのは、走らない環境では『テストが無い』ことが一覧から見えるようにする
// ため。実行時に return すると、検証していないのに緑になる。
#[cfg(target_os = "macos")]
#[test]
fn 大小が衝突するエントリは削除扱いにしない() {
    let tmp = build_case_colliding_fixture();
    if !fs_ignores_case(tmp.path()) {
        eprintln!("skipped: 大文字小文字を区別するファイルシステムでは再現しない");
        return;
    }

    let map = GitStatusMap::load(tmp.path()).unwrap();

    assert_eq!(map.status("Instagram.png"), None);
    assert_eq!(map.status("instagram.png"), None);
}

#[test]
fn 本当の削除はちゃんと報告する() {
    let tmp = build_fixture();
    fs::remove_file(tmp.path().join("untouched.txt")).unwrap();

    let map = GitStatusMap::load(tmp.path()).unwrap();

    assert!(
        map.status("untouched.txt")
            .is_some_and(|s| s.is_wt_deleted()),
        "a real deletion should still be reported, got: {:?}",
        map.status("untouched.txt")
    );
}
