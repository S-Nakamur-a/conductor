//! ターミナルパネルのヘルパー — PTY への転送、セッションの起動、タブクリック。
//!
//! forward_key_to_pty が使う KeyEvent → ANSI バイト列変換は ansi サブモジュールにある。

mod ansi;

use crossterm::event::KeyEvent;

use crate::app::{App, Focus, StatusLevel};
use crate::terminal::link as terminal_link;

use ansi::key_event_to_ansi;

/// 指定インデックスの PTY セッションへキーイベントを転送する。
pub(crate) fn forward_key_to_pty(app: &mut App, session_idx: usize, key: KeyEvent) {
    // アプリケーションカーソルキーモード（DECCKM）を有効にするプログラム —
    // less/bat のようなページャや vim のようなエディタ — は矢印キーや
    // Home/End を CSI（ESC [）ではなく SS3（ESC O）として期待する。セッションの
    // 現在のモードに従うことで、キーが実際に認識される（例: bat での矢印
    // キースクロール）。
    let app_cursor = app
        .terminal
        .pty_manager
        .session_application_cursor(session_idx);
    let Some(data) = key_event_to_ansi(&key, app_cursor) else {
        return;
    };

    if let Err(e) = app
        .terminal
        .pty_manager
        .write_to_session(session_idx, &data)
    {
        log::warn!("failed to write to PTY session: {e}");
    } else {
        // ユーザがターミナルに入力したらライブ表示に戻す。
        if let Some(pane) = app.terminal.pane_mut(app.focus.current()) {
            pane.scroll = 0;
        }
        // Claude Code セッションにユーザ入力を送ったら CC 待機シグナルをクリアする。
        app.clear_cc_waiting_signal(session_idx);
    }
}

/// 現在のフォーカス（Claude Code か Shell）に応じて新しいターミナルセッションを起動する。
pub(crate) fn spawn_terminal_session(app: &mut App) {
    match app.focus.current() {
        Focus::TerminalClaude => {
            app.set_status("Starting Claude Code...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(
                    format!("Failed to start Claude Code: {e}"),
                    StatusLevel::Error,
                );
                log::warn!("failed to spawn Claude Code session: {e}");
            } else {
                app.status_message = None;
            }
        }
        Focus::TerminalShell => {
            app.set_status("Starting shell...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_shell() {
                app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
                log::warn!("failed to spawn shell session: {e}");
            } else {
                app.status_message = None;
            }
        }
        _ => {}
    }
}

/// ターミナルのタブバーのクリックを処理する。
/// is_claude は Claude パネルなら true、Shell パネルなら false。
///
/// クリック判定は描画時（tab_bar::render）に記録されたヒット領域に基づくので、
/// スクロールするタブストリップと常に一致する — click_col は絶対スクリーン列
/// （記録されている領域も同様）。
pub(crate) fn handle_terminal_tab_click(app: &mut App, click_col: u16, is_claude: bool) {
    use crate::ui::tab_bar::TabAction;

    let hit = if is_claude {
        app.terminal.claude.tab_hits.at(click_col)
    } else {
        app.terminal.shell.tab_hits.at(click_col)
    };
    let Some(action) = hit else {
        return;
    };

    match action {
        TabAction::Select(global_idx) => {
            // セッションを切り替える（スクロールとレンダーキャッシュをリセットし、
            // 新しく選択したセッションでパネルを再描画させる）。
            if is_claude {
                app.switch_claude_session(global_idx);
            } else {
                app.switch_shell_session(global_idx);
            }
        }
        TabAction::Close(global_idx) => {
            // どのタブであっても1クリックで閉じる。これは、アクティブな
            // セッションだけを閉じ、非アクティブなセッションは選択するだけに
            // していた以前のガードを意図的に取り除いたもので、以前は閉じるのに
            // もう一度クリックが必要だった。そのガードはタブの色分けと対で
            // 成り立っていた — アクティブな [x] は theme.error（「これは
            // killする」）、非アクティブなものは theme.muted（「これは選択する
            // だけ」）で、挙動と見た目が一致していた。すべての [x] を初回
            // クリックで閉じるようにするには、それらすべてを theme.error で
            // 塗り直す必要があり（ui::tab_bar::render で実施済み）、この変更と
            // 切り離せない。グレーのアイコンが黙って実行中セッションを kill
            // するのは、2クリックのガードよりもはるかに悪いアフォーダンスに
            // なってしまう。
            app.close_terminal_session(global_idx);
            // 閉じると以降のセッションのインデックスが1つずつ繰り上がり、
            // タブのラベルは固定幅なので、次のタブの [x] はいま閉じたタブと
            // 同じスクリーン列に来る。そこへの2回目のクリック — 反射的な
            // ダブルクリックや、再描画前に同じフレームで2つのイベントが
            // 処理された場合 — は古いヒットマップに対して解決され、ユーザが
            // 狙っていなかったセッションを kill してしまう。ヒット領域を
            // クリアすることで、次のクリックは新しい描画を待つことになる。
            if is_claude {
                app.terminal.claude.tab_hits.clear();
            } else {
                app.terminal.shell.tab_hits.clear();
            }
        }
        TabAction::Add => {
            if is_claude {
                if let Err(e) = app.spawn_claude_code() {
                    app.set_status(
                        format!("Failed to start Claude Code: {e}"),
                        StatusLevel::Error,
                    );
                }
            } else if let Err(e) = app.spawn_shell() {
                app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
            }
        }
        TabAction::Expand => {
            let target = if is_claude {
                Focus::TerminalClaude
            } else {
                Focus::TerminalShell
            };
            if app.expanded_panel.is_some() {
                app.expanded_panel = None;
            } else {
                app.expanded_panel = Some(target);
            }
        }
        TabAction::ScrollLeft => {
            let scroll = if is_claude {
                &mut app.terminal.claude.tab_scroll
            } else {
                &mut app.terminal.shell.tab_scroll
            };
            *scroll = scroll.saturating_sub(1);
        }
        TabAction::ScrollRight => {
            let scroll = if is_claude {
                &mut app.terminal.claude.tab_scroll
            } else {
                &mut app.terminal.shell.tab_scroll
            };
            *scroll += 1;
        }
    }
}

/// 直近のターミナル出力からファイルパスを探し、最初に見つかったものを Viewer で開く。
///
/// Ctrl+G（またはユーザ設定のキー）で発火する。アクティブな PTY セッションの
/// 画面に表示されている行を、カーソル行から上方向へスキャンする。
pub(crate) fn open_file_from_terminal_output(app: &mut App) {
    let Some(pane) = app.terminal.pane(app.focus.current()) else {
        return;
    };
    let (session_idx, scroll_offset) = (pane.active_session, pane.scroll);

    let Some(idx) = session_idx else {
        app.set_status(
            "No active terminal session".to_string(),
            StatusLevel::Warning,
        );
        return;
    };

    let Some(screen_arc) = app.terminal.pty_manager.get_screen(idx) else {
        return;
    };

    // リンクの実在確認に使う根は Viewer のツリーのもの。ここで確認した相対パスを
    // そのまま open_file_in_viewer に渡すので、別の根で確認すると「リンクとして
    // 認識されたのに開くと空」になる。
    let wt_path = app.explorer.root().to_path_buf();

    // パーサをロックし、scrollback を設定して、カーソル行から上方向に行をスキャンする。
    let found = {
        let mut parser = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
        parser.set_scrollback(scroll_offset);
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let cursor_row = screen.cursor_position().0;

        let mut result = None;
        // カーソル行から上方向にスキャンし、直近のファイル参照を探す。
        for offset in 0..rows {
            let r = if cursor_row >= offset {
                cursor_row - offset
            } else {
                break;
            };
            let text = terminal_link::extract_row_text(screen, r, cols);
            let links = terminal_link::detect_file_links(&text, &wt_path);
            if let Some(link) = links.into_iter().next() {
                result = Some((link.path.clone(), link.line));
                break;
            }
        }
        parser.set_scrollback(0);
        result
    };

    match found {
        Some((path, line)) => app.open_file_in_viewer(&path, line),
        None => app.set_status(
            "No file path found in terminal output".to_string(),
            StatusLevel::Warning,
        ),
    }
}
