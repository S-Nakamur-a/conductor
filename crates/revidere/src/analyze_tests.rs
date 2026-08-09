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

    fn options(&self, base: Option<&str>, head: &str) -> Options {
        Options {
            repo: self.dir.clone(),
            base: base.map(str::to_string),
            head: head.to_string(),
            cache: true,
        }
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
    };
    write_artifact(&path, &r).unwrap();
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// base...head に差分が無いのは、0 件成功ではなく明示的なエラー。
// AI を起こす前に分かるので、起こさないことも併せて見る。
#[test]
fn a_range_with_no_changes_is_an_explicit_error_not_a_silent_success() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let oid = repo.commit_all("base");
    let ai = StubAi::new("", "");
    let e = analyze(&repo.options(Some(&oid), &oid), &ai).unwrap_err();
    assert!(matches!(e, AnalyzeError::NoDiff(_)), "{e:?}");
    assert_eq!(ai.calls(), 0, "差分が無いなら AI を起こす理由が無い");
}

// コミット範囲を見ているときに作業ツリーが汚れていても、
// 処理は止めずに進む（警告だけを出す）。
#[test]
fn analyze_does_not_stop_when_the_worktree_is_dirty_during_a_commit_range_review() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let base = repo.commit_all("base");
    repo.write("a.txt", "1\n2\n3\n4\n");
    let head = repo.commit_all("head");
    // レビュー対象は上のコミット範囲。その後で作業ツリーだけ汚す。
    repo.write("a.txt", "1\n2\n3\n4\nscratch\n");

    let full = answer(
        r#"{"title":"add line 4","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":4,"end":4}]}"#,
    );
    let ai = StubAi::new(&full, &full);
    let r = analyze(&repo.options(Some(&base), &head), &ai)
        .expect("汚れた作業ツリーで analyze を止めてはいけない");
    assert!(r.coverage.is_complete());
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
    let head = repo.commit_all("head");

    let initial = answer(
        r#"{"title":"add line 4","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":4,"end":4}]}"#,
    );
    // 差し戻し後の応答は、2 件とも分類しない（悪化させる）。
    let repaired = answer(
        r#"{"title":"nothing classified","importance":"minor","reason":"placeholder","body":"","ranges":[]}"#,
    );
    let ai = StubAi::new(&initial, &repaired);
    let r = analyze(&repo.options(Some(&base), &head), &ai).unwrap();

    assert_eq!(ai.calls(), 2, "説明なしが残ったなら差し戻しているはず");
    assert!(!r.coverage.is_complete(), "説明なしが残っている");
    assert_eq!(
        repo.artifact().coverage.unclassified.len(),
        1,
        "悪化した差し戻し後の結果ではなく、最初の結果が残っているはず"
    );
}
