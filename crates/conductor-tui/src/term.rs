//! 端末モードの入退出と panic フック。
//!
//! [enter] と [leave] は厳密な逆操作で、片方にだけモードが増えると戻ってきた端末が
//! 壊れる。[restore_modes] だけは Terminal も [Modes] も要らない形にしてあり、
//! アンワインドを飛ばす panic 経路が同じリセットを共有する。

use std::io;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, supports_keyboard_enhancement,
};

/// [enter] が実際に設定したモード。[leave] はこれを消費するので、押していない
/// 拡張フラグを pop することがない。
#[derive(Debug, Clone, Copy)]
pub struct Modes {
    keyboard_enhanced: bool,
}

pub fn enter<W: io::Write>(w: &mut W) -> io::Result<Modes> {
    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    enable_raw_mode()?;
    execute!(
        w,
        EnterAlternateScreen,
        // ratatui は全セルを明示的に位置決めするので、自動折り返しが残っていると
        // こちらの数えた幅より広く描く字形で行末が次行の先頭へ送られ、隣のパネルに
        // 染み出す。切っておけば右端で無害にクランプされる。
        DisableLineWrap,
        EnableMouseCapture,
        EnableBracketedPaste,
        // crossterm はマウスが端末ウィンドウから出たことを報告しない。FocusLost が
        // 「今どの要素の上にもマウスは無い」と言える唯一の信号になる。
        EnableFocusChange,
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
    Ok(Modes { keyboard_enhanced })
}

pub fn leave<W: io::Write>(w: &mut W, modes: Modes) -> io::Result<()> {
    if modes.keyboard_enhanced {
        execute!(w, PopKeyboardEnhancementFlags)?;
    }
    restore_modes(w)
}

/// [enter] が設定したモードのリセットをすべて書き出す。
///
/// panic 経路には Terminal も [Modes] も無いので、ここは生の writer だけで完結する。
/// cursor::Show を含めるのは ratatui が毎フレーム カーソルを隠すため — 異常終了で
/// 最後に設定されたカーソル状態は事実上いつも \x1b[?25l になる。
pub fn restore_modes<W: io::Write>(w: &mut W) -> io::Result<()> {
    // termios の呼び出しなので w には何も書かない。二度呼んでも無害。
    let _ = disable_raw_mode();
    execute!(
        w,
        EnableLineWrap,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        crossterm::cursor::Show,
    )
}

/// panic しても端末を戻すフックを仕掛け、バックトレースをログに残す。
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 端末を戻すのはメインスレッドの panic だけ。ワーカーが 1 つ死んだだけなら
        // イベントループは描き続けるので、そこで代替スクリーンを畳むと動いている
        // TUI のフレームがユーザーの実シェルへ書き殴られる。
        if std::thread::current().name() == Some("main") {
            let _ = restore_modes(&mut io::stdout());
        }

        if let Some(config_dir) = dirs::config_dir() {
            let log_dir = config_dir.join("conductor");
            let _ = std::fs::create_dir_all(&log_dir);
            let payload = format!(
                "=== conductor panic at {} ===\n{info}\n\nBacktrace:\n{}\n\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                std::backtrace::Backtrace::force_capture(),
            );
            let _ = std::fs::write(log_dir.join("panic.log"), &payload);
        }
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::restore_modes;

    /// panic フックの存在意義は leave が一度も走らなくてもこれらが戻ること。
    /// execute! の並びを信じず、実際に出るバイト列で確かめる。
    #[test]
    fn panic経路のリセットは4つとも出る() {
        let mut buf: Vec<u8> = Vec::new();
        restore_modes(&mut buf).expect("writing to a Vec cannot fail");
        let seq = String::from_utf8(buf).expect("escape sequences are ASCII");

        for (code, what) in [
            ("\x1b[?1049l", "代替スクリーンを抜ける"),
            ("\x1b[?2004l", "bracketed paste を切る"),
            ("\x1b[?1003l", "マウス捕捉を切る"),
            ("\x1b[?25h", "カーソルを戻す"),
        ] {
            assert!(seq.contains(code), "{what} ({code}) が無い: {seq:?}");
        }
    }
}
