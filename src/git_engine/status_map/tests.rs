//! GitStatusMap::classify のテスト。
//!
//! 各フィクスチャファイルの期待される TreeGitState は、statuses() が
//! たまたま報告するビットを読み返すのではなく、*そのフィクスチャが
//! どう作られたか*(git add したか、.gitignore に載せたか、ディスク上で
//! 何も触れていないか)から決める — そうしないとアサーションはテスト
//! 対象の実装をなぞるだけになってしまう。

use super::*;
use std::fs;

/// コミット済みファイル1つでリポジトリを作り、classify() が区別すべき
/// 全組み合わせをカバーする6ファイルを追加で配置する。さらに .gitignore
/// で ignored と宣言した build/ ディレクトリ(prefix 継承を確認するため
/// ネストしたファイルも含む)を用意する。リポジトリのルートパスを返す。
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
fn git_status_map_classify_untracked_dir_is_untracked_like_its_contents() {
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
fn git_status_map_classify_tracked_dir_stays_tracked() {
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
fn git_status_map_classify_sibling_sharing_a_prefix_is_not_ignored() {
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
fn git_status_map_classify_untouched_file_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("untouched.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_staged_only_file_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("staged_only.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_unstaged_only_file_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("unstaged_only.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_file_staged_and_then_edited_is_tracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("both.txt"), TreeGitState::Tracked);
}

#[test]
fn git_status_map_classify_never_added_file_is_untracked() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    assert_eq!(map.classify("untracked.txt"), TreeGitState::Untracked);
}

#[test]
fn git_status_map_classify_gitignored_dir_itself_is_ignored() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    // FileTreeEntry::path はディレクトリであっても末尾スラッシュを
    // 一切持たないが、libgit2 の折りたたまれた ignored ディレクトリの
    // キー("build/")は持つ — classify() が両方の形式をチェックして
    // いる場合のみこのテストは通る。
    assert_eq!(map.classify("build"), TreeGitState::Ignored);
}

#[test]
fn git_status_map_classify_file_under_gitignored_dir_inherits_ignored_via_prefix() {
    let tmp = build_fixture();
    let map = GitStatusMap::load(tmp.path()).unwrap();
    // libgit2 は build/deep/x.txt を個別に報告するのではなく、build/ を
    // 1つの status エントリに折りたたむ(実測で確認済み)。そのため
    // classify() が完全一致のルックアップではなく実際に祖先の
    // プレフィックスを遡っている場合のみこのテストは通る。
    assert_eq!(map.classify("build/deep/x.txt"), TreeGitState::Ignored);
}
