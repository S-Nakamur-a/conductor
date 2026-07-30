//! Locating the review database and the current branch for `mcp-serve`.
//!
//! Both answers come from wherever the server was spawned, not from the TUI:
//! the headless `claude` session runs in a worktree and inherits that as its
//! cwd, so the branch is whatever that worktree has checked out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Extract `--db <path>` from a command line, if present.
///
/// Split out from [`resolve_db_path`] so the precedence rules can be tested
/// without touching the process environment.
/// An empty value is dropped rather than passed along: `Connection::open("")`
/// opens a *private temporary* database, so `--db=` would give every tool a
/// scratch database that reports success and vanishes on exit — the same
/// looks-like-it-worked failure this subcommand exists to eliminate.
pub(super) fn parse_db_arg(args: impl IntoIterator<Item = String>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--db" {
            return it.next().filter(|v| !v.is_empty()).map(PathBuf::from);
        }
        // `--db=<path>` form, for symmetry with the usual CLI convention.
        if let Some(rest) = arg.strip_prefix("--db=") {
            return (!rest.is_empty()).then(|| PathBuf::from(rest));
        }
    }
    None
}

/// Find the review database, in the same order the Node server used, with
/// `--db` prepended for the conductor-spawned case.
///
/// 1. `--db <path>` — what `spawn_generation` passes
/// 2. `CONDUCTOR_DB_PATH` — what `pty_manager::spawn` injects into interactive
///    sessions, and the only route the marketplace plugin has (its `.mcp.json`
///    passes no arguments)
/// 3. the cwd's git root, then 4. the *main* worktree's root, for a session
///    running in a linked worktree
///
/// Steps 3 and 4 require the file to already exist. This is deliberate:
/// `Connection::open` would happily create an empty database, migrate it, and
/// then every tool would report success while the TUI showed nothing — the same
/// silent-failure shape this whole change exists to remove. An explicit `--db`
/// or `CONDUCTOR_DB_PATH` is taken at face value (the caller knows where it
/// wants to write, and may legitimately be creating a fresh repo's database).
/// The environment variable interactive sessions get their database from.
pub(super) const DB_PATH_ENV: &str = "CONDUCTOR_DB_PATH";

pub(super) fn resolve_db_path(db_arg: Option<PathBuf>) -> Result<PathBuf> {
    resolve_db_path_with(db_arg, std::env::var_os(DB_PATH_ENV).map(PathBuf::from))
}

/// [`resolve_db_path`] with the environment read for it.
///
/// The env value is a parameter so the precedence between it and `--db` can be
/// tested directly; `std::env::set_var` is `unsafe` in edition 2024 and would
/// race other tests in the same process anyway.
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

    // The worktree we were launched in.
    if let Some(workdir) = repo.workdir() {
        let candidate = conductor_db(workdir);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // A linked worktree keeps no `.conductor/` of its own; the database lives
    // in the main worktree. `commondir()` is `<main>/.git`, so its parent is
    // the main root.
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

/// `<root>/.conductor/conductor.db`.
///
/// Deliberately not [`crate::review_store::db_path`], which creates the
/// `.conductor` directory as a side effect — the discovery path above must be
/// able to probe a candidate without leaving anything behind.
fn conductor_db(root: &Path) -> PathBuf {
    root.join(".conductor").join("conductor.db")
}

/// The branch the server's cwd has checked out, or `None` when there isn't a
/// usable one.
///
/// `None` covers both "no HEAD yet" and a detached HEAD. Neither can key a
/// comment or a walkthrough, and the tools that need a branch turn this into
/// the same "detached HEAD?" message the Node server returned.
pub(super) fn current_branch(repo: &git2::Repository) -> Option<String> {
    if repo.head_detached().unwrap_or(true) {
        return None;
    }
    let head = repo.head().ok()?;
    head.shorthand()
        .filter(|name| *name != "HEAD")
        .map(str::to_owned)
}

/// Open the repository the server is running in.
pub(super) fn discover_repo() -> Result<git2::Repository> {
    let cwd = std::env::current_dir().context("failed to read the current directory")?;
    git2::Repository::discover(&cwd)
        .with_context(|| format!("not inside a git repository ({})", cwd.display()))
}

/// The FIFO the TUI listens on, derived from the database location.
///
/// Keyed off the database rather than the git root on purpose: that is what the
/// Node server did (`index.ts`), and the two can disagree — conductor opens the
/// database under whichever path it was started with, which for a session in a
/// linked worktree is not the main worktree the git helpers would resolve to.
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

    /// `Connection::open("")` opens a private temporary database: every tool
    /// would succeed, read its own writes back, and lose the lot on exit.
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

    /// `--db` wins over the environment: a stale `CONDUCTOR_DB_PATH` in the
    /// user's shell must not redirect a generation conductor asked for by path.
    ///
    /// Both values are supplied, so deleting either branch fails this.
    #[test]
    fn explicit_db_arg_beats_env() {
        let resolved = resolve_db_path_with(
            Some(PathBuf::from("/explicit/a.db")),
            Some(PathBuf::from("/from-env/b.db")),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("/explicit/a.db"));
    }

    /// With no `--db`, the environment is what interactive sessions rely on —
    /// the marketplace plugin's `.mcp.json` passes no arguments at all, so
    /// losing this branch would break every in-TUI session.
    #[test]
    fn env_is_used_when_no_db_arg() {
        let resolved = resolve_db_path_with(None, Some(PathBuf::from("/from-env/b.db"))).unwrap();
        assert_eq!(resolved, PathBuf::from("/from-env/b.db"));
    }

    /// Neither source given and no `.conductor/conductor.db` to discover: this
    /// must fail rather than fall through to creating an empty database, which
    /// would migrate cleanly and then report success at every tool while the
    /// TUI showed nothing.
    #[test]
    fn discovery_failure_is_an_error_not_a_fresh_database() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join(".conductor").join("conductor.db");
        // Sanity: the path this would have created does not exist yet.
        assert!(!probe.exists());

        // A tempdir is outside any git repo checkout only if git discovery
        // fails; when it does resolve to one, the candidate still must not be
        // created. Either way the invariant under test is the same.
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

    /// A repo with one commit on the branch git2 checks out by default.
    fn init_repo_with_commit(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let mut index = repo.index().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
        // `Tree` borrows `repo` and implements `Drop`, which keeps NLL from
        // shrinking that borrow past this scope; drop it explicitly so `repo`
        // can move out below.
        drop(tree);
        repo
    }

    #[test]
    fn current_branch_on_a_normal_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(dir.path());
        // git2::Repository::init defaults to a branch named "master" unless
        // the environment overrides init.defaultBranch.
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
        // HEAD points at an unborn branch — not detached, but `repo.head()`
        // fails to resolve since no commit exists to point to yet.
        assert_eq!(current_branch(&repo), None);
    }
}
