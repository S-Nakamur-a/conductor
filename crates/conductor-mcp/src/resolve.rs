//! レビュー DB と現在のブランチを、サーバがどこで起動されたかから決める。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const DB_PATH_ENV: &str = "CONDUCTOR_DB_PATH";

/// argv から `--db <path>` / `--db=<path>` を取り出す。
///
/// 空の値は捨てる。`Connection::open("")` はプライベートな一時 DB を開いてしまい、
/// すべてのツールが成功して自分の書き込みも読み戻せるのに、終了時に全部消える。
pub(crate) fn parse_db_arg(args: impl IntoIterator<Item = String>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--db" {
            return it.next().filter(|v| !v.is_empty()).map(PathBuf::from);
        }
        if let Some(rest) = arg.strip_prefix("--db=") {
            return (!rest.is_empty()).then(|| PathBuf::from(rest));
        }
    }
    None
}

/// レビュー DB を探す。
///
/// 1. `--db <path>`
/// 2. `CONDUCTOR_DB_PATH` — 対話セッションへ注入される。マーケットプレイス
///    プラグインが持つ唯一の経路 (.mcp.json は引数を渡さない)
/// 3. cwd の git ルート、次に 4. メイン worktree のルート
pub(crate) fn resolve_db_path(db_arg: Option<PathBuf>) -> Result<PathBuf> {
    resolve_db_path_with(db_arg, std::env::var_os(DB_PATH_ENV).map(PathBuf::from))
}

/// 環境変数の読み取りだけ外に出した [resolve_db_path]。優先順位をプロセスの
/// 環境に触れずにテストできる (`std::env::set_var` は edition 2024 では unsafe)。
fn resolve_db_path_with(db_arg: Option<PathBuf>, env_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = db_arg {
        if path.as_os_str().is_empty() {
            bail!("--db was given an empty path");
        }
        return Ok(path);
    }
    if let Some(from_env) = env_path {
        if from_env.as_os_str().is_empty() {
            bail!("{DB_PATH_ENV} is set but empty");
        }
        return Ok(from_env);
    }

    let cwd = std::env::current_dir().context("failed to read the current directory")?;
    let repo = git2::Repository::discover(&cwd).with_context(|| {
        format!(
            "not inside a git repository ({}) — pass --db or set CONDUCTOR_DB_PATH",
            cwd.display()
        )
    })?;

    if let Some(workdir) = repo.workdir()
        && let Some(found) = existing_db(workdir)
    {
        return Ok(found);
    }
    // リンクされた worktree は自分の .conductor/ を持たない。commondir() は
    // <main>/.git を返すので、その親がメインのルートになる。
    if let Some(main_root) = repo.commondir().parent()
        && let Some(found) = existing_db(main_root)
    {
        return Ok(found);
    }

    bail!(
        "cannot find .conductor/conductor.db from {} — pass --db or set CONDUCTOR_DB_PATH",
        cwd.display()
    )
}

/// 探索経路は候補を「調べる」だけで何も残してはいけない。ファイルが既にある
/// ことを要求するのはそのため — 無いまま開くと空の DB が作られ、TUI には何も
/// 出ていないのに全ツールが成功を報告する。同じ理由で
/// [conductor_core::review_store::db_path] も使わない (.conductor を副作用で作る)。
fn existing_db(root: &Path) -> Option<PathBuf> {
    let candidate = root.join(".conductor").join("conductor.db");
    candidate.is_file().then_some(candidate)
}

/// `root` がチェックアウトしているブランチ。使えるものが無ければ `None`。
///
/// `None` は detached HEAD と「まだ最初のコミットが無い」の両方を指す。どちらも
/// コメントのキーにできない。
pub(crate) fn branch_at(root: &Path) -> Option<String> {
    let repo = git2::Repository::discover(root).ok()?;
    if repo.head_detached().unwrap_or(true) {
        return None;
    }
    repo.head()
        .ok()?
        .shorthand()
        .filter(|name| *name != "HEAD")
        .map(str::to_owned)
}

/// TUI が待ち受ける FIFO。git ルートではなく DB の位置から導く — conductor は
/// 起動されたパスに応じて DB を開くので、リンクされた worktree では git から
/// 辿るメイン worktree と食い違うことがある。
pub(crate) fn refresh_pipe_path(db_path: &Path) -> Option<PathBuf> {
    db_path.parent().map(|dir| dir.join("refresh.pipe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn db引数は両方の綴りを読み値の無い指定を捨てる() {
        let cases: [(&[&str], Option<&str>); 6] = [
            (&["mcp-serve", "--db", "/tmp/a.db"], Some("/tmp/a.db")),
            (&["mcp-serve", "--db=/tmp/b.db"], Some("/tmp/b.db")),
            (&["mcp-serve"], None),
            (&["mcp-serve", "--db"], None),
            (&["mcp-serve", "--db="], None),
            (&["mcp-serve", "--db", ""], None),
        ];
        for (args, want) in cases {
            assert_eq!(
                parse_db_arg(argv(args)),
                want.map(PathBuf::from),
                "{args:?}"
            );
        }
    }

    /// --db が環境変数に勝つのは、ユーザのシェルに残った古い CONDUCTOR_DB_PATH が
    /// パス指定の要求を別の場所へリダイレクトしてはならないため。両方の値を与える
    /// ケースがあるので、どちらの分岐を消してもここが失敗する。
    #[test]
    fn dbパスの優先順位と空値の拒否() {
        let path = |s: &str| Some(PathBuf::from(s));
        let cases: [(Option<PathBuf>, Option<PathBuf>, Option<&str>); 5] = [
            (
                path("/explicit/a.db"),
                path("/env/b.db"),
                Some("/explicit/a.db"),
            ),
            (path("/explicit/a.db"), None, Some("/explicit/a.db")),
            (None, path("/env/b.db"), Some("/env/b.db")),
            (Some(PathBuf::new()), None, None),
            (None, Some(PathBuf::new()), None),
        ];
        for (arg, env, want) in cases {
            let case = format!("arg={arg:?} env={env:?}");
            match want {
                Some(w) => assert_eq!(
                    resolve_db_path_with(arg, env).unwrap(),
                    Path::new(w),
                    "{case}"
                ),
                None => assert!(resolve_db_path_with(arg, env).is_err(), "{case}"),
            }
        }
    }

    /// 見つからないときに空の DB を作る方へフォールスルーすると、マイグレーション
    /// は通り TUI には何も出ないのに全ツールが成功を報告することになる。
    #[test]
    fn 見つからないときは新規作成しない() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join(".conductor").join("conductor.db");

        // tempdir が何かのリポジトリの中にあると探索は成功しうる。その場合でも
        // 返るのは実在するファイルで、候補は作られない。
        if let Ok(found) = resolve_db_path_with(None, None) {
            assert!(found.is_file(), "resolver returned {}", found.display());
        }
        assert!(!probe.exists(), "resolver must not create a database");
    }

    #[test]
    fn リフレッシュ用パイプはdbの隣に置く() {
        assert_eq!(
            refresh_pipe_path(Path::new("/r/.conductor/conductor.db")),
            Some(PathBuf::from("/r/.conductor/refresh.pipe"))
        );
    }

    #[test]
    fn ブランチ名はdetachedと未コミットでnoneになる() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        assert_eq!(branch_at(dir.path()), None, "最初のコミット前");

        let mut index = repo.index().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
        drop(tree);

        // init.defaultBranch を上書きしていない限り "master"。環境に依らせない
        // ため repo 自身の答えと突き合わせる。
        let expected = repo.head().unwrap().shorthand().unwrap().to_string();
        assert_eq!(branch_at(dir.path()), Some(expected));

        let head = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(head).unwrap();
        assert_eq!(branch_at(dir.path()), None, "detached HEAD");
    }
}
