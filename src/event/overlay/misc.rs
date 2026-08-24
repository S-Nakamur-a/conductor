//! 小さな独立オーバーレイ群: ヘルプポップアップ、コマンドパレット、
//! テーマピッカー（ライブプレビュー付き）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, StatusLevel};
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::filterable_overlay_list_nav;
use super::overlay_list_nav;

// オーバーレイ: ヘルプ

pub(in crate::event) fn handle_help_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.overlays.active = ActiveOverlay::None;
        }
        // コンテキストを切り替えることでヘルプページを行き来できるようにする。
        KeyCode::Char('1') => app.overlays.help.context = Focus::Worktree,
        KeyCode::Char('2') => app.overlays.help.context = Focus::Explorer,
        KeyCode::Char('3') => app.overlays.help.context = Focus::Viewer,
        KeyCode::Char('4') => app.overlays.help.context = Focus::TerminalClaude,
        _ => {}
    }
}

// オーバーレイ: コマンドパレット

pub(in crate::event) fn handle_command_palette_key(app: &mut App, key: KeyEvent) {
    use crate::command_palette;

    let filtered = command_palette::filter_commands(
        &app.overlays.command_palette.filter,
        &app.keymap,
        app.focus.key_context(),
    );
    let count = filtered.len();

    if filterable_overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.command_palette.selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            if let Some(scored) = filtered.get(app.overlays.command_palette.selected) {
                let id = command_palette::COMMANDS[scored.index].id;
                app.overlays.active = ActiveOverlay::None;
                app.overlays.command_palette.filter.clear();
                app.execute_palette_command(id);
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.command_palette.filter.clear();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.command_palette.filter.delete_to_line_start();
            app.overlays.command_palette.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.command_palette.filter, false);
            app.overlays.command_palette.selected = 0;
        }
        _ => {
            if app.overlays.command_palette.filter.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.overlays.command_palette.selected = 0;
                    }
                    _ => {}
                }
            }
        }
    }
}

// オーバーレイ: テーマピッカー

/// テーマピッカーオーバーレイのキーを処理する。
///
/// Up/Down（または j/k）でライブプレビューしながらリストを閲覧する。移動する
/// たびに set_theme(name, false) を呼ぶことで、永続化せずに即座に UI へ反映する。
/// Enter で選択したテーマを確定・永続化し、Esc でピッカーを開いた時点の
/// テーマに戻す。
pub(in crate::event) fn handle_theme_picker_key(app: &mut App, key: KeyEvent) {
    let count = app.overlays.theme_picker.themes.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.theme_picker.selected,
        count,
    ) {
        // ライブプレビュー: 永続化せずに新しくハイライトされたテーマを適用する。
        let name = app
            .overlays
            .theme_picker
            .themes
            .get(app.overlays.theme_picker.selected)
            .cloned()
            .unwrap_or_default();
        app.set_theme(&name, false);
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let name = app
                .overlays
                .theme_picker
                .themes
                .get(app.overlays.theme_picker.selected)
                .cloned()
                .unwrap_or_default();
            app.overlays.active = ActiveOverlay::None;
            app.set_theme(&name, true);
            app.set_status(format!("Theme: {name}"), StatusLevel::Success);
        }
        KeyCode::Esc => {
            let orig = app.overlays.theme_picker.original.clone();
            app.overlays.active = ActiveOverlay::None;
            app.set_theme(&orig, false);
        }
        _ => {}
    }
}
