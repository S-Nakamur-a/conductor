//! conductor-next の起動と終了。
//!
//! 順番に意味がある — CLI の即答フラグは端末に触る前に返さねばならず (mcp-serve は
//! stdout で JSON-RPC を話す)、端末ケイパビリティの問い合わせは raw mode に入った
//! あと・ループが stdin を読み始める前でなければならない。

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use conductor_core::config::Config;
use conductor_core::git_engine::GitEngine;
use conductor_core::keymap::KeyMap;
use conductor_core::theme::Theme;
use conductor_core::{cc_hook, config, instance_lock, semantic_index, term_caps};
use conductor_svc::Services;
use conductor_tui::workspace::{RepoState, StatusLevel, StatusMessage, Workspace};
use conductor_tui::{run, term};
use crossterm::execute;
use crossterm::terminal::SetTitle;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

fn main() -> Result<()> {
    term::install_panic_hook();
    env_logger::init();

    if let Some(result) = cli_fast_path() {
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

    let mut ws = workspace(repo_root);
    let mut svc = Services::new();
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let modes = term::enter(terminal.backend_mut())?;
    let _ = execute!(
        io::stdout(),
        SetTitle(format!("conductor - {}", ws.repo.name))
    );

    apply_auto_theme(&mut ws);
    apply_auto_icons(&mut ws);

    let result = run::run(&mut terminal, &mut ws, &mut svc);

    // 復旧はエラー時も必ず。1 つ失敗しても残りを試す。
    let _ = term::leave(terminal.backend_mut(), modes);
    let _ = execute!(terminal.backend_mut(), SetTitle(""));
    let _ = terminal.show_cursor();
    result
}

fn workspace(root: PathBuf) -> Workspace {
    let (config, config_warning) = match Config::load() {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(format!("config: {e}"))),
    };
    let (keymap, keybind_warnings) = KeyMap::with_warnings(&config.keybinds);
    let theme = Theme::from_name(config.theme_name());
    let repo = repo_state(root, config.general.main_branch.clone());

    let mut ws = Workspace::new(repo, config, keymap, theme);
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

/// リポジトリ名と HEAD ブランチを添える。git でないディレクトリでも開けるよう、
/// 引けなければパスから作れるところまでで諦める。
fn repo_state(root: PathBuf, main_branch: String) -> RepoState {
    let dir_name = |path: &Path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        )
    };
    let git = GitEngine::open(&root).ok();
    let name = git
        .as_ref()
        .and_then(|g| g.main_worktree_path().ok())
        .map_or_else(|| dir_name(&root), |main| dir_name(&main));
    let branch = git
        .as_ref()
        .and_then(|g| g.list_worktrees().ok())
        .and_then(|worktrees| {
            worktrees
                .into_iter()
                .find(|w| w.path == root)
                .map(|w| w.branch)
        })
        .unwrap_or_else(|| main_branch.clone());
    RepoState {
        root,
        name,
        branch,
        main_branch,
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
fn cli_fast_path() -> Option<Result<()>> {
    match std::env::args().nth(1)?.as_str() {
        "--version" | "-V" => {
            println!("conductor {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        "--help" | "-h" => {
            print_help();
            Some(Ok(()))
        }
        "mcp-serve" => Some(conductor_mcp::run(
            std::env::args(),
            env!("CARGO_PKG_VERSION"),
        )),
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

fn print_help() {
    println!(
        r#"conductor {}

Usage: conductor-next [REPO_PATH]
       conductor-next index [REPO_PATH]
       conductor-next mcp-serve [--db <PATH>]
       conductor-next cc-hook

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
  -h, --help       Print this help and exit"#,
        env!("CARGO_PKG_VERSION")
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
