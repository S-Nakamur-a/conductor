// analyze() のテスト。git の呼び方そのものは git.rs 側の関心なので、ここでは
// git・差分・AI の応答の組み合わせ方だけを実物のリポジトリで見る。

use super::*;
use revidere_fixtures::{Repo, Section as S};
use std::cell::RefCell;

/// 決め打ちの答えを返す AI。差し戻しかどうかは、差し戻し用のプロンプトに
/// しか現れない文言 (prompt::repair の冒頭) で見分ける。
struct StubAi {
    first: String,
    repair: String,
    calls: RefCell<usize>,
}

impl StubAi {
    fn new(first: &str, repair: &str) -> Self {
        Self {
            first: first.to_string(),
            repair: repair.to_string(),
            calls: RefCell::new(0),
        }
    }

    /// 初回も差し戻しも同じ答えを返す。
    fn always(answer: &str) -> Self {
        Self::new(answer, answer)
    }

    fn calls(&self) -> usize {
        *self.calls.borrow()
    }
}

impl Ai for StubAi {
    fn complete(&self, _system: &str, user: &str) -> Result<String, String> {
        *self.calls.borrow_mut() += 1;
        if user.contains("直前の回答では") {
            Ok(self.repair.clone())
        } else {
            Ok(self.first.clone())
        }
    }

    fn identity(&self) -> String {
        "stub".to_string()
    }
}

/// 1 項目だけの答え。その項目が a.txt の後像 from..=to を持つ。
fn answer_covering(title: &str, from: u32, to: u32) -> String {
    revidere_fixtures::answer(&revidere_fixtures::sections(&[
        S::new(title, "core").lines("a.txt", "new", from, Some(to))
    ]))
}

fn options(repo: &Repo, base: &str) -> Options {
    Options {
        repo: repo.dir().to_path_buf(),
        base: Some(base.to_string()),
        cache: true,
        scope: crate::review::Scope::Base,
    }
}

fn artifact(repo: &Repo, scope: crate::review::Scope) -> Review {
    let root = git::root(repo.dir()).unwrap();
    let text = std::fs::read_to_string(crate::review::artifact_path(&root, scope)).unwrap();
    Review::from_json(&text).unwrap()
}

/// 成果物そのものを差分に数えさせない。実際の利用でも `.conductor` は無視されている。
fn repo_ignoring_artifacts() -> Repo {
    let repo = Repo::new("analyze-test");
    repo.write(".gitignore", ".conductor/\n");
    repo
}

/// base コミットの上に、a.txt に 1 行足した feature ブランチを作る。
fn one_line_feature(repo: &Repo) -> (String, String) {
    repo.write("a.txt", "1\n");
    let base = repo.commit_all("base");
    repo.branch("feature");
    repo.write("a.txt", "1\n2\n");
    let head = repo.commit_all("first");
    (base, head)
}

#[test]
fn 置き場所が無ければ作ってから書く() {
    let repo = Repo::new("analyze-artifact");
    let path = repo.dir().join("nested").join("review.json");
    let r = Review {
        schema: crate::review::SCHEMA_VERSION,
        base: "a".into(),
        head: "b".into(),
        overview: crate::Overview {
            problem: "p".into(),
            change: "c".into(),
            mechanism: "m".into(),
            placement: "pl".into(),
            scope: "s".into(),
        },
        sections: Vec::new(),
        impacts: Vec::new(),
        coverage: crate::Coverage::default(),
        since_previous: None,
    };
    write_artifact(&path, &r).unwrap();
    assert!(path.exists());
}

/// 差分が無いのは 0 件成功ではなくエラー。AI を起こす前に分かる。
#[test]
fn 変更の無い区間は黙って成功せず明示的なエラーになる() {
    let repo = Repo::new("analyze-test");
    repo.write("a.txt", "1\n2\n3\n");
    let oid = repo.commit_all("base");
    let ai = StubAi::always("");
    let e = analyze(&options(&repo, &oid), &ai).unwrap_err();
    assert!(matches!(e, AnalyzeError::NoDiff(_)), "{e:?}");
    assert_eq!(ai.calls(), 0, "差分が無いなら AI を起こす理由が無い");
}

/// 台帳が最後のコミットだけになっていると、それより前のコミットを指した項目が
/// 「存在しない行を指した」側に落ちるので、説明もれ検査で捕まる。
#[test]
fn 解析はベース以降の全コミットと未コミット分を見る() {
    let repo = Repo::new("analyze-test");
    repo.write("a.txt", "1\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.write("a.txt", "1\n2\n");
    repo.commit_all("first");
    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("second");
    repo.write("a.txt", "1\n2\n3\n4\n");

    let ai = StubAi::always(&answer_covering("lines 2-4", 2, 4));
    let r = analyze(&options(&repo, "main"), &ai).unwrap();

    assert_eq!(r.coverage.total, 3, "2 つのコミットと手元の変更で 3 行");
    assert!(r.coverage.is_complete(), "{:?}", r.coverage);
    assert_eq!(ai.calls(), 1, "全部説明できていれば差し戻しは要らない");
}

#[test]
fn 差し戻しで悪化したら最初の結果を残す() {
    let repo = Repo::new("analyze-test");
    repo.write("a.txt", "1\n2\n3\n");
    let base = repo.commit_all("base");
    // 追加行を 2 つにして、初回で 1 件だけ説明なしのまま残す。
    repo.write("a.txt", "1\n2\n3\n4\n5\n");
    repo.commit_all("head");

    let nothing_classified = revidere_fixtures::answer(&revidere_fixtures::sections(&[S::new(
        "何も分類しない",
        "minor",
    )]));
    let ai = StubAi::new(&answer_covering("add line 4", 4, 4), &nothing_classified);
    let r = analyze(&options(&repo, &base), &ai).unwrap();

    assert_eq!(ai.calls(), 2, "説明なしが残ったなら差し戻しているはず");
    assert!(!r.coverage.is_complete(), "説明なしが残っている");
    assert_eq!(
        artifact(&repo, crate::review::Scope::Base)
            .coverage
            .unclassified
            .len(),
        1,
        "悪化した差し戻し後の結果ではなく、最初の結果が残っているはず"
    );
}

#[test]
fn 初回のレビューに前回からの進みは付かない() {
    let repo = repo_ignoring_artifacts();
    repo.write("a.txt", "1\n");
    let base = repo.commit_all("base");
    repo.write("a.txt", "1\n2\n");

    let ai = StubAi::always(&answer_covering("add line 2", 2, 2));
    let r = analyze(&options(&repo, &base), &ai).unwrap();
    assert!(r.since_previous.is_none());
}

#[test]
fn 二度目のレビューは前回から動いたものを出す() {
    let repo = repo_ignoring_artifacts();
    let (base, first_head) = one_line_feature(&repo);
    let ai = StubAi::always(&answer_covering("changes", 2, 3));
    analyze(&options(&repo, &base), &ai).unwrap();

    repo.write("a.txt", "1\n2\n3\n");
    repo.write("later.txt", "added after the review\n");
    let second_head = repo.commit_all("second");

    let r = analyze(&options(&repo, &base), &ai).unwrap();
    let since = r.since_previous.expect("2 度目には前回からの進みが付く");
    assert!(first_head.starts_with(&since.previous_head), "{since:?}");
    assert!(second_head.starts_with(&since.head), "{since:?}");
    assert_eq!(
        since.files,
        Some(vec!["a.txt".to_string(), "later.txt".to_string()])
    );
    assert!(!since.history_rewritten);
}

/// ここで起点が今になると、読む前に最新化しただけで進みが消える。差分が動いて
/// いなければ AI も呼ばない空振りに見える操作なのに、成果物は上書きされている。
#[test]
fn コミットせずに解析し直しても比較の起点は動かない() {
    let repo = repo_ignoring_artifacts();
    let (base, first_head) = one_line_feature(&repo);
    let ai = StubAi::always(&answer_covering("changes", 2, 3));
    analyze(&options(&repo, &base), &ai).unwrap();

    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("second");
    analyze(&options(&repo, &base), &ai).unwrap();

    let calls_before = ai.calls();
    let r = analyze(&options(&repo, &base), &ai).unwrap();
    assert_eq!(
        ai.calls(),
        calls_before,
        "貯めた応答に当たって AI は走らない"
    );

    let since = r.since_previous.expect("起点が消えていないこと");
    assert!(
        first_head.starts_with(&since.previous_head),
        "起点は 2 度目のときと同じ first のまま: {since:?}"
    );
    assert_eq!(since.files, Some(vec!["a.txt".to_string()]));
}

/// 前回のコミットがもう残っていない (gc 済み、あるいは別のリポジトリの成果物)
/// ときは一覧を引けない。それを「変わったファイルは無い」に畳むと、山ほど
/// 動いていても無いと言い切ることになる。
#[test]
fn 辿れない前回headは空の一覧ではなく無しを返す() {
    let repo = repo_ignoring_artifacts();
    let (base, _) = one_line_feature(&repo);
    let ai = StubAi::always(&answer_covering("changes", 2, 3));
    analyze(&options(&repo, &base), &ai).unwrap();

    let root = git::root(repo.dir()).unwrap();
    let mut stored = artifact(&repo, crate::review::Scope::Base);
    stored.head = "0123456789abcdef0123456789abcdef01234567".to_string();
    write_artifact(
        &crate::review::artifact_path(&root, crate::review::Scope::Base),
        &stored,
    )
    .unwrap();

    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("second");

    let r = analyze(&options(&repo, &base), &ai).unwrap();
    let since = r.since_previous.expect("2 度目には前回からの進みが付く");
    assert!(since.history_rewritten, "{since:?}");
    assert_eq!(since.files, None, "引けなかったことを空と畳まない");
}

#[test]
fn 履歴が書き換わったことは進みの要約に印が付く() {
    let repo = repo_ignoring_artifacts();
    let (base, _) = one_line_feature(&repo);
    let ai = StubAi::always(&answer_covering("changes", 2, 2));
    analyze(&options(&repo, &base), &ai).unwrap();

    // 履歴ごと差し替える。前回の対象コミットはもう辿れない。
    repo.git(&["reset", "-q", "--hard", &base]);
    repo.write("a.txt", "1\nrewritten\n");
    repo.commit_all("rewritten");

    let r = analyze(&options(&repo, &base), &ai).unwrap();
    let since = r.since_previous.expect("2 度目には前回からの進みが付く");
    assert!(since.history_rewritten, "{since:?}");
}

/// 片方を見ている間にもう片方が消えると、行き来した時点で読んでいたものが変わる。
#[test]
fn 前回差分の成果物はブランチ全体の隣に置く() {
    let repo = repo_ignoring_artifacts();
    let (base, first_head) = one_line_feature(&repo);
    let ai = StubAi::always(&answer_covering("changes", 2, 3));
    analyze(&options(&repo, &base), &ai).unwrap();

    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("second");
    let branch = analyze(&options(&repo, &base), &ai).unwrap();
    let previous = branch
        .since_previous
        .expect("2 度目には前回からの進みが付く")
        .previous_head;
    assert!(first_head.starts_with(&previous));

    let mut delta_options = options(&repo, &previous);
    delta_options.scope = crate::review::Scope::SincePrevious;
    let delta = analyze(&delta_options, &ai).unwrap();

    assert_eq!(delta.base, previous, "起点は前回のレビューのコミット");
    assert!(
        delta.since_previous.is_none(),
        "前回からの差分そのものを見ているレビューは、さらに前回からの進みを持たない"
    );

    let root = git::root(repo.dir()).unwrap();
    assert!(crate::review::artifact_path(&root, crate::review::Scope::SincePrevious).exists());
    let still_there = artifact(&repo, crate::review::Scope::Base);
    assert_eq!(
        still_there.base,
        base[..still_there.base.len()],
        "ブランチ全体のレビューは上書きされていない"
    );
}
