//! VCS 操作系のオーバーレイ: cherry-pick、switch branch（リモートブランチから
//! worktree を作成）、grab（既存のローカルブランチを取得）、prune（古い worktree
//! を削除）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, StatusLevel};
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::{filterable_overlay_list_nav, overlay_list_nav};

// オーバーレイ: cherry-pick

pub(in crate::event) fn handle_cherry_pick_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let count = app.overlays.cherry_pick.commits.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.cherry_pick.selected,
        count,
    ) {
        return None;
    }

    match key.code {
        KeyCode::Enter => {
            app.execute_cherry_pick();
            app.overlays.active = ActiveOverlay::None;
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
        }
        KeyCode::Tab => {
            // ソースブランチを順番に切り替える。
            let current_branch = app
                .worktrees
                .get(app.worktrees.selected_index())
                .map(|w| w.branch.clone())
                .unwrap_or_default();
            let other_branches: Vec<String> = app
                .worktrees
                .iter()
                .filter(|w| w.branch != current_branch)
                .map(|w| w.branch.clone())
                .collect();
            if !other_branches.is_empty() {
                let cur_idx = other_branches
                    .iter()
                    .position(|b| *b == app.overlays.cherry_pick.source_branch)
                    .unwrap_or(0);
                let next_idx = (cur_idx + 1) % other_branches.len();
                app.overlays.cherry_pick.source_branch = other_branches[next_idx].clone();
                app.load_cherry_pick_commits();
            }
        }
        _ => {}
    }
    None
}

// オーバーレイ: switch branch

pub(in crate::event) fn handle_switch_branch_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let filtered = app.filtered_switch_branches();
    let count = filtered.len();

    if filterable_overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.switch_branch.selected,
        count,
    ) {
        return None;
    }

    match key.code {
        KeyCode::Enter => {
            let filtered = app.filtered_switch_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.switch_branch.selected) {
                let branch = app
                    .overlays
                    .switch_branch
                    .branches
                    .get(original_idx)
                    .cloned()?;
                app.overlays.active = ActiveOverlay::None;
                app.overlays.switch_branch.filter.clear();
                app.create_worktree_from_remote(&branch);
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.switch_branch.filter.clear();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.switch_branch.filter.delete_to_line_start();
            app.overlays.switch_branch.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.switch_branch.filter, false);
            app.overlays.switch_branch.selected = 0;
        }
        _ => {
            if app.overlays.switch_branch.filter.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.overlays.switch_branch.selected = 0;
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

// オーバーレイ: grab

pub(in crate::event) fn handle_grab_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let filtered = app.filtered_grab_branches();
    let count = filtered.len();

    if filterable_overlay_list_nav(&app.keymap, &key, &mut app.overlays.grab.selected, count) {
        return None;
    }

    match key.code {
        KeyCode::Enter => {
            let filtered = app.filtered_grab_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.grab.selected) {
                let branch = app.overlays.grab.branches.get(original_idx).cloned()?;
                app.overlays.active = ActiveOverlay::None;
                app.overlays.grab.filter.clear();
                app.execute_grab(&branch);
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.grab.filter.clear();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.grab.filter.delete_to_line_start();
            app.overlays.grab.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.grab.filter, false);
            app.overlays.grab.selected = 0;
        }
        _ => {
            if app.overlays.grab.filter.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.overlays.grab.selected = 0;
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

// オーバーレイ: prune

pub(in crate::event) fn handle_prune_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.overlays.active = ActiveOverlay::None;
            app.execute_prune();
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.prune.stale.clear();
            app.set_status("Prune cancelled.".to_string(), StatusLevel::Warning);
        }
        _ => {}
    }
    None
}
