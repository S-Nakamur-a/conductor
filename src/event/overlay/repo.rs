//! リポジトリの選択と開始のオーバーレイ: 複数リポジトリ切り替え、パス指定での
//! リポジトリオープン入力、PR intake (Review Pull Request)。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::overlay_list_nav;

// オーバーレイ: リポジトリセレクタ

pub(in crate::event) fn handle_repo_selector_key(app: &mut App, key: KeyEvent) {
    let count = app.repo.known.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.repo_selector.selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let selected = app.overlays.repo_selector.selected;
            app.overlays.active = ActiveOverlay::None;
            app.switch_repo(selected);
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
        }
        _ => {}
    }
}

// オーバーレイ: リポジトリパス入力

pub(in crate::event) fn handle_open_repo_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.open_repo.buffer.clear();
        }
        KeyCode::Enter => {
            let buffer = app.overlays.open_repo.buffer.text().to_string();
            app.overlays.active = ActiveOverlay::None;
            app.overlays.open_repo.buffer.clear();
            app.open_repo_from_path(&buffer);
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.open_repo.buffer.delete_to_line_start();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.open_repo.buffer, false);
        }
        _ => {
            app.overlays.open_repo.buffer.handle_key(key);
        }
    }
}

// オーバーレイ: PR intake (Review Pull Request)

pub(in crate::event) fn handle_pr_input_key(app: &mut App, key: KeyEvent) {
    // gh/git の intake 実行中は Esc のみ受け付ける。入力自体をフリーズさせることで、
    // 誤操作のキー入力がバックグラウンドスレッドと競合するのを防ぐ。
    if app.overlays.pr_input.loading {
        if key.code == KeyCode::Esc {
            app.overlays.active = ActiveOverlay::None;
        }
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.pr_input.buffer.clear();
            app.overlays.pr_input.error = None;
        }
        KeyCode::Enter => {
            let input = app.overlays.pr_input.buffer.text().to_string();
            if !input.trim().is_empty() {
                app.overlays.pr_input.error = None;
                app.start_pr_intake(&input);
            }
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.pr_input.buffer.delete_to_line_start();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.pr_input.buffer, false);
        }
        _ => {
            // 失敗後に何か編集したら古いエラーを消す。ユーザが既に変更した入力の
            // 隣に、古いエラーが居座り続けないようにするため。
            app.overlays.pr_input.error = None;
            app.overlays.pr_input.buffer.handle_key(key);
        }
    }
}
