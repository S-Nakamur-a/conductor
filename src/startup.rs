//! 起動と終了の各段取り。
//!
//! `main` から切り出してあるのは、順番に意味があるから — CLI の即答フラグは
//! 端末に触る前に返さねばならず、端末ケイパビリティの問い合わせは raw mode に
//! 入ったあと・イベントループが stdin を読み始める前でなければならない。
//! 名前の付いた関数に分けておくと、`main` を読むだけでその順番が見える。

use std::io;
use std::path::PathBuf;

use anyhow::Result;

use crate::app::{self, App};
use crate::term_caps;

/// CLI の即答フラグを処理する。
///
/// 何か出力して終了すべきなら `Some(結果)`、TUI を起動して続けるなら `None`。
/// 端末に触る前に呼ぶこと — `mcp-serve` は stdout で JSON-RPC を話すので、
/// 代替スクリーンに入ったりケイパビリティを問い合わせたりするとプロトコルが壊れる。
/// `--version` は更新機能の検証プローブでもある (新しいバイナリに差し替える前に
/// `--version` で起動して正常終了するか確かめている)。
pub fn handle_cli_fast_path() -> Option<Result<()>> {
    let arg = std::env::args().nth(1)?;
    match arg.as_str() {
        "--version" | "-V" => {
            println!("conductor {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        "--help" | "-h" => {
            print_help();
            Some(Ok(()))
        }
        "mcp-serve" => Some(crate::mcp_serve::run()),
        _ => None,
    }
}

fn print_help() {
    println!(
        r#"conductor {}

Usage: conductor [REPO_PATH]
       conductor mcp-serve [--db <PATH>]

  REPO_PATH    Git repository to open (defaults to the current directory)

Commands:
  mcp-serve    Serve the review database to Claude Code over stdio (MCP).
               Started automatically by conductor and by the Claude Code
               plugin; not usually run by hand.

    --db <PATH>    Review database to serve. Defaults to $CONDUCTOR_DB_PATH,
                   then .conductor/conductor.db in the surrounding repository.

Options:
  -V, --version    Print version and exit
  -h, --help       Print this help and exit"#,
        env!("CARGO_PKG_VERSION")
    );
}

/// 引数 (または現在のディレクトリ) から、開くリポジトリのルートを決める。
///
/// 囲っている worktree のルートまで登る。`repo_path` を基準にするものは
/// すべてルートを前提にしている — 特に `.conductor/conductor.db` は探した場所に
/// 作られるので、サブディレクトリから起動すると、そのサブディレクトリの隣に
/// 空の 2 つ目のデータベースができてしまっていた (コメントもウォークスルーも
/// 無く、`.conductor/` だけが残る)。`discover` は既にルートならそのまま返し、
/// リンクされた worktree はメインではなく自分自身のルートに解決される。
pub fn resolve_repo_path() -> Result<PathBuf> {
    let arg_path = match std::env::args().nth(1) {
        Some(path) => {
            let p = PathBuf::from(&path);
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir()?.join(p)
            }
        }
        None => std::env::current_dir()?,
    };

    Ok(git2::Repository::discover(&arg_path)
        .ok()
        .and_then(|repo| repo.workdir().map(std::path::Path::to_path_buf))
        .unwrap_or(arg_path))
}

/// 端末の背景色 (OSC11) を問い合わせ、設定が無ければ明暗に合うテーマへ切り替える。
///
/// raw mode に入っていて、かつイベントループが stdin を読み始める前に呼ぶ必要が
/// ある (下のグラフィックスプローブと同じ制約)。「設定されていないときだけ」の
/// 判定と明暗のしきい値は `term_caps` 側にあるので、ここには分岐を持たない。
pub fn apply_auto_theme(app: &mut App) {
    let Some(lum) = term_caps::query_background_luminance() else {
        return;
    };
    let configured = app.config.ui.theme.as_deref();
    match term_caps::auto_theme_for_background(lum, configured) {
        Some(theme) => {
            // セッション限りの切り替え (persist=false)。テーマピッカーで上書き可能。
            app.set_theme(theme, false);
            log::info!(
                "OSC11 auto-detected light background (luminance={lum:.2}): switched to {theme}"
            );
        }
        None => {
            log::info!("OSC11 auto-detected background (luminance={lum:.2}): keeping current theme");
        }
    }
}

/// リッチモードのティアを決めて `app` に反映し、有効なら起動時に一言知らせる。
///
/// 代替スクリーンに入ったあと・イベントループが stdin を読み始める前に呼ぶこと:
/// グラフィックスプローブは端末からの応答を自分で stdin から読むので、crossterm の
/// イベントループが動いているとそれを横取りされてしまう。
pub fn detect_rich_mode(app: &mut App) {
    let caps = term_caps::TermCaps::detect_from_env();
    let mode = app.config.rich.mode.clone();
    // `auto` は端末がグラフィックス対応らしいときだけ問い合わせる (未知の端末で
    // 起動が遅くならないように)。`force` は常に問い合わせるので、ヒントの一覧に
    // 載っていない端末での逃げ道になる。
    let probed = if mode == "force" || (mode != "off" && caps.graphics_likely) {
        match ratatui_image::picker::Picker::from_query_stdio() {
            Ok(picker) => Some(picker),
            Err(e) => {
                log::warn!("graphics protocol probe failed: {e}");
                None
            }
        }
    } else {
        None
    };

    let protocol = probed.map(|p| p.protocol_type());
    app.rich.tier = term_caps::resolve_rich_tier(&mode, &caps, protocol);
    app.rich.available = app.rich.tier;
    if app.rich.has_graphics() {
        app.rich.picker = probed;
    }
    log::info!(
        "rich mode: tier={:?} terminal={:?} protocol={:?}",
        app.rich.tier,
        caps.terminal_name,
        protocol
    );

    if app.rich.is_rich() {
        app.status_message = Some(app::StatusMessage::new(
            rich_mode_banner(app.rich.has_graphics(), caps.terminal_name.as_deref()),
            app::StatusLevel::Info,
            app.ui_tick,
        ));
    }
}

fn rich_mode_banner(has_graphics: bool, terminal_name: Option<&str>) -> String {
    match (has_graphics, terminal_name) {
        (true, Some(name)) => format!("✨ Rich mode — {name} graphics detected"),
        (true, None) => String::from("✨ Rich mode — terminal graphics detected"),
        (false, Some(name)) => format!("✨ Rich mode — {name} truecolor"),
        (false, None) => String::from("✨ Rich mode — truecolor"),
    }
}

/// 更新が入っていれば、同じ引数で自分自身を exec し直す (戻らない)。
///
/// `exec` はプロセスイメージを置き換えるので、この先の Drop も後続のコードも
/// 走らない。永続化が必要なものはすべて呼び出し前に済ませておくこと。
pub fn restart_if_updated(app: &App) {
    if !app.update.should_restart {
        return;
    }
    println!("Restarting Conductor...");
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&app.update.startup_exe)
        .args(&app.update.startup_args)
        .exec();
    eprintln!("Failed to restart: {err}");
    std::process::exit(1);
}

/// セッションの成果 (ゲーミフィケーション) を標準出力にまとめる。
/// 何も起きていないセッションでは何も出さない。
pub fn print_session_summary(app: &App) {
    let (Some(store), Some(session_id)) = (&app.review_store, &app.stats.session_id) else {
        return;
    };
    let Ok(stats) = store.end_stats_session(session_id) else {
        return;
    };
    if stats.reviews_created + stats.branches_created + stats.commits_made == 0 {
        return;
    }

    println!("\n--- Conductor Session Summary ---");
    for (label, value) in [
        ("Reviews created: ", stats.reviews_created),
        ("Branches created:", stats.branches_created),
        ("Commits made:    ", stats.commits_made),
    ] {
        if value > 0 {
            println!("  {label} {value}");
        }
    }
    if let Ok(streak) = store.calculate_streak()
        && streak.consecutive_days > 0
    {
        println!("  Current streak:    {} day(s)", streak.consecutive_days);
    }
    println!("---------------------------------\n");
}

/// パニックしても端末を元に戻すフックを仕掛け、バックトレースをログに残す。
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // まず端末を戻す。自分が失敗しうる I/O より先にやる: パニックは端末を
        // `enter_tui` が設定したままにするし、`leave_tui` は `run_loop` が
        // *return* したときにしか走らない — アンワインドは飛ばしてしまう。
        // これが無いと、カーソルが見えず (ratatui は毎フレーム `\x1b[?25l` を
        // 出す)、マウストラッキングが有効なまま (`\x1b[?1003h` で選択と
        // ポインタが変になる)、代替スクリーンと raw mode も生きたままの
        // シェルにユーザーが放り出され、`reset` を打つまで直らない。
        //
        // メインスレッドだけ。このクレートは `panic = "abort"` ではないので、
        // ワーカー (バックグラウンド差分・シンボル索引・worktree 操作) は自分
        // だけをアンワインドし、イベントループは 60fps で描き続ける。*その*
        // パニックで端末を畳むと、まだ動いている TUI の下で代替スクリーンと
        // raw mode が解除された状態になり、フレームがユーザーの実シェルに
        // 書き殴られ始める。ワーカーが 1 つ死ぬのは復帰可能で、下のログを
        // 残すだけで十分。`execute!` は内部で flush するので追加の flush は不要。
        if std::thread::current().name() == Some("main") {
            let _ = crate::restore_terminal_modes(&mut io::stdout());
        }

        if let Some(config_dir) = dirs::config_dir() {
            let log_dir = config_dir.join("conductor");
            let _ = std::fs::create_dir_all(&log_dir);
            let bt = std::backtrace::Backtrace::force_capture();
            let payload = format!(
                "=== Conductor panic at {} ===\n{info}\n\nBacktrace:\n{bt}\n\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            );
            let _ = std::fs::write(log_dir.join("panic.log"), &payload);
        }
        default_hook(info);
    }));
}
