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

/// core.quotepath の既定は true で、非 ASCII のパスを "a/\343\201\202.txt" の
/// ように 8 進エスケープ付きで出す。そうなるのは diff だけで、-z を付けた
/// ls-files は生のまま出すので、放っておくと同じファイルが 2 通りの名前で現れる。
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
/// `git diff base...HEAD` の base 側にあたる。先に 1 つのコミットへ潰して
/// おくと、そこから作業ツリーまでの 2 点指定で「ベース以降にこのブランチで
/// したこと全部」が 1 枚に収まる。
pub fn merge_base(repo: &Path, base: &str) -> Result<String, GitError> {
    Ok(run(repo, &["merge-base", base, "HEAD"])?.trim().to_string())
}

/// レビュー対象の diff。`from` から現在の作業ツリーまで。
///
/// 終点をコミットではなく作業ツリーにするのは、レビューしたいものが大抵まだ
/// コミットされていないため。
///
/// 未追跡ファイルは `git diff` に出ないので、1 件ずつ `--no-index` で起こして
/// 繋ぐ。`git add -N` なら 1 回で済むが、相手の index を書き換えるので採らない。
///
/// 文脈行はモデルには不要（自分でファイルを読む）だが、変更一覧の行番号を
/// 正しく数えるには必須なので既定の 3 行を保つ。
pub fn diff(repo: &Path, from: &str) -> Result<String, GitError> {
    let mut out = run(
        repo,
        &[
            "diff",
            // 出さないと削除＋追加に化けて変更箇所が水増しされる。
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

/// rev が今の HEAD から辿れるか。辿れないのは、そのコミットが履歴から消えた
/// ということ (rebase / amend / force push、あるいは巻き戻し)。
pub fn is_ancestor_of_head(repo: &Path, rev: &str) -> bool {
    run(repo, &["merge-base", "--is-ancestor", rev, "HEAD"]).is_ok()
}

/// [diff] と同じ範囲を名前だけで見る。未追跡をこちらでも繋ぐのは、新しく
/// 足したファイルが「変わっていない」側に落ちると、前回からの進みとして
/// 一番読みたいものが消えるから。
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
    use revidere_fixtures::Repo;

    fn repo() -> Repo {
        Repo::new("git-test")
    }

    fn diff_from(r: &Repo, base: &str) -> String {
        diff(r.dir(), &r.merge_base(base)).unwrap()
    }

    #[test]
    fn 区間はmerge_baseから始まりベース側の進みは入らない() {
        let r = repo();
        r.write("a.txt", "1\n2\n3\n");
        r.commit_all("base");
        r.branch("feature");
        r.write("a.txt", "1\n2\n3\nfeature\n");
        r.commit_all("feature change");
        // 共通祖先ではなく main の先端を起点にすると、この進みまで差分に混ざる。
        r.git(&["checkout", "-q", "main"]);
        r.write("b.txt", "main only\n");
        r.commit_all("main moved on");
        r.git(&["checkout", "-q", "feature"]);

        let out = diff_from(&r, "main");
        assert!(out.contains("feature"), "{out}");
        assert!(!out.contains("main only"), "{out}");
    }

    /// ここが「最後のコミット以降」だけになると、レビューは PR 全体ではなく
    /// 直近の一手しか映さなくなる。
    #[test]
    fn 区間はベース以降の全コミットと未コミット分を含む() {
        let r = repo();
        r.write("a.txt", "1\n");
        r.commit_all("base");
        r.branch("feature");
        r.write("first.txt", "first commit\n");
        r.commit_all("first");
        r.write("second.txt", "second commit\n");
        r.commit_all("second");
        r.write("third.txt", "not committed yet\n");

        let out = diff_from(&r, "main");
        for want in ["first commit", "second commit", "not committed yet"] {
            assert!(out.contains(want), "{want} が無い: {out}");
        }
    }

    #[test]
    fn 履歴が書き換わっても今のheadから区間を組み直す() {
        let r = repo();
        r.write("a.txt", "1\n");
        r.commit_all("base");
        r.branch("feature");
        r.write("old.txt", "abandoned work\n");
        r.commit_all("work that will be dropped");
        let dropped = r.head();

        r.git(&["reset", "-q", "--hard", "main"]);
        r.write("new.txt", "rewritten work\n");
        r.commit_all("rewritten");

        assert!(!is_ancestor_of_head(r.dir(), &dropped));
        let out = diff_from(&r, "main");
        assert!(out.contains("rewritten work"), "{out}");
        assert!(
            !out.contains("abandoned work"),
            "捨てたはずの変更が残っている: {out}"
        );
    }

    #[test]
    fn 変わったファイルは新しいコミットも未追跡も並べる() {
        let r = repo();
        r.write("a.txt", "1\n");
        r.commit_all("base");
        let previous = r.head();
        r.write("committed.txt", "x\n");
        r.commit_all("later commit");
        r.write("untracked.txt", "y\n");

        assert_eq!(
            changed_files(r.dir(), &previous).unwrap(),
            vec!["committed.txt".to_string(), "untracked.txt".to_string()]
        );
    }

    #[test]
    fn diffは既定の3行の文脈を保つ() {
        let r = repo();
        let base: String = (1..=10).map(|n| format!("{n}\n")).collect();
        r.write("a.txt", &base);
        r.commit_all("base");
        r.branch("feature");
        let changed = base.replace("5\n", "X\n");
        r.write("a.txt", &changed);
        r.commit_all("change line 5");

        // 変更は 5 行目だけ。文脈 3 行なら、ハンクは 2〜8 行目を覆う。
        let out = diff_from(&r, "main");
        assert!(out.contains("@@ -2,7 +2,7 @@"), "{out}");
    }

    #[test]
    fn 純粋なリネームは削除と追加ではなくリネームとして出す() {
        let r = repo();
        let content: String = (1..=20).map(|n| format!("line{n}\n")).collect();
        r.write("a.rs", &content);
        r.commit_all("add a.rs");
        r.branch("feature");
        r.git(&["mv", "a.rs", "b.rs"]);
        r.commit_all("rename a.rs to b.rs");

        let out = diff_from(&r, "main");
        assert!(out.contains("rename from a.rs"), "{out}");
        assert!(out.contains("rename to b.rs"), "{out}");
    }

    /// 差分がある未追跡ファイルは --no-index の終了コードが 1 になる。それを
    /// エラー扱いしていたら、この呼び出し自体が失敗する。
    #[test]
    fn 未追跡ファイルもコミット済みのdiffに繋ぎ込む() {
        let r = repo();
        r.write("a.txt", "1\n2\n3\n");
        r.commit_all("base");
        r.write("a.txt", "1\n2\nX\n");
        r.write("new.txt", "brand new\n");

        let out = diff_from(&r, "main");
        assert!(out.contains("-3") && out.contains("+X"), "{out}");
        assert!(
            out.contains("new.txt") && out.contains("brand new"),
            "{out}"
        );
    }

    /// core.quotepath の既定のままだと "a/\343\201\202.txt" というエスケープ付きの
    /// 別名で出て、変更一覧のパスが実在しない文字列になる。
    #[test]
    fn 非asciiのパスは8進エスケープされずに出る() {
        let r = repo();
        r.write("あ.txt", "1\n");
        r.commit_all("base");
        r.branch("feature");
        r.write("あ.txt", "2\n");
        r.commit_all("change");

        let out = diff_from(&r, "main");
        assert!(out.contains("diff --git a/あ.txt b/あ.txt"), "{out}");
        assert!(!out.contains("\\343"), "8 進エスケープが残っている: {out}");
    }

    #[test]
    fn ベースの推定はorigin_headからmainmasterの順に落ちる() {
        // 本物の remote は要らない。symbolic-ref の参照先は検証せずそのまま
        // 返すので、参照だけ用意すれば足りる。
        let point_origin_head: &[&str] = &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ];
        for (name, setup, want) in [
            (
                "origin/HEAD は main より優先",
                point_origin_head,
                Some("origin/develop"),
            ),
            ("origin/HEAD が無ければ main", &[][..], Some("main")),
            (
                "main が無ければ master",
                &["branch", "-m", "main", "master"][..],
                Some("master"),
            ),
            (
                "どれも無ければ推測しない",
                &["branch", "-m", "main", "trunk"][..],
                None,
            ),
        ] {
            let r = repo();
            r.write("a.txt", "1\n");
            r.commit_all("init");
            if !setup.is_empty() {
                r.git(setup);
            }
            assert_eq!(guess_base(r.dir()).ok().as_deref(), want, "{name}");
        }
    }

    #[test]
    fn サブディレクトリからでもルートを引ける() {
        let r = repo();
        r.write("sub/dir/a.txt", "x\n");
        r.commit_all("init");
        let got = root(&r.dir().join("sub/dir")).unwrap();
        assert_eq!(got.canonicalize().unwrap(), r.dir().canonicalize().unwrap());
    }
}
