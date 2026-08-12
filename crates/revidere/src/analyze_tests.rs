// analyze() のテスト。
//
// git そのものの呼び方は git.rs 側の関心なので、ここでは git・差分・AI の
// 応答の組み合わせ方だけを、実物の git リポジトリで見る。AI だけは決め打ちの
// 答えを返す [StubAi] に差し替える。

use super::*;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// テストごとに使い捨てにする一時ディレクトリ。git.rs の Repo と同じ理由
/// （テストの並列実行があっても pid だけでは衝突しうる）で連番を混ぜる。
fn unique_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "revidere-analyze-test-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// 決め打ちの答えを返す AI。差し戻しかどうかは、差し戻し用のプロンプトに
/// しか現れない文言（prompt.rs の repair() 冒頭）で見分ける。
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

const ANSWER_TEMPLATE: &str = r#"{"overview":{"problem":"p","change":"c","mechanism":"m","placement":"pl","scope":"s"},"sections":[{sections}],"impacts":[]}"#;

fn answer(sections: &str) -> String {
    ANSWER_TEMPLATE.replace("{sections}", sections)
}

/// テストごとに使い捨てる、実物の git リポジトリ。
struct Repo {
    dir: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = unique_dir("repo");
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repo { dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "t@example.com"]);
        repo.git(&["config", "user.name", "t"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} が失敗した: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn write(&self, path: &str, content: &str) {
        let p = self.dir.join(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    /// 変更を全部コミットして、そのコミットの oid を返す。
    fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// 書き出された成果物。パスは書く側と同じ関数から得る。
    fn artifact(&self) -> Review {
        let root = git::root(&self.dir).unwrap();
        let text = std::fs::read_to_string(crate::review::artifact_path(&root)).unwrap();
        Review::from_json(&text).unwrap()
    }

    fn options(&self, base: Option<&str>) -> Options {
        Options {
            repo: self.dir.clone(),
            base: base.map(str::to_string),
            cache: true,
        }
    }

    /// 成果物そのものを差分に数えさせないための下地。実際の利用でも
    /// `.conductor` は無視されている。
    fn ignore_artifacts(&self) {
        self.write(".gitignore", ".conductor/\n");
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// 成果物の書き出し先の親ディレクトリが無ければ作る。
#[test]
fn write_artifact_creates_the_parent_directory_when_it_is_missing() {
    let dir = unique_dir("artifact");
    let path = dir.join("nested").join("review.json");
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
    let _ = std::fs::remove_dir_all(&dir);
}

// 対象範囲に差分が無いのは、0 件成功ではなく明示的なエラー。
// AI を起こす前に分かるので、起こさないことも併せて見る。
#[test]
fn a_range_with_no_changes_is_an_explicit_error_not_a_silent_success() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let oid = repo.commit_all("base");
    let ai = StubAi::new("", "");
    let e = analyze(&repo.options(Some(&oid)), &ai).unwrap_err();
    assert!(matches!(e, AnalyzeError::NoDiff(_)), "{e:?}");
    assert_eq!(ai.calls(), 0, "差分が無いなら AI を起こす理由が無い");
}

// レビューはベースから今の作業ツリーまでが対象で、直近のコミット 1 つ分では
// ない。台帳が最後のコミットだけになっていると、それより前のコミットを指した
// 節は「存在しない行を指した」側に落ちるので、充足検査で捕まる。
#[test]
fn analyze_covers_every_commit_since_the_base_plus_the_uncommitted_work() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("a.txt", "1\n2\n");
    repo.commit_all("first");
    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("second");
    // まだコミットしていない手元の変更。
    repo.write("a.txt", "1\n2\n3\n4\n");

    let full = answer(
        r#"{"title":"lines 2-4","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":2,"end":4}]}"#,
    );
    let ai = StubAi::new(&full, &full);
    let r = analyze(&repo.options(Some("main")), &ai).unwrap();

    assert_eq!(r.coverage.total, 3, "2 つのコミットと手元の変更で 3 行");
    assert!(r.coverage.is_complete(), "{:?}", r.coverage);
    assert_eq!(ai.calls(), 1, "全部説明できていれば差し戻しは要らない");
}

// 差し戻し（repair）で説明なしが減らなかったら、差し戻し後の結果は
// 採らず元の結果を使う。
#[test]
fn analyze_keeps_the_first_result_when_repair_makes_coverage_worse() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let base = repo.commit_all("base");
    // 追加行を 2 つにして、初回で 1 件だけ説明なしのまま残す。
    repo.write("a.txt", "1\n2\n3\n4\n5\n");
    repo.commit_all("head");

    let initial = answer(
        r#"{"title":"add line 4","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":4,"end":4}]}"#,
    );
    // 差し戻し後の応答は、2 件とも分類しない（悪化させる）。
    let repaired = answer(
        r#"{"title":"nothing classified","importance":"minor","reason":"placeholder","body":"","ranges":[]}"#,
    );
    let ai = StubAi::new(&initial, &repaired);
    let r = analyze(&repo.options(Some(&base)), &ai).unwrap();

    assert_eq!(ai.calls(), 2, "説明なしが残ったなら差し戻しているはず");
    assert!(!r.coverage.is_complete(), "説明なしが残っている");
    assert_eq!(
        repo.artifact().coverage.unclassified.len(),
        1,
        "悪化した差し戻し後の結果ではなく、最初の結果が残っているはず"
    );
}

// 初回は比べる相手が無いので、前回からの進みは付けない。
#[test]
fn the_first_review_carries_no_since_previous_summary() {
    let repo = Repo::new();
    repo.ignore_artifacts();
    repo.write("a.txt", "1\n");
    let base = repo.commit_all("base");
    repo.write("a.txt", "1\n2\n");

    let full = answer(
        r#"{"title":"add line 2","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":2,"end":2}]}"#,
    );
    let ai = StubAi::new(&full, &full);
    let r = analyze(&repo.options(Some(&base)), &ai).unwrap();
    assert!(r.since_previous.is_none());
}

// 2 度目は、前回の対象コミットと今回の HEAD、その間で動いたファイルを持つ。
#[test]
fn a_second_review_reports_what_moved_since_the_previous_one() {
    let repo = Repo::new();
    repo.ignore_artifacts();
    repo.write("a.txt", "1\n");
    let base = repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("a.txt", "1\n2\n");
    let first_head = repo.commit_all("first");

    let full = answer(
        r#"{"title":"changes","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":2,"end":3}]}"#,
    );
    let ai = StubAi::new(&full, &full);
    analyze(&repo.options(Some(&base)), &ai).unwrap();

    // レビューのあとにもう 1 つコミットを積む。
    repo.write("a.txt", "1\n2\n3\n");
    repo.write("later.txt", "added after the review\n");
    let second_head = repo.commit_all("second");

    let r = analyze(&repo.options(Some(&base)), &ai).unwrap();
    let since = r.since_previous.expect("2 度目には前回からの進みが付く");
    assert!(first_head.starts_with(&since.previous_head), "{since:?}");
    assert!(second_head.starts_with(&since.head), "{since:?}");
    assert_eq!(
        since.files,
        vec!["a.txt".to_string(), "later.txt".to_string()]
    );
    assert!(!since.history_rewritten);
}

// 前回のコミットが履歴から消えている（rebase / amend / force push）ことは、
// 黙って「変わっていない」に畳まず、そう言う。
#[test]
fn a_rewritten_history_is_flagged_in_the_since_previous_summary() {
    let repo = Repo::new();
    repo.ignore_artifacts();
    repo.write("a.txt", "1\n");
    let base = repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("a.txt", "1\n2\n");
    repo.commit_all("work that will be dropped");

    let full = answer(
        r#"{"title":"changes","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":2,"end":2}]}"#,
    );
    let ai = StubAi::new(&full, &full);
    analyze(&repo.options(Some(&base)), &ai).unwrap();

    // 履歴ごと差し替える。前回の対象コミットはもう辿れない。
    repo.git(&["reset", "-q", "--hard", &base]);
    repo.write("a.txt", "1\nrewritten\n");
    repo.commit_all("rewritten");

    let r = analyze(&repo.options(Some(&base)), &ai).unwrap();
    let since = r.since_previous.expect("2 度目には前回からの進みが付く");
    assert!(since.history_rewritten, "{since:?}");
}
