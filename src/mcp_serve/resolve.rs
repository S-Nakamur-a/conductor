//! mcp-serve のために、レビューデータベースと現在のブランチを特定する。
//!
//! どちらの答えも、TUI からではなくサーバがどこで起動されたかから決まる。
//! ヘッドレスの claude セッションは worktree の中で動き、それを自分の cwd
//! として継承するので、ブランチはその worktree がチェックアウトしている
//! ものになる。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// コマンドラインから --db <path> があれば取り出す。
///
/// [resolve_db_path] から分離してあるのは、優先順位のルールをプロセスの
/// 環境変数に触れずにテストできるようにするため。空の値はそのまま渡さず
/// 捨てる。Connection::open("") は *プライベートな一時* データベースを開いて
/// しまうので、--db= を許すとすべてのツールが、成功したように見えて終了時に
/// 消えてしまうスクラッチデータベースを持つことになる — このサブコマンドが
/// 排除しようとしている「動いたように見える失敗」そのものである。
pub(super) fn parse_db_arg(args: impl IntoIterator<Item = String>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--db" {
            return it.next().filter(|v| !v.is_empty()).map(PathBuf::from);
        }
        // --db=<path> の形。通常の CLI の慣習との対称性のため。
        if let Some(rest) = arg.strip_prefix("--db=") {
            return (!rest.is_empty()).then(|| PathBuf::from(rest));
        }
    }
    None
}

/// レビューデータベースを、Node サーバが使っていたのと同じ順序で探す。
/// conductor が起動したケースのために --db を先頭に追加してある。
///
/// 1. --db <path> — spawn_generation が渡すもの
/// 2. CONDUCTOR_DB_PATH — pty_manager::spawn が対話的セッションに注入する
///    もので、マーケットプレイスプラグインが持つ唯一の経路でもある
///    （.mcp.json は引数を一切渡さない）
/// 3. cwd の git ルート、次に 4. *main* worktree のルート
///    （リンクされた worktree で動いているセッションの場合）
///
/// 3 と 4 のステップはファイルがすでに存在していることを要求する。これは
/// 意図的である。Connection::open は放っておくと空のデータベースを作って
/// マイグレーションまで済ませてしまい、TUI には何も表示されていないのに
/// 全てのツールが成功を報告することになる — これはこの変更全体が取り除こう
/// としている、まさに同じ形の静かな失敗である。明示的な --db や
/// CONDUCTOR_DB_PATH はそのまま額面通りに受け取る（呼び出し側はどこに
/// 書き込みたいか分かっているし、新しいリポジトリのデータベースを正当に
/// 作ろうとしている場合もあり得るため）。
/// 対話的セッションがデータベースを取得する際に使う環境変数。
pub(super) const DB_PATH_ENV: &str = "CONDUCTOR_DB_PATH";

pub(super) fn resolve_db_path(db_arg: Option<PathBuf>) -> Result<PathBuf> {
    resolve_db_path_with(db_arg, std::env::var_os(DB_PATH_ENV).map(PathBuf::from))
}

/// [resolve_db_path] から、環境変数の読み取り部分を切り出したもの。
///
/// env の値を引数にしてあるのは、--db との優先順位を直接テストできるように
/// するため。std::env::set_var は edition 2024 では unsafe であり、どのみち
/// 同一プロセス内の他のテストと競合してしまう。
pub(super) fn resolve_db_path_with(
    db_arg: Option<PathBuf>,
    env_path: Option<PathBuf>,
) -> Result<PathBuf> {
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

    // 起動された worktree そのもの。
    if let Some(workdir) = repo.workdir() {
        let candidate = conductor_db(workdir);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // リンクされた worktree は自分の .conductor/ を持たない。データベースは
    // main worktree にある。commondir() は <main>/.git を返すので、その
    // 親が main のルートになる。
    if let Some(main_root) = repo.commondir().parent() {
        let candidate = conductor_db(main_root);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(
        "cannot find .conductor/conductor.db from {} — pass --db or set CONDUCTOR_DB_PATH",
        cwd.display()
    )
}

/// [crate::review_store::db_path] は使わない。あちらは .conductor を副作用で作るが、
/// 上の探索経路は何も残さずに候補を調べられなければならない。
fn conductor_db(root: &Path) -> PathBuf {
    root.join(".conductor").join("conductor.db")
}

/// サーバの cwd がチェックアウトしているブランチ。使えるものが無ければ
/// None。
///
/// None は「まだ HEAD が無い」と「detached HEAD」の両方をカバーする。
/// どちらもコメントやウォークスルーのキーにはできず、ブランチを必要とする
/// ツールはこれを、Node サーバが返していたのと同じ「detached HEAD?」という
/// メッセージに変換する。
pub(super) fn current_branch(repo: &git2::Repository) -> Option<String> {
    if repo.head_detached().unwrap_or(true) {
        return None;
    }
    let head = repo.head().ok()?;
    head.shorthand()
        .filter(|name| *name != "HEAD")
        .map(str::to_owned)
}

/// サーバが動いているリポジトリを開く。
pub(super) fn discover_repo() -> Result<git2::Repository> {
    let cwd = std::env::current_dir().context("failed to read the current directory")?;
    git2::Repository::discover(&cwd)
        .with_context(|| format!("not inside a git repository ({})", cwd.display()))
}

/// TUI が待ち受ける FIFO。データベースの位置から導出する。
///
/// git ルートではなく意図的にデータベースを基準にしている。それが Node
/// サーバのやり方だった（index.ts）し、両者は食い違うことがある —
/// conductor はどのパスで起動されたかに応じてデータベースを開くが、
/// リンクされた worktree のセッションではそれは git のヘルパーが解決する
/// main worktree とは異なる。
pub(super) fn refresh_pipe_path(db_path: &Path) -> Option<PathBuf> {
    db_path.parent().map(|dir| dir.join("refresh.pipe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_db_arg_reads_separate_value() {
        assert_eq!(
            parse_db_arg(args(&["mcp-serve", "--db", "/tmp/a.db"])),
            Some(PathBuf::from("/tmp/a.db"))
        );
    }

    #[test]
    fn parse_db_arg_reads_equals_form() {
        assert_eq!(
            parse_db_arg(args(&["mcp-serve", "--db=/tmp/b.db"])),
            Some(PathBuf::from("/tmp/b.db"))
        );
    }

    #[test]
    fn parse_db_arg_is_none_when_absent() {
        assert_eq!(parse_db_arg(args(&["mcp-serve"])), None);
    }

    #[test]
    fn parse_db_arg_is_none_when_flag_has_no_value() {
        assert_eq!(parse_db_arg(args(&["mcp-serve", "--db"])), None);
    }

    /// Connection::open("") はプライベートな一時データベースを開いてしまう。
    /// すべてのツールが成功し、自分の書き込みを読み戻せてしまうが、終了時に
    /// 全部消えてしまう。
    #[test]
    fn parse_db_arg_rejects_an_empty_value() {
        assert_eq!(parse_db_arg(args(&["mcp-serve", "--db="])), None);
        assert_eq!(parse_db_arg(args(&["mcp-serve", "--db", ""])), None);
    }

    #[test]
    fn resolve_db_path_rejects_an_empty_explicit_path() {
        assert!(resolve_db_path_with(Some(PathBuf::new()), None).is_err());
        assert!(resolve_db_path_with(None, Some(PathBuf::new())).is_err());
    }

    /// --db は環境変数より優先される: ユーザのシェルに残っている古い
    /// CONDUCTOR_DB_PATH が、conductor がパス指定で要求した生成処理を
    /// 別の場所へリダイレクトしてしまってはならない。
    ///
    /// 両方の値が与えられているので、どちらの分岐を消してもこのテストは失敗する。
    #[test]
    fn explicit_db_arg_beats_env() {
        let resolved = resolve_db_path_with(
            Some(PathBuf::from("/explicit/a.db")),
            Some(PathBuf::from("/from-env/b.db")),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("/explicit/a.db"));
    }

    /// --db が無いとき、対話的セッションが頼るのは環境変数の方である —
    /// マーケットプレイスプラグインの .mcp.json は引数を一切渡さないので、
    /// この分岐を失うと TUI 内の全セッションが壊れる。
    #[test]
    fn env_is_used_when_no_db_arg() {
        let resolved = resolve_db_path_with(None, Some(PathBuf::from("/from-env/b.db"))).unwrap();
        assert_eq!(resolved, PathBuf::from("/from-env/b.db"));
    }

    /// どちらの情報源も与えられておらず、探索できる .conductor/conductor.db
    /// も無い場合: これは失敗しなければならない。空のデータベースを作る方に
    /// フォールスルーしてしまうと、マイグレーションは問題なく通り、TUI には
    /// 何も表示されないのに全てのツールが成功を報告することになる。
    #[test]
    fn discovery_failure_is_an_error_not_a_fresh_database() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join(".conductor").join("conductor.db");
        // 健全性チェック: 作られていたはずのパスがまだ存在しないことを確認する。
        assert!(!probe.exists());

        // tempdir がどの git リポジトリのチェックアウトの外にもあるとは限らない
        // — git の探索が失敗する場合に限る。何かのリポジトリに解決された
        // 場合でも、候補データベースは作られてはならない。どちらにせよ
        // テストしている不変条件は同じである。
        if let Ok(found) = resolve_db_path_with(None, None) {
            assert!(
                found.is_file(),
                "resolver returned a path that does not exist: {}",
                found.display()
            );
        }
        assert!(!probe.exists(), "resolver must not create a database");
    }

    #[test]
    fn refresh_pipe_sits_beside_the_database() {
        assert_eq!(
            refresh_pipe_path(Path::new("/r/.conductor/conductor.db")),
            Some(PathBuf::from("/r/.conductor/refresh.pipe"))
        );
    }

    /// git2 がデフォルトでチェックアウトするブランチに、1コミットだけあるリポジトリ。
    fn init_repo_with_commit(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let mut index = repo.index().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
        // Tree は repo を借用しており Drop を実装しているため、NLL が
        // その借用をこのスコープより先に縮めることができない。下で repo を
        // move できるように、明示的に drop する。
        drop(tree);
        repo
    }

    #[test]
    fn current_branch_on_a_normal_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(dir.path());
        // git2::Repository::init は、環境が init.defaultBranch を上書きしない
        // 限り、デフォルトで "master" という名前のブランチになる。
        let expected = repo.head().unwrap().shorthand().unwrap().to_string();

        assert_eq!(current_branch(&repo), Some(expected));
    }

    #[test]
    fn current_branch_is_none_when_head_is_detached() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(dir.path());
        let oid = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(oid).unwrap();

        assert_eq!(current_branch(&repo), None);
    }

    #[test]
    fn current_branch_is_none_before_the_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        // HEAD が指しているのは unborn branch である — detached ではないが、
        // まだ指す先のコミットが存在しないため repo.head() は解決に失敗する。
        assert_eq!(current_branch(&repo), None);
    }
}
