// cmd_* とその補助のテスト。
//
// git そのものの呼び方は git.rs 側の関心なので、ここでは git.rs / config.rs /
// ai.rs をどう組み合わせるかだけを、実物の git リポジトリで見る。

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// テストごとに使い捨てにする一時ディレクトリ。git.rs の Repo と同じ理由
/// （テストの並列実行があっても pid だけでは衝突しうる）で連番を混ぜる。
fn unique_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "revidere-cli-test-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// テストごとに使い捨てる、実物の git リポジトリ。
///
/// ここで確かめたいのは git.rs・config.rs・ai.rs を組み合わせた main.rs 側の
/// 振る舞いなので、モックにはしない。
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

    fn write_bytes(&self, path: &str, content: &[u8]) {
        std::fs::write(self.dir.join(path), content).unwrap();
    }

    /// 変更を全部コミットして、そのコミットの oid を返す。
    fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
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
        schema: revidere::review::SCHEMA_VERSION,
        base: "a".into(),
        head: "b".into(),
        overview: revidere::Overview {
            problem: "p".into(),
            change: "c".into(),
            mechanism: "m".into(),
            placement: "pl".into(),
            scope: "s".into(),
        },
        sections: Vec::new(),
        impacts: Vec::new(),
        coverage: revidere::Coverage::default(),
    };
    write_artifact(&path, &r).unwrap();
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// base...head に差分が無いのは、0 件成功ではなく明示的なエラー。
#[test]
fn a_range_with_no_changes_is_an_explicit_error_not_a_silent_success() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let oid = repo.commit_all("base");
    let args = crate::args::DiffArgs {
        repo: repo.dir.clone(),
        base: Some(oid.clone()),
        head: oid,
    };
    let e = cmd_ledger(&args).unwrap_err();
    assert!(matches!(e, CliError::Message(_)), "{e:?}");
}

// config サブコマンドは git リポジトリの外でも答えられる。
//
// この端末の $HOME に常用の設定があるかどうかはテストの制御外なので、
// 結果の中身ではなく「エラーにならないこと」だけを見る
// （config.rs の a_missing_candidate_is_not_an_error と同じ理由）。
#[test]
fn config_command_answers_outside_a_git_repository() {
    let dir = unique_dir("norepo");
    std::fs::create_dir_all(&dir).unwrap();
    let args = crate::args::ConfigArgs {
        repo: dir.clone(),
        config: None,
        ai: None,
        timeout: None,
        cache: true,
    };
    assert!(cmd_config(&args).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

// AI コマンドが決まらないなら、差分を読み込む前にエラーを返す。
//
// base に実在しない参照を渡し、かつ --config を実在しないパスに固定する。
// 差分側が先に動く実装なら git のエラーが返るはずで、この順序で
// CliError::Config が返ることが「AI 側を先に見ている」の証拠になる。
#[test]
fn analyze_reports_the_missing_ai_configuration_before_touching_a_broken_diff() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let head = repo.commit_all("base");
    let args = crate::args::AnalyzeArgs {
        repo: repo.dir.clone(),
        base: Some("does-not-exist".into()),
        head,
        out: None,
        config: Some(PathBuf::from("/no/such/config-for-test.toml")),
        ai: None,
        timeout: None,
        repair: true,
        cache: true,
    };
    let e = cmd_analyze(&args).unwrap_err();
    assert!(matches!(e, CliError::Config(_)), "{e:?}");
}

// verify は変更一覧の集計を git 自身の numstat と突き合わせる。
#[test]
fn verify_matches_git_numstat_for_a_clean_commit_range() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    let base = repo.commit_all("base");
    repo.write("a.txt", "1\n2\n3\n4\n");
    let head = repo.commit_all("head");
    let args = crate::args::DiffArgs {
        repo: repo.dir.clone(),
        base: Some(base),
        head,
    };
    assert!(cmd_verify(&args).unwrap());
}

// worktree モードでは、numstat に出ない未追跡ファイルを変更一覧側の
// 想定内の増分として数える。
#[test]
fn verify_counts_untracked_files_into_the_worktree_ledger() {
    let repo = Repo::new();
    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("base");
    repo.write("new.txt", "hello\n");
    let args = crate::args::DiffArgs {
        repo: repo.dir.clone(),
        base: None,
        head: revidere::git::WORKTREE.to_string(),
    };
    assert!(cmd_verify(&args).unwrap());
}

// バイナリファイル（numstat が "-" を返す）は行数の比較対象から除外する。
#[test]
fn verify_does_not_compare_line_counts_for_binary_files() {
    let repo = Repo::new();
    repo.write_bytes("a.bin", &[0, 1, 2]);
    let base = repo.commit_all("base");
    repo.write_bytes("a.bin", &[0, 1, 2, 255, 254, 253]);
    let head = repo.commit_all("binary change");
    let args = crate::args::DiffArgs {
        repo: repo.dir.clone(),
        base: Some(base),
        head,
    };
    assert!(cmd_verify(&args).unwrap());
}

// AI を実際に起こす側。ai.rs の subprocess テストと同じ理由で sh に頼るので
// Unix だけ。
#[cfg(unix)]
mod subprocess {
    use super::*;

    /// sh -c の $0 に "sh" を、$1 以降に extra を渡す。JSON をシェル文字列の
    /// 中へ埋め込むと引用符のエスケープが面倒になるので、位置引数として渡す
    /// （ai.rs の prompt_placeholder テストと同じ形）。
    fn sh_argv(script: &str, extra: &[&str]) -> Vec<String> {
        let mut v = vec![
            "sh".to_string(),
            "-c".to_string(),
            script.to_string(),
            "sh".to_string(),
        ];
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    }

    const ANSWER_TEMPLATE: &str = r#"{"overview":{"problem":"p","change":"c","mechanism":"m","placement":"pl","scope":"s"},"sections":[{sections}],"impacts":[]}"#;

    fn answer(sections: &str) -> String {
        ANSWER_TEMPLATE.replace("{sections}", sections)
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

        let full_coverage = answer(
            r#"{"title":"add line 4","importance":"core","reason":"main change","body":"","ranges":[{"path":"a.txt","side":"new","start":4,"end":4}]}"#,
        );
        let out = repo.dir.join(".revidere").join("review.json");
        let args = crate::args::AnalyzeArgs {
            repo: repo.dir.clone(),
            base: Some(base),
            head,
            out: Some(out),
            config: None,
            ai: Some(sh_argv(
                "cat >/dev/null; printf '%s' \"$1\"",
                &[&full_coverage],
            )),
            timeout: None,
            repair: true,
            cache: true,
        };
        assert!(
            cmd_analyze(&args).unwrap(),
            "汚れた作業ツリーで analyze を止めてはいけない"
        );
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
        // repair() のプロンプトにだけ現れる文言で初回と差し戻しを区別する
        // （prompt.rs の repair() 冒頭「直前の回答では」）。
        let script =
            "if cat | grep -qF '直前の回答では'; then printf '%s' \"$2\"; else printf '%s' \"$1\"; fi";
        let out = repo.dir.join(".revidere").join("review.json");
        let args = crate::args::AnalyzeArgs {
            repo: repo.dir.clone(),
            base: Some(base),
            head,
            out: Some(out.clone()),
            config: None,
            ai: Some(sh_argv(script, &[&initial, &repaired])),
            timeout: None,
            repair: true,
            cache: true,
        };
        let ok = cmd_analyze(&args).unwrap();
        assert!(!ok, "説明なしが残るので説明もれ検査は通らない");
        let saved = Review::from_json(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(
            saved.coverage.unclassified.len(),
            1,
            "悪化した差し戻し後の結果ではなく、最初の結果が残っているはず"
        );
    }

    /// 使い方に書いてある既定値と、実際の既定値。定数は 15 * 60 で書いてあり
    /// 使い方は 900 と直に書いてあるので、片方だけ動かせる。
    #[test]
    fn the_usage_states_the_actual_default_timeout() {
        let secs = config::DEFAULT_TIMEOUT_SECS.to_string();
        assert!(USAGE.contains(&secs), "使い方に {secs} が出ていない");
    }
}
