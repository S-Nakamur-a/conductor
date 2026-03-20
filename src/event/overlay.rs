//! Overlay handlers — worktree input, cherry-pick, history, resume session,
//! repo selector, open repo, comment detail, help, filename search, grep search,
//! viewer search, review input, review search, review template, switch branch,
//! grab, prune, command palette.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, StatusLevel};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;
use crate::review_store::CommentKind;

use super::clipboard_paste;
use super::explorer::submit_new_comment;

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
                app.worktree_mgr.input_buffer.clear();
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
                    let base_ref = if let Some(&(original_idx, _)) = filtered.get(app.worktree_mgr.base_branch_selected) {
                        app.worktree_mgr.base_branch_list.get(original_idx).cloned().unwrap_or_default()
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
                    app.worktree_mgr.base_branch_filter.clear();
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
                        "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
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
                    app.worktree_mgr.smart_description_buffer.clear();
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

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.overlays.cherry_pick.selected + 1 < count {
                app.overlays.cherry_pick.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.overlays.cherry_pick.selected > 0 {
                app.overlays.cherry_pick.selected -= 1;
            }
        }
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
                app.overlays.history.search_query.clear();
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
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.overlays.history.selected + 1 < count {
                app.overlays.history.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.overlays.history.selected > 0 {
                app.overlays.history.selected -= 1;
            }
        }
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

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if filtered_count > 0 && app.overlays.resume_session.selected + 1 < filtered_count {
                app.overlays.resume_session.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.overlays.resume_session.selected > 0 {
                app.overlays.resume_session.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let filtered = app.filtered_resume_sessions();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.resume_session.selected) {
                let Some(session) = app.overlays.resume_session.sessions.get(original_idx).cloned() else {
                    return;
                };
                app.overlays.active = ActiveOverlay::None;
                app.overlays.resume_session.filter.clear();
                app.set_status(format!("Resuming: {}...", session.display.chars().take(40).collect::<String>()), StatusLevel::Info);
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
            app.overlays.resume_session.filter.clear();
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
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.overlays.repo_selector.selected + 1 < count {
                app.overlays.repo_selector.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.overlays.repo_selector.selected > 0 {
                app.overlays.repo_selector.selected -= 1;
            }
        }
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
            app.overlays.open_repo.buffer.clear();
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
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ') => {
            app.review_state.comment_detail_active = false;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if app.review_state.comment_detail_scroll < app.review_state.comment_detail_max_scroll {
                app.review_state.comment_detail_scroll += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.review_state.comment_detail_scroll > 0 {
                app.review_state.comment_detail_scroll -= 1;
            }
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
            // Delete from the detail view.
            let idx = app.review_state.comment_detail_idx;
            app.review_state.selected = idx;
            app.review_state.comment_detail_active = false;
            app.delete_selected_review_comment();
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
            app.viewer_state.filename_search.filename_search_query.clear();
            app.viewer_state.filename_search.filename_search_results.clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Enter => {
            if let Some(result) = app
                .viewer_state
                .filename_search.filename_search_results
                .get(app.viewer_state.filename_search.filename_search_selected)
                .cloned()
            {
                app.viewer_state.filename_search.filename_search_active = false;

                // Reveal and open the selected file (keep Focus on Explorer).
                if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let wt_path = wt.path.clone();
                    app.viewer_state.reveal_file_in_tree(&result.path, &wt_path);
                    let tab_width = app.config.viewer.tab_width;
                    app.viewer_state.open_file(&wt_path, &result.path, tab_width);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&result.path);
                }
            }
            app.viewer_state.filename_search.filename_search_query.clear();
            app.viewer_state.filename_search.filename_search_results.clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer_state.filename_search.filename_search_query.clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Down => {
            let count = app.viewer_state.filename_search.filename_search_results.len();
            if count > 0 && app.viewer_state.filename_search.filename_search_selected + 1 < count {
                app.viewer_state.filename_search.filename_search_selected += 1;
            }
        }
        KeyCode::Up => {
            if app.viewer_state.filename_search.filename_search_selected > 0 {
                app.viewer_state.filename_search.filename_search_selected -= 1;
            }
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let count = app.viewer_state.filename_search.filename_search_results.len();
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
            clipboard_paste(app, |a| &mut a.viewer_state.filename_search.filename_search_query, false);
            app.viewer_state.filename_search.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        _ => {
            if app.viewer_state.filename_search.filename_search_query.handle_key(key) {
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

    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.grep_search.running = false;
            app.overlays.grep_search.bg_op.clear();
            app.overlays.grep_search.bg_op_phase2.clear();
            app.overlays.grep_search.debounce_deadline = None;
            app.overlays.grep_search.phase1_active = false;
        }
        KeyCode::Enter => {
            // Jump to the selected result — only if on a Match row.
            let selected = app.overlays.grep_search.selected;
            let result = app.overlays.grep_search.result_tree.get_match_at(selected).cloned();
            if let Some(result) = result {
                app.overlays.active = ActiveOverlay::None;
                app.overlays.grep_search.running = false;
                app.overlays.grep_search.bg_op.clear();
                app.overlays.grep_search.bg_op_phase2.clear();
                app.overlays.grep_search.debounce_deadline = None;
                app.overlays.grep_search.phase1_active = false;

                if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let wt_path = wt.path.clone();
                    app.viewer_state.reveal_file_in_tree(&result.file_path, &wt_path);
                    let tab_width = app.config.viewer.tab_width;
                    app.viewer_state.open_file(&wt_path, &result.file_path, tab_width);
                    app.rehighlight_viewer();
                    app.viewer_state.content.file_scroll = result.line_number.saturating_sub(1);
                    app.set_focus(Focus::Viewer);
                }
            } else {
                // On a Dir/File row: toggle expand/collapse.
                app.overlays.grep_search.result_tree.toggle_expand(selected);
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let count = app.overlays.grep_search.result_tree.visible_rows().len();
            if count == 0 {
                return;
            }
            let selected = app.overlays.grep_search.selected;
            // If current row is a collapsed dir/file, skip to the next sibling.
            if app.overlays.grep_search.result_tree.is_collapsed(selected) {
                if let Some(next) = app.overlays.grep_search.result_tree.next_sibling_index(selected) {
                    app.overlays.grep_search.selected = next;
                }
            } else if selected + 1 < count {
                app.overlays.grep_search.selected = selected + 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.overlays.grep_search.selected > 0 {
                app.overlays.grep_search.selected -= 1;
            }
        }
        KeyCode::Char('h') if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
            // Collapse the current node (or parent if on a Match row).
            let selected = app.overlays.grep_search.selected;
            let rows = app.overlays.grep_search.result_tree.visible_rows().to_vec();
            match rows.get(selected) {
                Some(SearchTreeRow::Dir { expanded: true, .. }) | Some(SearchTreeRow::File { expanded: true, .. }) => {
                    app.overlays.grep_search.result_tree.collapse(selected);
                }
                Some(SearchTreeRow::Match { depth, .. }) => {
                    // Find parent file/dir row.
                    let d = *depth;
                    for i in (0..selected).rev() {
                        let parent_depth = match &rows[i] {
                            SearchTreeRow::Dir { depth, .. } => Some(*depth),
                            SearchTreeRow::File { depth, .. } => Some(*depth),
                            _ => None,
                        };
                        if let Some(pd) = parent_depth {
                            if pd < d {
                                app.overlays.grep_search.selected = i;
                                app.overlays.grep_search.result_tree.collapse(i);
                                break;
                            }
                        }
                    }
                }
                Some(SearchTreeRow::Dir { expanded: false, .. }) | Some(SearchTreeRow::File { expanded: false, .. }) => {
                    // Already collapsed — move to parent dir.
                    let d = match &rows[selected] {
                        SearchTreeRow::Dir { depth, .. } => *depth,
                        SearchTreeRow::File { depth, .. } => *depth,
                        _ => 0,
                    };
                    if d > 0 {
                        for i in (0..selected).rev() {
                            if let SearchTreeRow::Dir { depth, .. } = &rows[i] {
                                if *depth < d {
                                    app.overlays.grep_search.selected = i;
                                    break;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char('l') if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
            // Expand the current node.
            app.overlays.grep_search.result_tree.expand(app.overlays.grep_search.selected);
        }
        KeyCode::Char('g') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.selected = 0;
            app.overlays.grep_search.scroll = 0;
        }
        KeyCode::Char('G') => {
            let count = app.overlays.grep_search.result_tree.visible_rows().len();
            if count > 0 {
                app.overlays.grep_search.selected = count - 1;
            }
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.grep_search.query.clear();
            app.schedule_grep_search();
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.regex_mode = !app.overlays.grep_search.regex_mode;
            app.schedule_grep_search();
        }
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.case_sensitive = !app.overlays.grep_search.case_sensitive;
            app.schedule_grep_search();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.grep_search.query, false);
            app.schedule_grep_search();
        }
        _ => {
            if app.overlays.grep_search.query.handle_key(key) {
                // Text-modifying keys trigger a new search.
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.schedule_grep_search();
                    }
                    _ => {}
                }
            }
        }
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
            app.viewer_state.search.search_query.clear();
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
    // Shift+Enter inserts a newline (multi-line editing).
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        app.review_state.input_buffer.insert_char('\n');
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.review_state.input_buffer.clear();
            app.review_state.input_mode = ReviewInputMode::Normal;
            app.review_state.status_message = None;
        }
        KeyCode::Enter => {
            let buffer = app.review_state.input_buffer.text().to_string();
            match app.review_state.input_mode {
                ReviewInputMode::AddingComment => {
                    submit_new_comment(app, &buffer);
                }
                ReviewInputMode::EditingComment => {
                    if !buffer.is_empty() {
                        app.update_selected_review_body(&buffer);
                    }
                }
                ReviewInputMode::ReplyingToComment => {
                    if !buffer.is_empty() {
                        app.add_reply_to_selected_comment(&buffer);
                    }
                }
                ReviewInputMode::Normal => unreachable!(),
            }
            app.review_state.input_buffer.clear();
            app.review_state.input_mode = ReviewInputMode::Normal;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.review_state.input_buffer.clear();
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
            app.review_state.search_query.clear();
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

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.review_state.template_selected + 1 < count {
                app.review_state.template_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.review_state.template_selected > 0 {
                app.review_state.template_selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(tmpl) =
                app.review_state.templates.get(app.review_state.template_selected)
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
            if let Some(tmpl) =
                app.review_state.templates.get(app.review_state.template_selected)
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

    match key.code {
        KeyCode::Down => {
            if count > 0 && app.overlays.switch_branch.selected + 1 < count {
                app.overlays.switch_branch.selected += 1;
            }
        }
        KeyCode::Up => {
            if app.overlays.switch_branch.selected > 0 {
                app.overlays.switch_branch.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let filtered = app.filtered_switch_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.switch_branch.selected) {
                let Some(branch) = app.overlays.switch_branch.branches.get(original_idx).cloned() else {
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
            app.overlays.switch_branch.filter.clear();
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
    let count = app.overlays.grab.branches.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.overlays.grab.selected + 1 < count {
                app.overlays.grab.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.overlays.grab.selected > 0 {
                app.overlays.grab.selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(branch) = app.overlays.grab.branches.get(app.overlays.grab.selected).cloned() {
                app.overlays.active = ActiveOverlay::None;
                app.execute_grab(&branch);
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
        }
        _ => {}
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

    let filtered = command_palette::filter_commands(&app.overlays.command_palette.filter);
    let count = filtered.len();

    match key.code {
        KeyCode::Down => {
            if count > 0 && app.overlays.command_palette.selected + 1 < count {
                app.overlays.command_palette.selected += 1;
            }
        }
        KeyCode::Up => {
            if app.overlays.command_palette.selected > 0 {
                app.overlays.command_palette.selected -= 1;
            }
        }
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
            app.overlays.command_palette.filter.clear();
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
