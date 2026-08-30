//! Conductor — 端末で動く Git ワークスペース兼コードレビューツール。

mod ai_caller;
mod anim;
mod app;
mod background;
mod cc_hook;
mod cc_notify;
mod ccusage_cache;
mod claude_sessions;
mod command_palette;
mod config;
mod config_watcher;
mod diff_state;
mod event;
mod event_loop;
mod event_loop_timers;
mod explorer;
mod file_watcher;
mod gemini_api;
mod git_engine;
mod go_test;
mod grep_search;
pub mod hit_map;
mod icons;
mod instance_lock;
mod keymap;
mod mcp_serve;
mod menu;
mod overlay;
mod overlay_grep;
mod pr_intake;
mod pty_manager;
mod reflow;
mod refresh_pipe;
mod repo_path;
mod revidere;
mod review_publish;
mod review_state;
mod review_store;
mod rust_test;
mod search_result_tree;
mod semantic_index;
mod startup;
mod symbol_index;
mod term_caps;
mod terminal_link;
mod terminal_state;
mod test_run;
mod text_input;
mod theme;
mod timer;
pub mod types;
mod ui;
mod update_checker;
mod viewer;
mod worktree;

use std::io;

use anyhow::Result;
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;

fn main() -> Result<()> {
    startup::install_panic_hook();
    env_logger::init();

    // 端末に触る前に。mcp-serve は stdout で JSON-RPC を話すので、
    // 代替スクリーンに入ったあとでは手遅れになる。
    if let Some(result) = startup::handle_cli_fast_path() {
        return result;
    }

    // 端末に触る前に。断られたら通常スクリーンのまま理由を出して終わりたい。
    let repo_path = startup::resolve_repo_path()?;
    let _instance_lock = match instance_lock::acquire(&repo_path) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            anyhow::bail!(
                "conductor is already open on this repository:\n  {}\n\n\
                 All worktrees of a repository share one window.\n\
                 Switch to that window, or close it first.",
                instance_lock::locked_repo_root(&repo_path).display()
            );
        }
        // 排他できないことを理由にリポジトリを開けなくするほうが害が大きい。
        Err(e) => {
            log::warn!("could not take the single-instance lock: {e:#}");
            None
        }
    };

    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    log::debug!("keyboard_enhanced = {keyboard_enhanced}");
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    enter_tui(terminal.backend_mut(), keyboard_enhanced)?;

    let mut app = App::new(repo_path);
    execute!(
        io::stdout(),
        SetTitle(format!("conductor - {}", app.repo.main_name))
    )?;

    // raw mode に入ったあと・イベントループが stdin を読み始める前でなければ
    // ならない。端末への問い合わせの応答を自分で stdin から読むため。
    startup::apply_auto_theme(&mut app);
    // こちらは端末に問い合わせないので順序の制約は無いが、外観の初期化として
    // 隣に置いている。
    startup::apply_auto_icons(&mut app);

    app.start_symbol_index_build();
    app.start_semantic_index_load();

    let result = event_loop::run_loop(&mut terminal, &mut app);

    // 端末の復旧はエラー時も必ず。途中で 1 つ失敗しても残りを試す
    // (中途半端に戻った tty にユーザーを取り残さないため)。
    let _ = leave_tui(terminal.backend_mut(), keyboard_enhanced);
    let _ = execute!(terminal.backend_mut(), SetTitle(""));
    let _ = terminal.show_cursor();

    // 再起動より前に。exec はプロセスイメージを置き換えるので、
    // その先では Drop も後続のコードも走らない。
    app.persist_view_state();
    startup::restart_if_updated(&app); // 更新済みなら戻らない
    startup::print_session_summary(&app);

    result
}

// 端末モードの設定と後始末
//
// enter_tui と leave_tui は厳密に逆の操作で、拡張フラグの push/pop が
// raw mode・代替スクリーン・マウス/ペーストの捕捉を挟む形になっている。
// これにより中断から復帰までが綺麗に往復する。両方を 1 か所に置いてあるので、
// main の起動・終了と、エディタ中断時のガードがまったく同じ対称な手順を
// 共有できる。片方にだけフラグが増えると、戻ってきた端末が微妙に壊れる。

/// 全画面 TUI の端末モードに入る (raw mode、代替スクリーン、マウスと
/// bracketed paste の捕捉、対応していれば kitty のキーボード拡張フラグ)。
fn enter_tui<W: io::Write>(w: &mut W, keyboard_enhanced: bool) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(
        w,
        EnterAlternateScreen,
        // TUI は全セルを明示的に位置決めする (ratatui が行ごとに MoveTo する)
        // ので、自動折り返しは切っておかなければならない。切らないと、こちらの
        // 数えた幅より端末が広く描く字形があったときに行の末尾が最終桁を越え、
        // 自動折り返しで次の行の先頭桁へ送られてしまう。あるパネルのはみ出しが
        // 別のパネルの左端に染み出すことになる。折り返しを切っておけば、
        // はみ出しは右端で無害にクランプされる。
        DisableLineWrap,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        // crossterm はマウスが端末ウィンドウから出たことを報告しない。そのため
        // Event::FocusLost (端末がフォーカスを完全に失う、alt-tab など) が、
        // 「今この瞬間、マウスは描画されたどの要素の上にも乗っていない」と
        // イベントループが確信できる唯一の信号になる。古くなったホバー状態
        // (Viewer の下線、ポップアップ、ツリーや差分の行ハイライト) を消すのに使う。
        crossterm::event::EnableFocusChange,
    )?;
    if keyboard_enhanced {
        execute!(
            w,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )?;
    }
    Ok(())
}

/// 全画面 TUI の端末モードを抜け、端末を cooked / 通常スクリーンの状態へ戻す。
/// [enter_tui] の厳密な逆操作。
fn leave_tui<W: io::Write>(w: &mut W, keyboard_enhanced: bool) -> io::Result<()> {
    if keyboard_enhanced {
        execute!(w, PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    restore_terminal_modes(w)?;
    Ok(())
}

/// [leave_tui] が依存するモードのリセットをすべて書き出し、raw mode を抜ける。
///
/// leave_tui から切り出してあるのは、パニックフックが再利用するため。フックは
/// Terminal にも keyboard_enhanced にも触れないが、enter_tui が設定した
/// モードは元に戻さねばならない。戻さないとアンワインドでユーザーの tty が
/// 取り残される。ジェネリックな writer を取ることで、実際の端末に触らずに
/// 出力されるシーケンスをテストで検証できるようにもなっている。
///
/// cursor::Show を含めているのは、ratatui が毎フレーム カーソルを隠すため
/// (ウィジェットがカーソル位置を要求しない限り Terminal::draw が
/// hide_cursor を呼ぶ)。つまり \x1b[?25l が事実上いつも最後に設定した
/// カーソル状態になる。通常の終了経路では terminal.show_cursor() で戻すが、
/// パニック経路には Terminal が無いのでここでやる必要がある。
pub(crate) fn restore_terminal_modes<W: io::Write>(w: &mut W) -> io::Result<()> {
    // disable_raw_mode はエスケープシーケンスではなく libc / termios の呼び出しなので
    // w には何も書かない。leave_tui が既に呼んだあとでも無害 (かつ冪等)。
    let _ = disable_raw_mode();
    execute!(
        w,
        EnableLineWrap,
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        crossterm::cursor::Show,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::restore_terminal_modes;

    /// パニックフックの存在意義は、leave_tui が一度も走らなくてもこれらの
    /// モードが元に戻ること。execute! の並びが完全であり続けることを信じる
    /// のではなく、生のバイト列に対して検証する。異常終了のあとユーザーが実際に
    /// 報告してくる症状は「カーソルが見えない」と「マウスの挙動がおかしい」の
    /// 2 つで、それがちょうどこの 2 つのリセットに対応する。
    #[test]
    fn panic_hook_restores_terminal() {
        let mut buf: Vec<u8> = Vec::new();
        restore_terminal_modes(&mut buf).expect("writing to a Vec cannot fail");
        let seq = String::from_utf8(buf).expect("escape sequences are ASCII");

        assert!(
            seq.contains("\x1b[?25h"),
            "caret must be shown again (ratatui hides it every frame); got {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1003l"),
            "any-event mouse tracking must be turned off; got {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1049l"),
            "the alternate screen must be left; got {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?2004l"),
            "bracketed paste must be turned off; got {seq:?}"
        );
    }
}
