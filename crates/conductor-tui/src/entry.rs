//! conductor の起動と終了。
//!
//! 順番に意味がある — CLI の即答フラグは端末に触る前に返さねばならず (mcp-serve は
//! stdout で JSON-RPC を話す)、端末ケイパビリティの問い合わせは raw mode に入った
//! あと・ループが stdin を読み始める前でなければならない。

use std::io;
use std::path::{Path, PathBuf};

use crate::workspace::{RepoState, StatusLevel, StatusMessage, Workspace};
use crate::{run, term};
use anyhow::Result;
use conductor_core::config::Config;
use conductor_core::git_engine::GitEngine;
use conductor_core::keymap::KeyMap;
use conductor_core::theme::Theme;
use conductor_core::{cc_hook, config, instance_lock, semantic_index, term_caps};
use conductor_svc::Services;
use conductor_svc::watch::{ConfigWatcher, RefreshPipe, WatchEvent};
use crossterm::execute;
use crossterm::terminal::SetTitle;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// 端末を借りて画面を回し、抜けるときは必ず返す。
///
/// version は root の Cargo.toml のもの。conductor-tui 自身の版は 0.1.0 なので、ここを
/// env!("CARGO_PKG_VERSION") に畳むと更新チェックが常に「新版あり」になる。
pub fn run(version: &'static str) -> Result<()> {
    term::install_panic_hook();
    env_logger::init();

    if let Some(result) = cli_fast_path(version) {
        return result;
    }

    let options = Options::parse(std::env::args().skip(1))?;

    // 端末に触る前に。断られたら通常スクリーンのまま理由を出して終わりたい。
    let repo_root = worktree_root(&arg_path(options.repo)?);
    let _window_lock = if options.second_window {
        None
    } else {
        acquire_window_lock()?
    };
    // 窓のロックを知らない古い版の窓とは、リポジトリのロックで今までどおり取り合う。
    let _lock = if options.second_window {
        None
    } else {
        match instance_lock::acquire(&repo_root) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => anyhow::bail!(
                "conductor is already open on this repository:\n  {}\n\n\
                 All worktrees of a repository share one window.\n\
                 Switch to that window, close it first, or open a second one\n\
                 for verification with --second-window.",
                instance_lock::locked_repo_root(&repo_root).display()
            ),
            // 排他できないことを理由にリポジトリを開けなくするほうが害が大きい。
            Err(e) => {
                log::warn!("could not take the single-instance lock: {e:#}");
                None
            }
        }
    };

    let mut ws = workspace(repo_root, version);
    if options.second_window {
        ws.panels.terminal.leave_session_hooks_alone();
    }
    let mut svc = Services::new();
    // リポジトリにつき 1 つしかない口は、2 つ目のウィンドウでは先客に譲る。FIFO は
    // 作り直しが先客の読み口を消し、ソケットは宛先を奪う。どちらも黙って壊れる。
    let repo_channels = !options.second_window;
    // Claude Code のフックが送る active/waiting とセッション id もホストが受けて中継する。
    if repo_channels {
        attach_pty_host(&mut ws, &svc, version);
    }
    // MCP がレビュー DB を書いたら、この FIFO 越しに読み直しを促してくる。
    let _refresh = if repo_channels {
        started(
            "refresh pipe",
            RefreshPipe::new(&ws.repo.root, svc.sender()),
        )
    } else {
        None
    };
    // 設定ファイルを外部のエディタで書き換えたら、外観をその場で入れ替える。
    let _config_watch = started(
        "config watcher",
        ConfigWatcher::new(&config::config_file_path(), svc.sender()),
    );
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let modes = term::enter(terminal.backend_mut())?;
    let _ = execute!(
        io::stdout(),
        SetTitle(format!("conductor - {}", ws.repo.name))
    );

    apply_auto_theme(&mut ws);
    apply_auto_icons(&mut ws);

    // 更新は走っているファイルを rename で置き換えるので、済んだあとの current_exe は
    // 消えた inode を指す。再起動に使うパスと引数はここで捕まえておく。
    let relaunch_exe = std::env::current_exe();
    let relaunch_args: Vec<String> = std::env::args().skip(1).collect();

    let result = run::run(&mut terminal, &mut ws, &mut svc);
    ws.panels.revidere.abort();

    // 復旧はエラー時も必ず。1 つ失敗しても残りを試す。
    let _ = term::leave(terminal.backend_mut(), modes);
    let _ = execute!(terminal.backend_mut(), SetTitle(""));
    let _ = terminal.show_cursor();
    if ws.relaunch
        && let Ok(exe) = relaunch_exe
    {
        relaunch(&exe, &relaunch_args);
    }
    result
}

/// ほかのリポジトリのホストもこの窓がつなぐので、2 つ目の窓が開くとホストを取り合う。
fn acquire_window_lock() -> Result<Option<instance_lock::InstanceLock>> {
    let lock = cc_hook::ensure_runtime_dir().and_then(|dir| instance_lock::acquire_window(&dir));
    match lock {
        Ok(Some(lock)) => Ok(Some(lock)),
        Ok(None) => anyhow::bail!(
            "conductor is already open in another window.\n\n\
             One window shows the sessions of every repository.\n\
             Switch to that window, close it first, or open a second one\n\
             for verification with --second-window."
        ),
        // 排他できないことを理由に開けなくするほうが害が大きい。
        Err(e) => {
            log::warn!("could not take the single-window lock: {e:#}");
            Ok(None)
        }
    }
}

/// 前のウィンドウから残っているセッションに戻る。自分のリポジトリのホストが居なければここで起こし、
/// ほかのリポジトリのホストは居るものにだけつなぐ。
fn attach_pty_host<P: Send + 'static>(ws: &mut Workspace, svc: &Services<P>, version: &str) {
    let sender = svc.sender();
    let notify = move |n: cc_hook::Notification| match n {
        cc_hook::Notification::Ui { .. } => sender.send_watch(WatchEvent::from(n)),
        n => sender.send_watch_unscoped(WatchEvent::from(n)),
    };
    let mut warnings = Vec::new();
    match ws
        .panels
        .terminal
        .attach_host(&ws.repo.root, version, notify)
    {
        Ok(attached) if attached.host_version == version => {}
        Ok(attached) => warnings.push(format!(
            "The PTY host is still v{}; it is replaced at the next start after its sessions have all ended",
            attached.host_version
        )),
        Err(e) => warnings.push(format!(
            "PTY host unavailable, sessions will end with this window: {e:#}"
        )),
    }
    let skipped = match cc_hook::ensure_runtime_dir() {
        Ok(dir) => ws
            .panels
            .terminal
            .attach_other_hosts(&cc_hook::pty_host_sockets(&dir)),
        Err(e) => {
            log::warn!("could not list the other PTY hosts: {e:#}");
            0
        }
    };
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} other PTY host(s) not attached: another conductor window holds them or they did not answer"
        ));
    }
    if warnings.is_empty() {
        return;
    }
    // ステータスは 1 枠なので、別々に出すと後勝ちで消える。
    let text = warnings.join(" · ");
    log::warn!("{text}");
    ws.chrome.status = Some(StatusMessage {
        level: StatusLevel::Warning,
        text,
        shown_at: std::time::Instant::now(),
    });
}

/// 同じ引数で自分自身を exec し直す (戻らない)。プロセスイメージを置き換えるので、
/// 永続化が必要なものはすべて呼ぶ前に済ませておくこと。
fn relaunch(exe: &Path, args: &[String]) -> ! {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(exe).args(args).exec();
    eprintln!("Failed to restart: {err}");
    std::process::exit(1);
}

fn workspace(root: PathBuf, version: &'static str) -> Workspace {
    let (config, config_warning) = match Config::load() {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(format!("config: {e}"))),
    };
    let (keymap, keybind_warnings) = KeyMap::with_warnings(&config.keybinds);
    let theme = Theme::from_name(config.theme_name());
    let repo = repo_state(root, &config);

    let mut ws = Workspace::new(repo, config, keymap, theme, version);
    let warning = config_warning.into_iter().chain(
        keybind_warnings
            .iter()
            .map(std::string::ToString::to_string),
    );
    if let Some(text) = warning.reduce(|a, b| format!("{a}; {b}")) {
        ws.chrome.status = Some(StatusMessage {
            level: StatusLevel::Warning,
            text,
            shown_at: std::time::Instant::now(),
        });
    }
    ws
}

/// git でないディレクトリでも開けるよう、引けなければパスから作れるところまでで諦める。
/// ブランチは worktree 一覧が届いてから決まる。
fn repo_state(root: PathBuf, config: &Config) -> RepoState {
    let mut repo = RepoState::open(&root).unwrap_or_else(|_| {
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        RepoState {
            root: root.clone(),
            name,
            main_branch: String::from("main"),
            known: vec![root.clone()],
        }
    });
    for extra in &config.general.repos {
        repo.remember(extra);
    }
    repo
}

/// 立ち上がらなくても起動は続ける。
fn started<T>(what: &str, made: Result<T>) -> Option<T> {
    match made {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("{what}: {e:#}");
            None
        }
    }
}

/// TUI を起動するときの引数。
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    repo: Option<String>,
    second_window: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Self::default();
        for arg in args {
            match arg.as_str() {
                "--second-window" => options.second_window = true,
                _ if arg.starts_with('-') => {
                    anyhow::bail!("unknown option: {arg}\nRun conductor --help for the usage.")
                }
                _ if options.repo.is_none() => options.repo = Some(arg),
                _ => anyhow::bail!("conductor opens one repository, but got a second path: {arg}"),
            }
        }
        Ok(options)
    }
}

/// 引数 (無ければ現在のディレクトリ) を絶対パスにする。
fn arg_path(arg: Option<String>) -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(match arg {
        Some(arg) if Path::new(&arg).is_absolute() => PathBuf::from(arg),
        Some(arg) => cwd.join(arg),
        None => cwd,
    })
}

/// 囲っている worktree のルートまで登る。.conductor/ は探した場所に作られるので、
/// サブディレクトリから起動すると 2 つ目の空のデータベースができてしまう。
fn worktree_root(from: &Path) -> PathBuf {
    GitEngine::open(from)
        .ok()
        .and_then(|git| git.list_worktrees().ok())
        .and_then(|worktrees| {
            worktrees
                .into_iter()
                .filter(|w| from.starts_with(&w.path))
                .max_by_key(|w| w.path.components().count())
                .map(|w| w.path)
        })
        .unwrap_or_else(|| from.to_path_buf())
}

/// 何か出力して終了すべきなら Some(結果)、TUI を起動して続けるなら None。
fn cli_fast_path(version: &str) -> Option<Result<()>> {
    match std::env::args().nth(1)?.as_str() {
        "--version" | "-V" => {
            println!("conductor {version}");
            Some(Ok(()))
        }
        "--help" | "-h" => {
            print_help(version);
            Some(Ok(()))
        }
        "mcp-serve" => Some(conductor_mcp::run(std::env::args(), version)),
        "cc-hook" => Some(cc_hook::run()),
        "cc-signal" => Some(match std::env::args().nth(2).as_deref() {
            Some("active") => cc_hook::run_signal(cc_hook::CcSignal::Active),
            Some("waiting") => cc_hook::run_signal(cc_hook::CcSignal::Waiting),
            other => Err(anyhow::anyhow!(
                "cc-signal expects active or waiting, got {other:?}"
            )),
        }),
        "cc-statusline" => Some(cc_hook::statusline::run().map(|code| {
            if code != 0 {
                std::process::exit(code);
            }
        })),
        "index" => Some(build_index()),
        "pty-host" => Some(run_pty_host(version)),
        _ => None,
    }
}

fn run_pty_host(version: &str) -> Result<()> {
    let root = arg_path(std::env::args().nth(2))?;
    let scrollback = Config::load()
        .unwrap_or_else(|e| {
            log::warn!("config: {e:#}");
            Config::default()
        })
        .terminal
        .inactive_scrollback;
    conductor_svc::pty::run_host(&root, version, scrollback)
}

fn build_index() -> Result<()> {
    let root = worktree_root(&arg_path(std::env::args().nth(2))?);
    let outcome = semantic_index::build_index(&root)?;
    for built in &outcome.built {
        println!(
            "{}: {} documents ({} without provenance)",
            built.index.display(),
            built.documents,
            built.missing_provenance
        );
    }
    for failure in &outcome.failures {
        eprintln!("{failure}");
    }
    Ok(())
}

fn print_help(version: &str) {
    println!(
        r#"conductor {version}

Usage: conductor [REPO_PATH] [--second-window]
       conductor index [REPO_PATH]
       conductor mcp-serve [--db <PATH>]
       conductor cc-hook
       conductor cc-signal <active|waiting>
       conductor cc-statusline
       conductor pty-host [REPO_PATH]

  REPO_PATH    Git repository to open (defaults to the current directory)

Commands:
  index        Build the SCIP code index for every index root in the tree,
               into the main worktree's .conductor/. An index root is a
               directory holding Cargo.toml (rust-analyzer), go.mod (scip-go)
               or tsconfig.json (scip-typescript); the tool for each must be
               on PATH. A root whose tool is missing is skipped and the rest
               are still built.

  mcp-serve    Serve the review database to Claude Code over stdio (MCP).
               Started automatically by conductor and by the Claude Code
               plugin; not usually run by hand.

    --db <PATH>    Review database to serve. Defaults to $CONDUCTOR_DB_PATH,
                   then .conductor/conductor.db in the surrounding repository.

  cc-hook      Claude Code SessionStart hook. Reads the hook payload on stdin
               and reports the panel's current session id back to the running
               conductor. Wired up automatically via --settings when conductor
               spawns a Claude panel; not usually run by hand.

  cc-signal    Claude Code prompt/stop/notification hook. Tells the running
               conductor whether the panel in this directory is working or
               waiting for you, which drives the worktree monitor strip. Wired
               up the same way as cc-hook; not usually run by hand.

  cc-statusline
               Claude Code statusLine. Keeps a copy of the status JSON (context
               and 5h / 7d usage) for the session list, then runs your own
               statusLine command with the same input and prints its output.
               Wired up the same way as cc-hook; not usually run by hand.

  pty-host     Hold the repository's Claude Code and shell sessions so they
               keep running after the window closes. Started by conductor
               and exits once it has no sessions and no window; not usually
               run by hand.

Options:
  --second-window  Open a repository that another conductor already has open.
                   For verifying a build, not for working in. A repository has
                   one set of .conductor/ channels and the first window keeps
                   them, so this window has no MCP refresh, no Claude Code
                   session hooks (the monitor strip stays blank and a panel's
                   /clear is not followed) and writes no claude-hooks.json.
                   The review database and the code index are shared as usual.

  -V, --version    Print version and exit
  -h, --help       Print this help and exit"#
    );
}

/// 端末の背景色 (OSC11) を問い合わせ、設定が無ければ明暗に合うテーマへ切り替える。
/// 応答を自分で stdin から読むので、呼ぶ場所はモジュール doc の順序制約に従う。
fn apply_auto_theme(ws: &mut Workspace) {
    let Some(lum) = term_caps::query_background_luminance() else {
        return;
    };
    match term_caps::auto_theme_for_background(lum, ws.config.ui.theme.as_deref()) {
        // セッション限りの切り替え。テーマピッカーで上書きできる。
        Some(name) => ws.theme = Theme::from_name(name),
        None => log::info!("OSC11 luminance={lum:.2}: keeping the configured theme"),
    }
}

/// アイコンの文字セットが未設定なら端末から判定して設定ファイルへ書く。
fn apply_auto_icons(ws: &mut Workspace) {
    if ws.config.ui.icons.is_some() {
        return;
    }
    let Some(set) = term_caps::detect_icon_set() else {
        return;
    };
    ws.config.ui.icons = Some(set);
    if let Err(e) = config::persist_ui_icons(set) {
        log::warn!("icon set: detected {set:?} but could not write it to config: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::Options;

    fn parse(args: &[&str]) -> anyhow::Result<Options> {
        Options::parse(args.iter().map(|a| (*a).to_string()))
    }

    #[test]
    fn フラグとパスは順番を問わない() {
        let want = Options {
            repo: Some("../other".into()),
            second_window: true,
        };
        assert_eq!(parse(&["--second-window", "../other"]).unwrap(), want);
        assert_eq!(parse(&["../other", "--second-window"]).unwrap(), want);
        assert_eq!(parse(&[]).unwrap(), Options::default());
    }

    #[test]
    fn 読めない引数はパスとして飲み込まずに断る() {
        assert!(parse(&["--secondwindow"]).is_err());
        assert!(parse(&["one", "two"]).is_err());
    }
}
