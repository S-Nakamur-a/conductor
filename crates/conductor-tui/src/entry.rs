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
use conductor_svc::watch::{CcNotifyListener, RefreshPipe};
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

    // 端末に触る前に。断られたら通常スクリーンのまま理由を出して終わりたい。
    let repo_root = worktree_root(&arg_path(std::env::args().nth(1))?);
    let _lock = match instance_lock::acquire(&repo_root) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => anyhow::bail!(
            "conductor is already open on this repository:\n  {}\n\n\
             All worktrees of a repository share one window.\n\
             Switch to that window, or close it first.",
            instance_lock::locked_repo_root(&repo_root).display()
        ),
        // 排他できないことを理由にリポジトリを開けなくするほうが害が大きい。
        Err(e) => {
            log::warn!("could not take the single-instance lock: {e:#}");
            None
        }
    };

    let mut ws = workspace(repo_root, version);
    let mut svc = Services::new();
    // Claude Code のフックがこのソケットへ active/waiting とセッション id を送る。
    let _cc_notify = match CcNotifyListener::new(&ws.repo.root, svc.sender()) {
        Ok(listener) => Some(listener),
        Err(e) => {
            log::warn!("cc-notify listener: {e:#}");
            None
        }
    };
    // MCP がレビュー DB を書いたら、この FIFO 越しに読み直しを促してくる。
    let _refresh = match RefreshPipe::new(&ws.repo.root, svc.sender()) {
        Ok(pipe) => Some(pipe),
        Err(e) => {
            log::warn!("refresh pipe: {e:#}");
            None
        }
    };
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
    let main_branch = config.general.main_branch.clone();
    let mut repo = RepoState::open(&root, &main_branch).unwrap_or_else(|_| {
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        RepoState {
            root: root.clone(),
            name,
            main_branch,
            known: vec![root.clone()],
        }
    });
    for extra in &config.general.repos {
        repo.remember(extra);
    }
    repo
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
        "index" => Some(build_index()),
        _ => None,
    }
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

Usage: conductor [REPO_PATH]
       conductor index [REPO_PATH]
       conductor mcp-serve [--db <PATH>]
       conductor cc-hook

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

Options:
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
