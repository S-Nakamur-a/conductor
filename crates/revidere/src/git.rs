// git の呼び出し。接続点はここ 1 箇所で、これ以外にリポジトリの前提を持たない。

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

/// git の起動。パスの出方を揃えるのがここの仕事でもある。
///
/// core.quotepath の既定は true で、非 ASCII のパスを "a/\343\201\202.txt" の
/// ように 8 進エスケープ付きで囲んで出す。diff だけがそうなり、-z を付けた
/// ls-files は生のまま出すので、放っておくと同じファイルが 2 通りの名前で現れる。
/// false に固定して生で受ける。
fn git(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.args(["-c", "core.quotepath=false", "-C"]).arg(repo);
    c
}

fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = git(repo)
        .args(args)
        .output()
        .map_err(|e| GitError(format!("git を起動できない: {e}")))?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git {} が失敗した: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// リポジトリのルート。渡されたパスがサブディレクトリでもよい。
pub fn root(path: &Path) -> Result<PathBuf, GitError> {
    let s = run(path, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(s.trim()))
}

/// 短い OID に解決する。表示と、成果物の base/head に使う。
pub fn short_oid(repo: &Path, rev: &str) -> Result<String, GitError> {
    Ok(run(repo, &["rev-parse", "--short", rev])?
        .trim()
        .to_string())
}

/// ベースを推定する。origin/HEAD が指す先、無ければ main、無ければ master。
pub fn guess_base(repo: &Path) -> Result<String, GitError> {
    if let Ok(s) = run(
        repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) {
        let s = s.trim();
        if let Some(name) = s.strip_prefix("refs/remotes/") {
            return Ok(name.to_string());
        }
    }
    for cand in ["main", "master"] {
        if run(repo, &["rev-parse", "--verify", "--quiet", cand]).is_ok() {
            return Ok(cand.to_string());
        }
    }
    Err(GitError(
        "ベースを推定できない。--base で指定してほしい".to_string(),
    ))
}

/// base と HEAD の共通祖先。レビューの起点。
///
/// ベース側の進みを差分に混ぜないための解決で、`git diff base...HEAD` の
/// base 側にあたる。先に 1 つのコミットへ潰しておくと、そこから作業ツリーまでの
/// 2 点指定で「ベース以降にこのブランチでしたこと全部」が 1 枚に収まる。
pub fn merge_base(repo: &Path, base: &str) -> Result<String, GitError> {
    Ok(run(repo, &["merge-base", base, "HEAD"])?.trim().to_string())
}

/// レビュー対象の diff。`from` から現在の作業ツリーまで。
///
/// 終点をコミットではなく作業ツリーにするのは、レビューしたいものが大抵まだ
/// コミットされていないため。起点が merge-base なので、コミット済みの変更と
/// 手元の変更が 1 枚の差分に収まる。
///
/// 未追跡ファイルは `git diff` に出ないので、1 件ずつ `--no-index` で
/// 追加ファイルの差分として起こして繋ぐ。`git add -N` を使えば 1 回で済むが、
/// それは相手の index を書き換えるので採らない。
///
/// 文脈行はモデルには不要（自分でファイルを読む）だが、
/// 変更一覧の行番号を正しく数えるには必須なので既定の 3 行を保つ。
pub fn diff(repo: &Path, from: &str) -> Result<String, GitError> {
    let mut out = run(
        repo,
        &[
            "diff",
            // rename を rename として出す。出さないと削除＋追加に化けて
            // 変更箇所が水増しされる。
            "--find-renames",
            "--no-color",
            "--no-ext-diff",
            from,
        ],
    )?;
    for path in untracked(repo)? {
        // --no-index は差分があると終了コード 1 を返すので、成否で判定しない。
        let o = git(repo)
            .args([
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-index",
                "/dev/null",
            ])
            .arg(&path)
            .output()
            .map_err(|e| GitError(format!("git を起動できない: {e}")))?;
        let text = String::from_utf8_lossy(&o.stdout);
        if !text.trim().is_empty() {
            out.push_str(&text);
        }
    }
    Ok(out)
}

/// 未追跡ファイルの一覧（.gitignore で無視されているものは含まない）。
pub fn untracked(repo: &Path) -> Result<Vec<String>, GitError> {
    let out = run(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    Ok(out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect())
}

/// rev が今の HEAD から辿れるか。
///
/// 辿れないのは、そのコミットが履歴から消えたということ (rebase / amend /
/// force push、あるいは古いコミットへの巻き戻し)。前回のレビューの続きとして
/// 差分を語ってよいかの判断に使う。
pub fn is_ancestor_of_head(repo: &Path, rev: &str) -> bool {
    run(repo, &["merge-base", "--is-ancestor", rev, "HEAD"]).is_ok()
}

/// from から作業ツリーまでで変わったファイル。
///
/// [diff] と同じ範囲を名前だけで見る。未追跡ファイルは `git diff` に出ないので
/// こちらでも繋ぐ — 新しく足したファイルが「変わっていない」側に落ちると、
/// 前回からの進みとして一番読みたいものが消える。
pub fn changed_files(repo: &Path, from: &str) -> Result<Vec<String>, GitError> {
    let mut files: Vec<String> = run(repo, &["diff", "--find-renames", "--name-only", from])?
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect();
    files.extend(untracked(repo)?);
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// テストごとに使い捨てる、実物の git リポジトリ。
    ///
    /// この関数群は git の呼び方そのものが正しさなので、モックではなく
    /// 実際の git を子プロセスとして動かして確かめる。
    struct Repo {
        dir: PathBuf,
    }

    static SEQ: AtomicU32 = AtomicU32::new(0);

    impl Repo {
        fn new() -> Self {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("revidere-git-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Repo { dir };
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.email", "t@example.com"]);
            repo.git(&["config", "user.name", "t"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
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

        fn commit_all(&self, msg: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", msg]);
        }

        fn head(&self) -> String {
            self.git(&["rev-parse", "HEAD"]).trim().to_string()
        }

        /// レビューの起点。実際の呼ばれ方どおり merge-base を経由する。
        fn from(&self, base: &str) -> String {
            merge_base(&self.dir, base).unwrap()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_range_starts_at_the_merge_base_so_the_bases_own_progress_stays_out() {
        let r = Repo::new();
        r.write("a.txt", "1\n2\n3\n");
        r.commit_all("base");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.write("a.txt", "1\n2\n3\nfeature\n");
        r.commit_all("feature change");
        // ベース側もそのあと進める。共通祖先ではなく main の先端を起点にすると、
        // この進みまで差分に混ざる。
        r.git(&["checkout", "-q", "main"]);
        r.write("b.txt", "main only\n");
        r.commit_all("main moved on");
        r.git(&["checkout", "-q", "feature"]);

        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(out.contains("feature"), "{out}");
        assert!(!out.contains("main only"), "{out}");
    }

    /// ベースからの全差分になっていること。ここが「最後のコミット以降」だけに
    /// なると、レビューは PR 全体ではなく直近の一手しか映さなくなる。
    #[test]
    fn the_range_covers_every_commit_since_the_base_not_only_the_latest_one() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("base");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.write("first.txt", "first commit\n");
        r.commit_all("first");
        r.write("second.txt", "second commit\n");
        r.commit_all("second");
        // さらに未コミットの手元の変更。
        r.write("third.txt", "not committed yet\n");

        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(
            out.contains("first commit"),
            "1 つ目のコミットが無い: {out}"
        );
        assert!(
            out.contains("second commit"),
            "2 つ目のコミットが無い: {out}"
        );
        assert!(out.contains("not committed yet"), "手元の変更が無い: {out}");
    }

    /// 履歴が書き換わった (rebase / amend / force push) ときも、起点は
    /// 今の HEAD とベースから引き直される。
    #[test]
    fn the_range_is_rebuilt_from_the_current_head_after_the_history_is_rewritten() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("base");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.write("old.txt", "abandoned work\n");
        r.commit_all("work that will be dropped");
        let dropped = r.head();

        // 履歴ごと差し替える。前のコミットはもう辿れない。
        r.git(&["reset", "-q", "--hard", "main"]);
        r.write("new.txt", "rewritten work\n");
        r.commit_all("rewritten");

        assert!(!is_ancestor_of_head(&r.dir, &dropped));
        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(out.contains("rewritten work"), "{out}");
        assert!(
            !out.contains("abandoned work"),
            "捨てたはずの変更が残っている: {out}"
        );
    }

    #[test]
    fn changed_files_lists_both_new_commits_and_untracked_files() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("base");
        let previous = r.head();
        r.write("committed.txt", "x\n");
        r.commit_all("later commit");
        r.write("untracked.txt", "y\n");

        assert_eq!(
            changed_files(&r.dir, &previous).unwrap(),
            vec!["committed.txt".to_string(), "untracked.txt".to_string()]
        );
    }

    #[test]
    fn diff_keeps_the_default_three_lines_of_context() {
        let r = Repo::new();
        let base: String = (1..=10).map(|n| format!("{n}\n")).collect();
        r.write("a.txt", &base);
        r.commit_all("base");
        r.git(&["checkout", "-q", "-b", "feature"]);
        let changed: String = (1..=10)
            .map(|n| {
                if n == 5 {
                    "X\n".to_string()
                } else {
                    format!("{n}\n")
                }
            })
            .collect();
        r.write("a.txt", &changed);
        r.commit_all("change line 5");

        // 変更は 5 行目 1 つだけ。既定の文脈 3 行なら、ハンクは
        // 2〜8 行目（7 行）を覆う。
        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(out.contains("@@ -2,7 +2,7 @@"), "{out}");
    }

    #[test]
    fn find_renames_reports_a_pure_rename_instead_of_delete_and_add() {
        let r = Repo::new();
        let content: String = (1..=20).map(|n| format!("line{n}\n")).collect();
        r.write("a.rs", &content);
        r.commit_all("add a.rs");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.git(&["mv", "a.rs", "b.rs"]);
        r.commit_all("rename a.rs to b.rs");

        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(out.contains("rename from a.rs"), "{out}");
        assert!(out.contains("rename to b.rs"), "{out}");
    }

    #[test]
    fn the_diff_stitches_untracked_files_into_the_committed_diff() {
        let r = Repo::new();
        r.write("a.txt", "1\n2\n3\n");
        r.commit_all("base");
        // コミット済みファイルへの未コミットの変更。
        r.write("a.txt", "1\n2\nX\n");
        // 未追跡ファイル。`git diff` にはこれが出ない。
        r.write("new.txt", "brand new\n");

        // 差分がある未追跡ファイルは --no-index の終了コードが 1 になる。
        // それをエラー扱いしていたら、この呼び出し自体が失敗する。
        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(out.contains("-3") && out.contains("+X"), "{out}");
        assert!(
            out.contains("new.txt") && out.contains("brand new"),
            "{out}"
        );
    }

    /// 非 ASCII のパスが、そのままの名前で diff に出る。
    ///
    /// core.quotepath の既定のままだと "a/\343\201\202.txt" というエスケープ付きの
    /// 別名で出て、変更一覧のパスが実在しない文字列になる。
    #[test]
    fn a_non_ascii_path_is_not_octal_escaped_in_the_diff() {
        let r = Repo::new();
        r.write("あ.txt", "1\n");
        r.commit_all("base");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.write("あ.txt", "2\n");
        r.commit_all("change");

        let out = diff(&r.dir, &r.from("main")).unwrap();
        assert!(out.contains("diff --git a/あ.txt b/あ.txt"), "{out}");
        assert!(!out.contains("\\343"), "8 進エスケープが残っている: {out}");
    }

    #[test]
    fn guess_base_prefers_main_when_no_origin_head_is_set() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("init");
        assert_eq!(guess_base(&r.dir).unwrap(), "main");
    }

    #[test]
    fn guess_base_falls_back_to_master_when_main_is_absent() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("init");
        r.git(&["branch", "-m", "main", "master"]);
        assert_eq!(guess_base(&r.dir).unwrap(), "master");
    }

    #[test]
    fn guess_base_errors_rather_than_guessing_when_nothing_matches() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("init");
        r.git(&["branch", "-m", "main", "trunk"]);
        assert!(guess_base(&r.dir).is_err());
    }

    #[test]
    fn guess_base_follows_origin_head_over_main_and_master() {
        let r = Repo::new();
        r.write("a.txt", "1\n");
        r.commit_all("init");
        // 本物の remote は要らない。symbolic-ref の参照先は検証せずそのまま
        // 返すだけなので、参照だけ用意すれば足りる。
        r.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ]);
        assert_eq!(guess_base(&r.dir).unwrap(), "origin/develop");
    }

    #[test]
    fn root_resolves_from_a_subdirectory() {
        let r = Repo::new();
        r.write("sub/dir/a.txt", "x\n");
        r.commit_all("init");
        let got = root(&r.dir.join("sub/dir")).unwrap();
        assert_eq!(got.canonicalize().unwrap(), r.dir.canonicalize().unwrap());
    }
}
