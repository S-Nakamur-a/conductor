//! Overlay handlers — worktree input, cherry-pick, history, resume session,
//! repo selector, open repo, comment detail, help, filename search, grep search,
//! viewer search, review input, review search, review template, switch branch,
//! grab, prune, command palette.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, StatusLevel};
use crate::keymap::{Action, KeyContext, KeyMap};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;
#[allow(unused_imports)]
use crate::review_store::CommentKind;

use super::clipboard_paste;
use super::explorer::submit_new_comment;

// ── Shared overlay list navigation ────────────────────────────────────

/// Handle common list-navigation keys for overlay popups via the keymap.
///
/// Resolves the key against `KeyContext::Overlay` and adjusts `*selected`
/// within `0..count`. Returns `true` if the key was consumed.
fn overlay_list_nav(keymap: &KeyMap, key: &KeyEvent, selected: &mut usize, count: usize) -> bool {
    let Some(action) = keymap.resolve(key, KeyContext::Overlay) else {
        return false;
    };
    apply_list_nav(action, selected, count)
}

/// Would this key event produce a literal character for a text field? A bare
/// printable char (no Ctrl/Alt/Super) is typed input. SHIFT is intentionally
/// NOT disqualifying: `Shift+G` is a literal `G` to type into a filter.
fn is_text_input_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// List navigation for overlays that ALSO have a text filter (command palette,
/// filename search, …). A bare printable key falls through to the filter, so
/// only non-text keys (arrows, PageUp/Down) navigate — `j`/`k`/`g` get typed.
fn filterable_overlay_list_nav(
    keymap: &KeyMap,
    key: &KeyEvent,
    selected: &mut usize,
    count: usize,
) -> bool {
    if is_text_input_key(key) {
        return false;
    }
    overlay_list_nav(keymap, key, selected, count)
}

fn apply_list_nav(action: Action, selected: &mut usize, count: usize) -> bool {
    match action {
        Action::NavigateDown => {
            if count > 0 && *selected + 1 < count {
                *selected += 1;
            }
            true
        }
        Action::NavigateUp => {
            if *selected > 0 {
                *selected -= 1;
            }
            true
        }
        Action::GoToTop => {
            *selected = 0;
            true
        }
        Action::GoToBottom => {
            if count > 0 {
                *selected = count - 1;
            }
            true
        }
        _ => false,
    }
}

// ── Overlay: worktree input ─────────────────────────────────────────────

pub(super) fn handle_worktree_input_key(app: &mut App, key: KeyEvent) {
    use crate::app::WorktreeInputMode;

    match app.worktree_mgr.input_mode {
        WorktreeInputMode::CreatingWorktree => match key.code {
            KeyCode::Esc => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.worktree_mgr.input_buffer.clear();
                app.status_message = None;
            }
            KeyCode::Tab => {
                // Switch to Smart Mode.
                let text = app.worktree_mgr.input_buffer.text().to_string();
                app.worktree_mgr.input_buffer.clear();
                app.worktree_mgr.smart_description_buffer.set_text(&text);
                app.worktree_mgr.input_mode = WorktreeInputMode::SmartDescription;
                app.set_status(
                    "Describe your task (Shift+Enter: newline, Enter: generate, Tab: manual mode, Esc: cancel)".to_string(),
                    StatusLevel::Info,
                );
            }
            KeyCode::Enter => {
                let name = app.worktree_mgr.input_buffer.text().to_string();
                if name.is_empty() {
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.input_buffer.clear();
                    app.set_status("Cancelled (empty name).".to_string(), StatusLevel::Warning);
                } else {
                    // Move to step 2: base branch picker.
                    app.worktree_mgr.pending_branch = name;
                    app.worktree_mgr.input_buffer.clear();
                    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktreeBase;
                    app.load_base_branches();
                    app.status_message = None;
                }
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                app.worktree_mgr.input_buffer.delete_to_line_start();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clipboard_paste(app, |a| &mut a.worktree_mgr.input_buffer, false);
            }
            _ => {
                app.worktree_mgr.input_buffer.handle_key(key);
            }
        },
        WorktreeInputMode::CreatingWorktreeBase => {
            let filtered = app.filtered_base_branches();
            let count = filtered.len();

            match key.code {
                KeyCode::Esc => {
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.base_branch_filter.clear();
                    app.worktree_mgr.pending_branch.clear();
                    app.set_status("Cancelled.".to_string(), StatusLevel::Warning);
                }
                KeyCode::Down => {
                    if count > 0 && app.worktree_mgr.base_branch_selected + 1 < count {
                        app.worktree_mgr.base_branch_selected += 1;
                    }
                }
                KeyCode::Up => {
                    if app.worktree_mgr.base_branch_selected > 0 {
                        app.worktree_mgr.base_branch_selected -= 1;
                    }
                }
                KeyCode::Enter => {
                    let filtered = app.filtered_base_branches();
                    let base_ref = if let Some(&(original_idx, _)) =
                        filtered.get(app.worktree_mgr.base_branch_selected)
                    {
                        app.worktree_mgr
                            .base_branch_list
                            .get(original_idx)
                            .cloned()
                            .unwrap_or_default()
                    } else if !app.worktree_mgr.base_branch_filter.is_empty() {
                        // No match — use the filter text as a raw ref.
                        app.worktree_mgr.base_branch_filter.text().to_string()
                    } else {
                        String::new() // Will default to origin/main
                    };
                    let branch_name = app.worktree_mgr.pending_branch.clone();
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.base_branch_filter.clear();
                    app.worktree_mgr.pending_branch.clear();
                    app.create_worktree_from_base(&branch_name, &base_ref);
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                    app.worktree_mgr.base_branch_filter.delete_to_line_start();
                    app.worktree_mgr.base_branch_selected = 0;
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clipboard_paste(app, |a| &mut a.worktree_mgr.base_branch_filter, false);
                    app.worktree_mgr.base_branch_selected = 0;
                }
                _ => {
                    if app.worktree_mgr.base_branch_filter.handle_key(key) {
                        // Text changed — reset selection for filtering keys.
                        match key.code {
                            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                                app.worktree_mgr.base_branch_selected = 0;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        WorktreeInputMode::ConfirmingDelete => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                // Branch deletion is handled by the completion handler (delete_branch_after = true).
                app.delete_selected_worktree(true);
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.set_status("Deletion cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingDeleteBranch => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let branch = app.worktree_mgr.pending_delete_branch.clone();
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.worktree_mgr.pending_delete_branch.clear();
                app.delete_branch(&branch, false);
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                let branch = app.worktree_mgr.pending_delete_branch.clone();
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.worktree_mgr.pending_delete_branch.clear();
                app.delete_branch(&branch, true);
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.worktree_mgr.pending_delete_branch.clear();
                app.set_status("Branch kept.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingUngrab => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.execute_ungrab();
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.set_status("Ungrab cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingReset => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.perform_reset_main_to_origin();
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.set_status("Reset cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::SmartDescription => {
            // Shift+Enter inserts a newline (multi-line editing).
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
                app.worktree_mgr.smart_description_buffer.insert_char('\n');
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.smart_description_buffer.clear();
                    app.status_message = None;
                }
                KeyCode::Tab => {
                    // Switch back to manual mode.
                    let text = app.worktree_mgr.smart_description_buffer.text().to_string();
                    app.worktree_mgr.smart_description_buffer.clear();
                    app.worktree_mgr.input_buffer.set_text(&text);
                    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
                    app.set_status(
                        "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):"
                            .to_string(),
                        StatusLevel::Info,
                    );
                }
                KeyCode::Enter => {
                    let desc = app.worktree_mgr.smart_description_buffer.trim().to_string();
                    if desc.is_empty() {
                        app.set_status("Description is empty.".to_string(), StatusLevel::Warning);
                    } else {
                        app.start_smart_worktree_async(&desc);
                        app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                        app.worktree_mgr.smart_description_buffer.clear();
                    }
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                    app.worktree_mgr
                        .smart_description_buffer
                        .delete_to_line_start();
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clipboard_paste(app, |a| &mut a.worktree_mgr.smart_description_buffer, true);
                }
                _ => {
                    app.worktree_mgr.smart_description_buffer.handle_key(key);
                }
            }
        }
        WorktreeInputMode::Normal => unreachable!(),
    }
}

// ── Overlay: cherry-pick ────────────────────────────────────────────────

pub(super) fn handle_cherry_pick_key(app: &mut App, key: KeyEvent) {
    let count = app.overlays.cherry_pick.commits.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.cherry_pick.selected,
        count,
    ) {
        return;
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
            // Cycle through source branches.
            let current_branch = app
                .worktrees
                .get(app.selected_worktree)
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
}

// ── Overlay: session history ────────────────────────────────────────────

pub(super) fn handle_history_key(app: &mut App, key: KeyEvent) {
    if app.overlays.history.search_active {
        match key.code {
            KeyCode::Enter => {
                app.overlays.history.search_active = false;
                app.search_session_history();
            }
            KeyCode::Esc => {
                app.overlays.history.search_active = false;
                app.overlays.history.search_query.clear();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                app.overlays.history.search_query.delete_to_line_start();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clipboard_paste(app, |a| &mut a.overlays.history.search_query, false);
            }
            _ => {
                app.overlays.history.search_query.handle_key(key);
            }
        }
        return;
    }

    let count = app.overlays.history.records.len();

    if filterable_overlay_list_nav(&app.keymap, &key, &mut app.overlays.history.selected, count) {
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.history.search_query.clear();
            app.overlays.history.search_active = false;
        }
        KeyCode::Char('/') => {
            app.overlays.history.search_active = true;
            app.overlays.history.search_query.clear();
        }
        KeyCode::Char('s') => {
            app.save_current_session_history();
        }
        _ => {}
    }
}

// ── Overlay: resume Claude session ──────────────────────────────────────

pub(super) fn handle_resume_session_key(app: &mut App, key: KeyEvent) {
    let filtered_count = app.filtered_resume_sessions().len();

    if filterable_overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.resume_session.selected,
        filtered_count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let filtered = app.filtered_resume_sessions();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.resume_session.selected) {
                let Some(session) = app
                    .overlays
                    .resume_session
                    .sessions
                    .get(original_idx)
                    .cloned()
                else {
                    return;
                };
                app.overlays.active = ActiveOverlay::None;
                app.overlays.resume_session.filter.clear();
                app.set_status(
                    format!(
                        "Resuming: {}...",
                        session.display.chars().take(40).collect::<String>()
                    ),
                    StatusLevel::Info,
                );
                match app.resume_claude_session(&session.session_id, &session.display) {
                    Ok(_) => {
                        app.status_message = None;
                        app.set_focus(Focus::TerminalClaude);
                    }
                    Err(e) => {
                        app.set_status(format!("Failed to resume: {e}"), StatusLevel::Error);
                        log::warn!("failed to resume Claude session: {e}");
                    }
                }
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.resume_session.filter.clear();
        }
        KeyCode::Tab => {
            // Toggle between current-repo-only and all-projects mode.
            app.overlays.resume_session.all_projects = !app.overlays.resume_session.all_projects;
            app.load_resume_sessions();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.resume_session.filter.delete_to_line_start();
            app.overlays.resume_session.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.resume_session.filter, false);
            app.overlays.resume_session.selected = 0;
        }
        _ => {
            if app.overlays.resume_session.filter.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.overlays.resume_session.selected = 0;
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Overlay: repo selector ──────────────────────────────────────────────

pub(super) fn handle_repo_selector_key(app: &mut App, key: KeyEvent) {
    let count = app.repo_list.len();

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

// ── Overlay: open repo path input ───────────────────────────────────────

pub(super) fn handle_open_repo_key(app: &mut App, key: KeyEvent) {
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

// ── Overlay: comment detail ─────────────────────────────────────────────

pub(super) fn handle_comment_detail_key(app: &mut App, key: KeyEvent) {
    // Handle scroll navigation via keymap.
    if let Some(action) = app.keymap.resolve(&key, KeyContext::Overlay) {
        match action {
            Action::NavigateDown => {
                if app.review_state.comment_detail_scroll
                    < app.review_state.comment_detail_max_scroll
                {
                    app.review_state.comment_detail_scroll += 1;
                }
                return;
            }
            Action::NavigateUp => {
                if app.review_state.comment_detail_scroll > 0 {
                    app.review_state.comment_detail_scroll -= 1;
                }
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ') => {
            app.review_state.comment_detail_active = false;
        }
        KeyCode::Char('e') => {
            // Edit from the detail view.
            let idx = app.review_state.comment_detail_idx;
            if let Some(comment) = app.review_state.comments.get(idx) {
                app.review_state.input_buffer.set_text(&comment.body);
                app.review_state.input_mode = ReviewInputMode::EditingComment;
                app.review_state.selected = idx;
                app.review_state.comment_detail_active = false;
            }
        }
        KeyCode::Char('R') => {
            // Reply from the detail view.
            let idx = app.review_state.comment_detail_idx;
            app.review_state.input_buffer.clear();
            app.review_state.input_mode = ReviewInputMode::ReplyingToComment;
            app.review_state.selected = idx;
            app.review_state.comment_detail_active = false;
        }
        KeyCode::Delete => {
            // Delete from the detail view (with confirmation).
            let idx = app.review_state.comment_detail_idx;
            app.review_state.comment_detail_active = false;
            if let Some(id) = app.review_state.comments.get(idx).map(|c| c.id.clone()) {
                app.request_delete_comment_by_id(id);
            }
        }
        KeyCode::Char('r') => {
            // Toggle resolve from the detail view.
            let idx = app.review_state.comment_detail_idx;
            app.review_state.selected = idx;
            app.toggle_selected_review_status();
        }
        _ => {}
    }
}

// ── Overlay: help ───────────────────────────────────────────────────────

pub(super) fn handle_help_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.overlays.active = ActiveOverlay::None;
        }
        // Allow scrolling through help pages by switching context.
        KeyCode::Char('1') => app.overlays.help.context = Focus::Worktree,
        KeyCode::Char('2') => app.overlays.help.context = Focus::Explorer,
        KeyCode::Char('3') => app.overlays.help.context = Focus::Viewer,
        KeyCode::Char('4') => app.overlays.help.context = Focus::TerminalClaude,
        _ => {}
    }
}

// ── Overlay: filename search ────────────────────────────────────────────

pub(super) fn handle_filename_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.filename_search.filename_search_active = false;
            app.viewer_state
                .filename_search
                .filename_search_query
                .clear();
            app.viewer_state
                .filename_search
                .filename_search_results
                .clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Enter => {
            if let Some(result) = app
                .viewer_state
                .filename_search
                .filename_search_results
                .get(app.viewer_state.filename_search.filename_search_selected)
                .cloned()
            {
                app.viewer_state.filename_search.filename_search_active = false;

                // Reveal and open the selected file (keep Focus on Explorer).
                if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let wt_path = wt.path.clone();
                    app.viewer_state.reveal_file_in_tree(&result.path, &wt_path);
                    let tab_width = app.config.viewer.tab_width;
                    app.viewer_state
                        .open_file(&wt_path, &result.path, tab_width);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&result.path);
                }
            }
            app.viewer_state
                .filename_search
                .filename_search_query
                .clear();
            app.viewer_state
                .filename_search
                .filename_search_results
                .clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer_state
                .filename_search
                .filename_search_query
                .delete_to_line_start();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        _ if filterable_overlay_list_nav(
            &app.keymap,
            &key,
            &mut app.viewer_state.filename_search.filename_search_selected,
            app.viewer_state
                .filename_search
                .filename_search_results
                .len(),
        ) => {}
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let count = app
                .viewer_state
                .filename_search
                .filename_search_results
                .len();
            if count > 0 && app.viewer_state.filename_search.filename_search_selected + 1 < count {
                app.viewer_state.filename_search.filename_search_selected += 1;
            }
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.viewer_state.filename_search.filename_search_selected > 0 {
                app.viewer_state.filename_search.filename_search_selected -= 1;
            }
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(
                app,
                |a| &mut a.viewer_state.filename_search.filename_search_query,
                false,
            );
            app.viewer_state.filename_search.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        _ => {
            if app
                .viewer_state
                .filename_search
                .filename_search_query
                .handle_key(key)
            {
                // Text-modifying keys reset selection and re-run search.
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.viewer_state.filename_search.filename_search_selected = 0;
                        app.viewer_state.execute_filename_search();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Overlay: grep (full-text) search ────────────────────────────────────

pub(super) fn handle_grep_search_key(app: &mut App, key: KeyEvent) {
    use crate::search_result_tree::SearchTreeRow;

    // ── Keys handled regardless of input/result focus ────────────────
    match key.code {
        KeyCode::Esc => {
            if !app.overlays.grep_search.input_focused {
                // Return focus to input field instead of closing.
                app.overlays.grep_search.input_focused = true;
            } else {
                app.overlays.active = ActiveOverlay::None;
                app.overlays.grep_search.running = false;
                app.overlays.grep_search.bg_op.clear();
                app.overlays.grep_search.bg_op_phase2.clear();
                app.overlays.grep_search.debounce_deadline = None;
                app.overlays.grep_search.phase1_active = false;
            }
            return;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            app.overlays.grep_search.input_focused = !app.overlays.grep_search.input_focused;
            return;
        }
        // Ctrl+r / Ctrl+i / Ctrl+v / Cmd+Backspace — always available.
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.regex_mode = !app.overlays.grep_search.regex_mode;
            app.schedule_grep_search();
            return;
        }
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.case_sensitive = !app.overlays.grep_search.case_sensitive;
            app.schedule_grep_search();
            return;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.grep_search.query, false);
            app.overlays.grep_search.input_focused = true;
            app.schedule_grep_search();
            return;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.grep_search.query.delete_to_line_start();
            app.overlays.grep_search.input_focused = true;
            app.schedule_grep_search();
            return;
        }
        // Arrow Down from input moves focus to results.
        KeyCode::Down if app.overlays.grep_search.input_focused => {
            app.overlays.grep_search.input_focused = false;
            return;
        }
        // Enter — jump to result or toggle expand (works in both modes).
        KeyCode::Enter => {
            let selected = app.overlays.grep_search.selected;
            let result = app
                .overlays
                .grep_search
                .result_tree
                .get_match_at(selected)
                .cloned();
            if let Some(result) = result {
                app.overlays.active = ActiveOverlay::None;
                app.overlays.grep_search.running = false;
                app.overlays.grep_search.bg_op.clear();
                app.overlays.grep_search.bg_op_phase2.clear();
                app.overlays.grep_search.debounce_deadline = None;
                app.overlays.grep_search.phase1_active = false;

                if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let wt_path = wt.path.clone();
                    app.viewer_state
                        .reveal_file_in_tree(&result.file_path, &wt_path);
                    let tab_width = app.config.viewer.tab_width;
                    app.viewer_state
                        .open_file(&wt_path, &result.file_path, tab_width);
                    app.rehighlight_viewer();
                    let hit_0 = result.line_number.saturating_sub(1);
                    let max = app
                        .viewer_state
                        .content
                        .file_content
                        .len()
                        .saturating_sub(1);
                    app.viewer_state.content.file_scroll =
                        result.line_number.saturating_sub(6).min(max);
                    app.viewer_state.content.grep_highlight_line = Some(result.line_number);
                    if app.viewer_state.content.file_scroll > hit_0 {
                        app.viewer_state.content.file_scroll = hit_0;
                    }
                    app.set_focus(Focus::Viewer);
                }
            } else {
                app.overlays.grep_search.result_tree.toggle_expand(selected);
            }
            return;
        }
        _ => {}
    }

    // ── Input-focused mode: all keys go to the text input ────────────
    if app.overlays.grep_search.input_focused {
        if app.overlays.grep_search.query.handle_key(key) {
            match key.code {
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                    app.schedule_grep_search();
                }
                _ => {}
            }
        }
        return;
    }

    // ── Result-focused mode: vim-style navigation ────────────────────
    if let Some(action) = app.keymap.resolve(&key, KeyContext::Overlay) {
        match action {
            Action::NavigateDown => {
                let count = app.overlays.grep_search.result_tree.visible_rows().len();
                if count == 0 {
                    return;
                }
                let selected = app.overlays.grep_search.selected;
                if app.overlays.grep_search.result_tree.is_collapsed(selected) {
                    if let Some(next) = app
                        .overlays
                        .grep_search
                        .result_tree
                        .next_sibling_index(selected)
                    {
                        app.overlays.grep_search.selected = next;
                    }
                } else if selected + 1 < count {
                    app.overlays.grep_search.selected = selected + 1;
                }
                return;
            }
            Action::NavigateUp => {
                if app.overlays.grep_search.selected > 0 {
                    app.overlays.grep_search.selected -= 1;
                }
                return;
            }
            Action::GoToTop => {
                app.overlays.grep_search.selected = 0;
                app.overlays.grep_search.scroll = 0;
                return;
            }
            Action::GoToBottom => {
                let count = app.overlays.grep_search.result_tree.visible_rows().len();
                if count > 0 {
                    app.overlays.grep_search.selected = count - 1;
                }
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Left | KeyCode::Char('h')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            let selected = app.overlays.grep_search.selected;
            let rows = app.overlays.grep_search.result_tree.visible_rows().to_vec();
            match rows.get(selected) {
                Some(SearchTreeRow::Dir { expanded: true, .. })
                | Some(SearchTreeRow::File { expanded: true, .. }) => {
                    app.overlays.grep_search.result_tree.collapse(selected);
                }
                Some(SearchTreeRow::Match { depth, .. }) => {
                    let d = *depth;
                    for i in (0..selected).rev() {
                        let parent_depth = match &rows[i] {
                            SearchTreeRow::Dir { depth, .. } => Some(*depth),
                            SearchTreeRow::File { depth, .. } => Some(*depth),
                            _ => None,
                        };
                        if let Some(pd) = parent_depth
                            && pd < d
                        {
                            app.overlays.grep_search.selected = i;
                            app.overlays.grep_search.result_tree.collapse(i);
                            break;
                        }
                    }
                }
                Some(SearchTreeRow::Dir {
                    expanded: false, ..
                })
                | Some(SearchTreeRow::File {
                    expanded: false, ..
                }) => {
                    let d = match &rows[selected] {
                        SearchTreeRow::Dir { depth, .. } => *depth,
                        SearchTreeRow::File { depth, .. } => *depth,
                        _ => 0,
                    };
                    if d > 0 {
                        for i in (0..selected).rev() {
                            if let SearchTreeRow::Dir { depth, .. } = &rows[i]
                                && *depth < d
                            {
                                app.overlays.grep_search.selected = i;
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Right | KeyCode::Char('l')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.overlays
                .grep_search
                .result_tree
                .expand(app.overlays.grep_search.selected);
        }
        // Any other character key in result-focused mode: switch to input and type.
        KeyCode::Char(_) => {
            app.overlays.grep_search.input_focused = true;
            if app.overlays.grep_search.query.handle_key(key) {
                app.schedule_grep_search();
            }
        }
        KeyCode::Backspace | KeyCode::Delete => {
            app.overlays.grep_search.input_focused = true;
            if app.overlays.grep_search.query.handle_key(key) {
                app.schedule_grep_search();
            }
        }
        _ => {}
    }
}

// ── Overlay: viewer search ──────────────────────────────────────────────

pub(super) fn handle_viewer_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.search.search_active = false;
        }
        KeyCode::Enter => {
            app.viewer_state.search.search_active = false;
            app.viewer_state.execute_search();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer_state.search.search_query.delete_to_line_start();
            app.viewer_state.execute_search();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.viewer_state.search.search_query, false);
            app.viewer_state.execute_search();
        }
        _ => {
            if app.viewer_state.search.search_query.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.viewer_state.execute_search();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Overlay: review input ───────────────────────────────────────────────

pub(super) fn handle_review_input_key(app: &mut App, key: KeyEvent) {
    // Delete confirmation is a y/n prompt, not a text field — handle it first.
    if app.review_state.input_mode == ReviewInputMode::ConfirmingDelete {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                app.confirm_pending_delete();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.cancel_pending_delete();
            }
            _ => {}
        }
        return;
    }

    // Shift+Enter inserts a newline (multi-line editing).
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        app.review_state.input_buffer.insert_char('\n');
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.review_state.input_buffer.clear();
            app.review_state.input_anchor = None;
            app.review_state.editing_reply = None;
            app.review_state.input_mode = ReviewInputMode::Normal;
            app.review_state.status_message = None;
        }
        KeyCode::Enter => {
            let buffer = app.review_state.input_buffer.text().to_string();
            match app.review_state.input_mode {
                ReviewInputMode::AddingComment => {
                    // Inline compose: anchor known, buffer is body-only. Falls
                    // back to the legacy `file:line body` parse when no anchor
                    // (template picker / command palette entry points).
                    if let Some((file, start, end)) = app.review_state.input_anchor.take() {
                        let body = buffer.trim();
                        if body.is_empty() {
                            app.review_state.status_message =
                                Some("Comment body is empty.".to_string());
                        } else {
                            let kind = app.review_state.input_kind;
                            app.add_review_comment(
                                &file,
                                start,
                                end,
                                kind,
                                body,
                                crate::review_store::Author::User,
                            );
                        }
                    } else {
                        submit_new_comment(app, &buffer);
                    }
                }
                ReviewInputMode::EditingComment => {
                    if !buffer.is_empty() {
                        app.update_selected_review_body(&buffer);
                    }
                }
                ReviewInputMode::EditingReply => {
                    if !buffer.is_empty() {
                        app.update_selected_reply_body(&buffer);
                    }
                }
                ReviewInputMode::ReplyingToComment => {
                    if !buffer.is_empty() {
                        app.add_reply_to_selected_comment(&buffer);
                    }
                }
                // ConfirmingDelete is intercepted above; Normal never reaches here.
                ReviewInputMode::Normal | ReviewInputMode::ConfirmingDelete => unreachable!(),
            }
            app.review_state.input_buffer.clear();
            app.review_state.editing_reply = None;
            app.review_state.input_mode = ReviewInputMode::Normal;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.review_state.input_buffer.delete_to_line_start();
        }
        KeyCode::Tab if app.review_state.input_mode == ReviewInputMode::AddingComment => {
            app.review_state.input_kind = match app.review_state.input_kind {
                CommentKind::Suggest => CommentKind::Question,
                CommentKind::Question => CommentKind::Suggest,
            };
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.review_state.input_buffer, true);
        }
        _ => {
            app.review_state.input_buffer.handle_key(key);
        }
    }
}

// ── Overlay: review search ──────────────────────────────────────────────

pub(super) fn handle_review_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.review_state.search_active = false;
            app.review_state.search_query.clear();
            app.review_state.apply_filter();
        }
        KeyCode::Enter => {
            app.review_state.search_active = false;
            app.review_state.apply_filter();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.review_state.search_query.delete_to_line_start();
            app.review_state.apply_filter();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.review_state.search_query, false);
            app.review_state.apply_filter();
        }
        _ => {
            if app.review_state.search_query.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.review_state.apply_filter();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Overlay: review template picker ─────────────────────────────────────

pub(super) fn handle_review_template_key(app: &mut App, key: KeyEvent) {
    let count = app.review_state.templates.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.review_state.template_selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            if let Some(tmpl) = app
                .review_state
                .templates
                .get(app.review_state.template_selected)
            {
                app.review_state.input_buffer.set_text(&tmpl.body);
                app.review_state.input_kind = tmpl.kind;
                app.review_state.input_mode = ReviewInputMode::AddingComment;
                app.review_state.status_message =
                    Some("Template loaded. Prefix with file:line then Enter.".to_string());
            }
            app.review_state.template_picker_active = false;
        }
        KeyCode::Esc => {
            app.review_state.template_picker_active = false;
        }
        KeyCode::Delete => {
            if let Some(tmpl) = app
                .review_state
                .templates
                .get(app.review_state.template_selected)
            {
                let id = tmpl.id.clone();
                app.delete_review_template(&id);
            }
            let new_count = app.review_state.templates.len();
            if new_count == 0 {
                app.review_state.template_picker_active = false;
            } else if app.review_state.template_selected >= new_count {
                app.review_state.template_selected = new_count - 1;
            }
        }
        _ => {}
    }
}

// ── Overlay: switch branch ──────────────────────────────────────────────

pub(super) fn handle_switch_branch_key(app: &mut App, key: KeyEvent) {
    let filtered = app.filtered_switch_branches();
    let count = filtered.len();

    if filterable_overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.switch_branch.selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let filtered = app.filtered_switch_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.switch_branch.selected) {
                let Some(branch) = app
                    .overlays
                    .switch_branch
                    .branches
                    .get(original_idx)
                    .cloned()
                else {
                    return;
                };
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
}

// ── Overlay: grab ───────────────────────────────────────────────────────

pub(super) fn handle_grab_key(app: &mut App, key: KeyEvent) {
    let filtered = app.filtered_grab_branches();
    let count = filtered.len();

    if filterable_overlay_list_nav(&app.keymap, &key, &mut app.overlays.grab.selected, count) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let filtered = app.filtered_grab_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.grab.selected) {
                let Some(branch) = app.overlays.grab.branches.get(original_idx).cloned() else {
                    return;
                };
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
}

// ── Overlay: prune ──────────────────────────────────────────────────────

pub(super) fn handle_prune_key(app: &mut App, key: KeyEvent) {
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
}

// ── Overlay: command palette ─────────────────────────────────────────────

pub(super) fn handle_command_palette_key(app: &mut App, key: KeyEvent) {
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

// ── References overlay ──────────────────────────────────────────────────

pub(super) fn handle_references_key(app: &mut App, key: KeyEvent) {
    let count = app.references_overlay.results.len();
    if count == 0 {
        if key.code == KeyCode::Esc {
            app.references_overlay.active = false;
        }
        return;
    }

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.references_overlay.selected,
        count,
    ) {
        adjust_references_scroll(app);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.references_overlay.active = false;
        }
        KeyCode::Enter => {
            let selected = app.references_overlay.selected;
            if let Some(reference) = app.references_overlay.results.get(selected).cloned() {
                app.references_overlay.active = false;
                app.jump_to_location(&reference.file_path, reference.line, 0);
            }
        }
        _ => {}
    }
}

fn adjust_references_scroll(app: &mut App) {
    let selected = app.references_overlay.selected;
    let scroll = &mut app.references_overlay.scroll;
    // Assume ~20 visible lines in the popup.
    let visible = 20usize;
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible {
        *scroll = selected.saturating_sub(visible - 1);
    }
}

// ── Symbol hint overlay ─────────────────────────────────────────────────

/// Handle key input while the symbol hint overlay is waiting for the second label character.
pub(super) fn handle_symbol_hint_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.symbol_hint_overlay = Default::default();
        }
        KeyCode::Char(c) if c.is_ascii_lowercase() => {
            app.symbol_hint_overlay.input.push(c);
            let input = app.symbol_hint_overlay.input.clone();
            // Find matching hint.
            let matched = app
                .symbol_hint_overlay
                .hints
                .iter()
                .find(|h| h.label == input)
                .cloned();
            // Dismiss hints.
            let scroll = app.viewer_state.content.file_scroll;
            app.symbol_hint_overlay = Default::default();
            if let Some(hint) = matched {
                // Build action overlay for this symbol.
                let screen_row = hint.line.saturating_sub(1).saturating_sub(scroll);
                open_symbol_action_overlay(app, &hint.symbol_name, screen_row);
            }
        }
        _ => {
            app.symbol_hint_overlay = Default::default();
        }
    }
}

/// Build and show the symbol action overlay for the given symbol.
/// `source_screen_row` is the screen row (0-indexed) where the symbol appeared.
fn open_symbol_action_overlay(app: &mut App, symbol_name: &str, source_screen_row: usize) {
    use crate::overlay::{SymbolAction, SymbolActionOverlay};

    let mut actions = Vec::new();

    // Definitions.
    let defs = app.symbol_index.find_definitions(symbol_name);
    if defs.len() == 1 {
        actions.push(SymbolAction {
            key: 'd',
            label: "Go to definition".to_string(),
            file_path: defs[0].file_path.clone(),
            line: defs[0].line,
        });
    } else if defs.len() > 1 {
        actions.push(SymbolAction {
            key: 'd',
            label: format!("Go to definition ({} results)", defs.len()),
            file_path: defs[0].file_path.clone(),
            line: defs[0].line,
        });
    }

    // Implementations.
    let impls = app.symbol_index.find_implementations(symbol_name);
    if impls.len() == 1 {
        actions.push(SymbolAction {
            key: 'i',
            label: "Go to implementation".to_string(),
            file_path: impls[0].file_path.clone(),
            line: impls[0].line,
        });
    } else if impls.len() > 1 {
        actions.push(SymbolAction {
            key: 'i',
            label: format!("Go to implementation ({} results)", impls.len()),
            file_path: impls[0].file_path.clone(),
            line: impls[0].line,
        });
    }

    // References (always show — count requires file scan).
    let root = app.symbol_index.root();
    let refs = app.symbol_index.find_references(symbol_name, &root);
    if !refs.is_empty() {
        actions.push(SymbolAction {
            key: 'r',
            label: format!("Find references ({} refs)", refs.len()),
            file_path: refs[0].file_path.clone(),
            line: refs[0].line,
        });
    }

    if actions.is_empty() {
        app.set_status(
            format!("No navigation targets for '{symbol_name}'"),
            crate::app::StatusLevel::Warning,
        );
        return;
    }

    // Context-aware default selection: if cursor is at the definition site,
    // pre-select "Find references" so pressing Enter goes to references.
    let at_def = app.is_cursor_at_definition(symbol_name);
    let default_idx = if at_def {
        actions.iter().position(|a| a.key == 'r').unwrap_or(0)
    } else {
        0
    };

    app.symbol_action_overlay = SymbolActionOverlay {
        active: true,
        symbol_name: symbol_name.to_string(),
        actions,
        selected: default_idx,
        source_screen_row,
    };
}

// ── Symbol action overlay ───────────────────────────────────────────────

/// Handle key input in the symbol action overlay.
pub(super) fn handle_symbol_action_key(app: &mut App, key: KeyEvent) {
    let count = app.symbol_action_overlay.actions.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.symbol_action_overlay.selected,
        count,
    ) {
        return;
    }

    let symbol = app.symbol_action_overlay.symbol_name.clone();
    let screen_row = app.symbol_action_overlay.source_screen_row;
    match key.code {
        KeyCode::Esc => {
            app.symbol_action_overlay = Default::default();
        }
        KeyCode::Char('d') => {
            app.symbol_action_overlay = Default::default();
            jump_to_symbol_definition(app, &symbol, screen_row);
        }
        KeyCode::Char('i') => {
            app.symbol_action_overlay = Default::default();
            jump_to_symbol_implementation(app, &symbol, screen_row);
        }
        KeyCode::Char('r') => {
            app.symbol_action_overlay = Default::default();
            jump_to_symbol_references(app, &symbol);
        }
        KeyCode::Enter => {
            let idx = app.symbol_action_overlay.selected;
            if let Some(action) = app.symbol_action_overlay.actions.get(idx).cloned() {
                app.symbol_action_overlay = Default::default();
                match action.key {
                    'd' => jump_to_symbol_definition(app, &symbol, screen_row),
                    'i' => jump_to_symbol_implementation(app, &symbol, screen_row),
                    'r' => jump_to_symbol_references(app, &symbol),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn jump_to_symbol_definition(app: &mut App, symbol: &str, screen_row: usize) {
    let defs = app.symbol_index.find_definitions(symbol);
    match defs.len() {
        0 => {
            app.set_status(
                format!("No definition found for '{symbol}'"),
                crate::app::StatusLevel::Warning,
            );
        }
        1 => {
            app.jump_to_location(&defs[0].file_path, defs[0].line, screen_row);
            app.set_status(
                format!("Jumped to definition of '{symbol}' (Ctrl+O to go back)"),
                crate::app::StatusLevel::Success,
            );
        }
        _ => {
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = format!("{symbol} (definitions)");
            app.references_overlay.results = defs
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
        }
    }
}

fn jump_to_symbol_implementation(app: &mut App, symbol: &str, screen_row: usize) {
    let impls = app.symbol_index.find_implementations(symbol);
    match impls.len() {
        0 => {
            app.set_status(
                format!("No implementations found for '{symbol}'"),
                crate::app::StatusLevel::Warning,
            );
        }
        1 => {
            app.jump_to_location(&impls[0].file_path, impls[0].line, screen_row);
            app.set_status(
                format!("Jumped to implementation of '{symbol}' (Ctrl+O to go back)"),
                crate::app::StatusLevel::Success,
            );
        }
        _ => {
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = format!("{symbol} (implementations)");
            app.references_overlay.results = impls
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
        }
    }
}

// ── Overlay: theme picker ────────────────────────────────────────────────

/// Handle keys for the theme picker overlay.
///
/// Up/Down (or j/k) browse the list with live preview — each movement calls
/// `set_theme(name, false)` so the UI updates immediately without persisting.
/// Enter confirms and persists the selected theme; Esc reverts to the theme
/// that was active when the picker was opened.
pub(super) fn handle_theme_picker_key(app: &mut App, key: KeyEvent) {
    let count = app.overlays.theme_picker.themes.len();

    if overlay_list_nav(&app.keymap, &key, &mut app.overlays.theme_picker.selected, count) {
        // Live preview: apply the newly highlighted theme without persisting.
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

fn jump_to_symbol_references(app: &mut App, symbol: &str) {
    let root = app.symbol_index.root();
    let refs = app.symbol_index.find_references(symbol, &root);
    if refs.is_empty() {
        app.set_status(
            format!("No references found for '{symbol}'"),
            crate::app::StatusLevel::Warning,
        );
        return;
    }
    app.references_overlay.active = true;
    app.references_overlay.symbol_name = symbol.to_string();
    app.references_overlay.results = refs;
    app.references_overlay.selected = 0;
    app.references_overlay.scroll = 0;
}

#[cfg(test)]
mod nav_guard_tests {
    use super::is_text_input_key;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn bare_printable_char_is_text_input() {
        // In a filterable overlay these must be typed, not treated as navigation.
        for c in ['j', 'k', 'g', 'G'] {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty());
            assert!(is_text_input_key(&key), "{c:?} should be text input");
        }
        // Shift is not disqualifying: Shift+G is a literal 'G' to type.
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert!(is_text_input_key(&shift_g));
    }

    #[test]
    fn modified_and_named_keys_are_not_text_input() {
        // These should still drive list navigation in a filterable overlay.
        let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        let alt_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        assert!(!is_text_input_key(&ctrl_n));
        assert!(!is_text_input_key(&alt_j));
        assert!(!is_text_input_key(&up));
        assert!(!is_text_input_key(&down));
    }
}
