//! Overlay handlers — worktree input, cherry-pick, history, resume session,
//! repo selector, open repo, comment detail, help, filename search, grep search,
//! viewer search, review input, review search, review template, switch branch,
//! grab, prune, command palette.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, StatusLevel};
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
                    "Describe your task (Alt+Enter: newline, Enter: generate, Tab: manual mode, Esc: cancel)".to_string(),
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
            KeyCode::Backspace => {
                app.worktree_mgr.input_buffer.delete_backward();
            }
            KeyCode::Delete => {
                app.worktree_mgr.input_buffer.delete_forward();
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                app.worktree_mgr.input_buffer.move_word_left();
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                app.worktree_mgr.input_buffer.move_word_right();
            }
            KeyCode::Left => {
                app.worktree_mgr.input_buffer.move_left();
            }
            KeyCode::Right => {
                app.worktree_mgr.input_buffer.move_right();
            }
            KeyCode::Home => {
                app.worktree_mgr.input_buffer.move_home();
            }
            KeyCode::End => {
                app.worktree_mgr.input_buffer.move_end();
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.worktree_mgr.input_buffer.select_all_and_clear();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clipboard_paste(app, |a| &mut a.worktree_mgr.input_buffer, false);
            }
            KeyCode::Char(c) => {
                app.worktree_mgr.input_buffer.insert_char(c);
            }
            _ => {}
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
                KeyCode::Backspace => {
                    app.worktree_mgr.base_branch_filter.delete_backward();
                    app.worktree_mgr.base_branch_selected = 0;
                }
                KeyCode::Delete => {
                    app.worktree_mgr.base_branch_filter.delete_forward();
                    app.worktree_mgr.base_branch_selected = 0;
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                    app.worktree_mgr.base_branch_filter.move_word_left();
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                    app.worktree_mgr.base_branch_filter.move_word_right();
                }
                KeyCode::Left => {
                    app.worktree_mgr.base_branch_filter.move_left();
                }
                KeyCode::Right => {
                    app.worktree_mgr.base_branch_filter.move_right();
                }
                KeyCode::Home => {
                    app.worktree_mgr.base_branch_filter.move_home();
                }
                KeyCode::End => {
                    app.worktree_mgr.base_branch_filter.move_end();
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.worktree_mgr.base_branch_filter.select_all_and_clear();
                    app.worktree_mgr.base_branch_selected = 0;
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clipboard_paste(app, |a| &mut a.worktree_mgr.base_branch_filter, false);
                    app.worktree_mgr.base_branch_selected = 0;
                }
                KeyCode::Char(c) if key.modifiers.is_empty() => {
                    app.worktree_mgr.base_branch_filter.insert_char(c);
                    app.worktree_mgr.base_branch_selected = 0;
                }
                _ => {}
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
            // Alt+Enter inserts a newline (multi-line editing).
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::ALT) {
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
                KeyCode::Backspace => {
                    app.worktree_mgr.smart_description_buffer.delete_backward();
                }
                KeyCode::Delete => {
                    app.worktree_mgr.smart_description_buffer.delete_forward();
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                    app.worktree_mgr.smart_description_buffer.move_word_left();
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                    app.worktree_mgr.smart_description_buffer.move_word_right();
                }
                KeyCode::Left => {
                    app.worktree_mgr.smart_description_buffer.move_left();
                }
                KeyCode::Right => {
                    app.worktree_mgr.smart_description_buffer.move_right();
                }
                KeyCode::Home => {
                    app.worktree_mgr.smart_description_buffer.move_home();
                }
                KeyCode::End => {
                    app.worktree_mgr.smart_description_buffer.move_end();
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.worktree_mgr.smart_description_buffer.select_all_and_clear();
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clipboard_paste(app, |a| &mut a.worktree_mgr.smart_description_buffer, true);
                }
                KeyCode::Char(c) => {
                    app.worktree_mgr.smart_description_buffer.insert_char(c);
                }
                _ => {}
            }
        }
        WorktreeInputMode::Normal => unreachable!(),
    }
}

// ── Overlay: cherry-pick ────────────────────────────────────────────────

pub(super) fn handle_cherry_pick_key(app: &mut App, key: KeyEvent) {
    let count = app.cherry_pick.commits.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.cherry_pick.selected + 1 < count {
                app.cherry_pick.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.cherry_pick.selected > 0 {
                app.cherry_pick.selected -= 1;
            }
        }
        KeyCode::Enter => {
            app.execute_cherry_pick();
            app.cherry_pick.active = false;
        }
        KeyCode::Esc => {
            app.cherry_pick.active = false;
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
                    .position(|b| *b == app.cherry_pick.source_branch)
                    .unwrap_or(0);
                let next_idx = (cur_idx + 1) % other_branches.len();
                app.cherry_pick.source_branch = other_branches[next_idx].clone();
                app.load_cherry_pick_commits();
            }
        }
        _ => {}
    }
}

// ── Overlay: session history ────────────────────────────────────────────

pub(super) fn handle_history_key(app: &mut App, key: KeyEvent) {
    if app.history.search_active {
        match key.code {
            KeyCode::Enter => {
                app.history.search_active = false;
                app.search_session_history();
            }
            KeyCode::Esc => {
                app.history.search_active = false;
                app.history.search_query.clear();
            }
            KeyCode::Backspace => {
                app.history.search_query.delete_backward();
            }
            KeyCode::Delete => {
                app.history.search_query.delete_forward();
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                app.history.search_query.move_word_left();
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
                app.history.search_query.move_word_right();
            }
            KeyCode::Left => {
                app.history.search_query.move_left();
            }
            KeyCode::Right => {
                app.history.search_query.move_right();
            }
            KeyCode::Home => {
                app.history.search_query.move_home();
            }
            KeyCode::End => {
                app.history.search_query.move_end();
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.history.search_query.select_all_and_clear();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clipboard_paste(app, |a| &mut a.history.search_query, false);
            }
            KeyCode::Char(c) => {
                app.history.search_query.insert_char(c);
            }
            _ => {}
        }
        return;
    }

    let count = app.history.records.len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.history.selected + 1 < count {
                app.history.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.history.selected > 0 {
                app.history.selected -= 1;
            }
        }
        KeyCode::Esc => {
            app.history.active = false;
            app.history.search_query.clear();
            app.history.search_active = false;
        }
        KeyCode::Char('/') => {
            app.history.search_active = true;
            app.history.search_query.clear();
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
            if filtered_count > 0 && app.resume_session.selected + 1 < filtered_count {
                app.resume_session.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.resume_session.selected > 0 {
                app.resume_session.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let filtered = app.filtered_resume_sessions();
            if let Some(&(original_idx, _)) = filtered.get(app.resume_session.selected) {
                let Some(session) = app.resume_session.sessions.get(original_idx).cloned() else {
                    return;
                };
                app.resume_session.active = false;
                app.resume_session.filter.clear();
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
            app.resume_session.active = false;
            app.resume_session.filter.clear();
        }
        KeyCode::Tab => {
            // Toggle between current-repo-only and all-projects mode.
            app.resume_session.all_projects = !app.resume_session.all_projects;
            app.load_resume_sessions();
        }
        KeyCode::Backspace => {
            app.resume_session.filter.delete_backward();
            app.resume_session.selected = 0;
        }
        KeyCode::Delete => {
            app.resume_session.filter.delete_forward();
            app.resume_session.selected = 0;
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.resume_session.filter.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.resume_session.filter.move_word_right();
        }
        KeyCode::Left => {
            app.resume_session.filter.move_left();
        }
        KeyCode::Right => {
            app.resume_session.filter.move_right();
        }
        KeyCode::Home => {
            app.resume_session.filter.move_home();
        }
        KeyCode::End => {
            app.resume_session.filter.move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.resume_session.filter.select_all_and_clear();
            app.resume_session.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.resume_session.filter, false);
            app.resume_session.selected = 0;
        }
        KeyCode::Char(c) if key.modifiers.is_empty() => {
            app.resume_session.filter.insert_char(c);
            app.resume_session.selected = 0;
        }
        _ => {}
    }
}

// ── Overlay: repo selector ──────────────────────────────────────────────

pub(super) fn handle_repo_selector_key(app: &mut App, key: KeyEvent) {
    let count = app.repo_list.len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.repo_selector.selected + 1 < count {
                app.repo_selector.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.repo_selector.selected > 0 {
                app.repo_selector.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let selected = app.repo_selector.selected;
            app.repo_selector.active = false;
            app.switch_repo(selected);
        }
        KeyCode::Esc => {
            app.repo_selector.active = false;
        }
        _ => {}
    }
}

// ── Overlay: open repo path input ───────────────────────────────────────

pub(super) fn handle_open_repo_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.open_repo.active = false;
            app.open_repo.buffer.clear();
        }
        KeyCode::Enter => {
            let buffer = app.open_repo.buffer.text().to_string();
            app.open_repo.active = false;
            app.open_repo.buffer.clear();
            app.open_repo_from_path(&buffer);
        }
        KeyCode::Backspace => {
            app.open_repo.buffer.delete_backward();
        }
        KeyCode::Delete => {
            app.open_repo.buffer.delete_forward();
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.open_repo.buffer.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.open_repo.buffer.move_word_right();
        }
        KeyCode::Left => {
            app.open_repo.buffer.move_left();
        }
        KeyCode::Right => {
            app.open_repo.buffer.move_right();
        }
        KeyCode::Home => {
            app.open_repo.buffer.move_home();
        }
        KeyCode::End => {
            app.open_repo.buffer.move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.open_repo.buffer.select_all_and_clear();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.open_repo.buffer, false);
        }
        KeyCode::Char(c) => {
            app.open_repo.buffer.insert_char(c);
        }
        _ => {}
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
            app.help.active = false;
        }
        // Allow scrolling through help pages by switching context.
        KeyCode::Char('1') => app.help.context = Focus::Worktree,
        KeyCode::Char('2') => app.help.context = Focus::Explorer,
        KeyCode::Char('3') => app.help.context = Focus::Viewer,
        KeyCode::Char('4') => app.help.context = Focus::TerminalClaude,
        _ => {}
    }
}

// ── Overlay: filename search ────────────────────────────────────────────

pub(super) fn handle_filename_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.filename_search_active = false;
            app.viewer_state.filename_search_query.clear();
            app.viewer_state.filename_search_results.clear();
            app.viewer_state.filename_search_selected = 0;
        }
        KeyCode::Enter => {
            if let Some(result) = app
                .viewer_state
                .filename_search_results
                .get(app.viewer_state.filename_search_selected)
                .cloned()
            {
                app.viewer_state.filename_search_active = false;

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
            app.viewer_state.filename_search_query.clear();
            app.viewer_state.filename_search_results.clear();
            app.viewer_state.filename_search_selected = 0;
        }
        KeyCode::Backspace => {
            app.viewer_state.filename_search_query.delete_backward();
            app.viewer_state.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        KeyCode::Delete => {
            app.viewer_state.filename_search_query.delete_forward();
            app.viewer_state.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        KeyCode::Down => {
            let count = app.viewer_state.filename_search_results.len();
            if count > 0 && app.viewer_state.filename_search_selected + 1 < count {
                app.viewer_state.filename_search_selected += 1;
            }
        }
        KeyCode::Up => {
            if app.viewer_state.filename_search_selected > 0 {
                app.viewer_state.filename_search_selected -= 1;
            }
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let count = app.viewer_state.filename_search_results.len();
            if count > 0 && app.viewer_state.filename_search_selected + 1 < count {
                app.viewer_state.filename_search_selected += 1;
            }
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.viewer_state.filename_search_selected > 0 {
                app.viewer_state.filename_search_selected -= 1;
            }
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.viewer_state.filename_search_query.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.viewer_state.filename_search_query.move_word_right();
        }
        KeyCode::Left => {
            app.viewer_state.filename_search_query.move_left();
        }
        KeyCode::Right => {
            app.viewer_state.filename_search_query.move_right();
        }
        KeyCode::Home => {
            app.viewer_state.filename_search_query.move_home();
        }
        KeyCode::End => {
            app.viewer_state.filename_search_query.move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.viewer_state.filename_search_query.select_all_and_clear();
            app.viewer_state.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.viewer_state.filename_search_query, false);
            app.viewer_state.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        KeyCode::Char(c) => {
            app.viewer_state.filename_search_query.insert_char(c);
            app.viewer_state.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        _ => {}
    }
}

// ── Overlay: grep (full-text) search ────────────────────────────────────

pub(super) fn handle_grep_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.grep_search.active = false;
            app.grep_search.running = false;
            app.grep_search.bg_op.clear();
        }
        KeyCode::Enter => {
            if app.grep_search.results.is_empty() || app.grep_search.running {
                // No results yet or still typing — start/restart search.
                app.start_grep_search();
            } else {
                // Jump to the selected result.
                if let Some(result) = app.grep_search.results.get(app.grep_search.selected).cloned() {
                    app.grep_search.active = false;
                    app.grep_search.running = false;
                    app.grep_search.bg_op.clear();

                    if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                        let wt_path = wt.path.clone();
                        app.viewer_state.reveal_file_in_tree(&result.file_path, &wt_path);
                        let tab_width = app.config.viewer.tab_width;
                        app.viewer_state.open_file(&wt_path, &result.file_path, tab_width);
                        app.rehighlight_viewer();
                        app.viewer_state.file_scroll = result.line_number.saturating_sub(1);
                        app.set_focus(Focus::Viewer);
                    }
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let count = app.grep_search.results.len();
            if count > 0 && app.grep_search.selected + 1 < count {
                app.grep_search.selected += 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.grep_search.selected > 0 {
                app.grep_search.selected -= 1;
            }
        }
        KeyCode::Backspace => {
            app.grep_search.query.delete_backward();
        }
        KeyCode::Delete => {
            app.grep_search.query.delete_forward();
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.grep_search.query.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.grep_search.query.move_word_right();
        }
        KeyCode::Left => {
            app.grep_search.query.move_left();
        }
        KeyCode::Right => {
            app.grep_search.query.move_right();
        }
        KeyCode::Home => {
            app.grep_search.query.move_home();
        }
        KeyCode::End => {
            app.grep_search.query.move_end();
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.grep_search.regex_mode = !app.grep_search.regex_mode;
        }
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.grep_search.case_sensitive = !app.grep_search.case_sensitive;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.grep_search.query, false);
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.grep_search.query.select_all_and_clear();
        }
        KeyCode::Char(c) => {
            app.grep_search.query.insert_char(c);
        }
        _ => {}
    }
}

// ── Overlay: viewer search ──────────────────────────────────────────────

pub(super) fn handle_viewer_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.search_active = false;
        }
        KeyCode::Enter => {
            app.viewer_state.search_active = false;
            app.viewer_state.execute_search();
        }
        KeyCode::Backspace => {
            app.viewer_state.search_query.delete_backward();
            app.viewer_state.execute_search();
        }
        KeyCode::Delete => {
            app.viewer_state.search_query.delete_forward();
            app.viewer_state.execute_search();
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.viewer_state.search_query.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.viewer_state.search_query.move_word_right();
        }
        KeyCode::Left => {
            app.viewer_state.search_query.move_left();
        }
        KeyCode::Right => {
            app.viewer_state.search_query.move_right();
        }
        KeyCode::Home => {
            app.viewer_state.search_query.move_home();
        }
        KeyCode::End => {
            app.viewer_state.search_query.move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.viewer_state.search_query.select_all_and_clear();
            app.viewer_state.execute_search();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.viewer_state.search_query, false);
            app.viewer_state.execute_search();
        }
        KeyCode::Char(c) => {
            app.viewer_state.search_query.insert_char(c);
            app.viewer_state.execute_search();
        }
        _ => {}
    }
}

// ── Overlay: review input ───────────────────────────────────────────────

pub(super) fn handle_review_input_key(app: &mut App, key: KeyEvent) {
    // Alt+Enter inserts a newline (multi-line editing).
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::ALT) {
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
        KeyCode::Backspace => {
            app.review_state.input_buffer.delete_backward();
        }
        KeyCode::Delete => {
            app.review_state.input_buffer.delete_forward();
        }
        KeyCode::Tab if app.review_state.input_mode == ReviewInputMode::AddingComment => {
            app.review_state.input_kind = match app.review_state.input_kind {
                CommentKind::Suggest => CommentKind::Question,
                CommentKind::Question => CommentKind::Suggest,
            };
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.review_state.input_buffer.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => {
            app.review_state.input_buffer.move_word_right();
        }
        KeyCode::Left => {
            app.review_state.input_buffer.move_left();
        }
        KeyCode::Right => {
            app.review_state.input_buffer.move_right();
        }
        KeyCode::Home => {
            app.review_state.input_buffer.move_home();
        }
        KeyCode::End => {
            app.review_state.input_buffer.move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.review_state.input_buffer.select_all_and_clear();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.review_state.input_buffer, true);
        }
        KeyCode::Char(c) => {
            app.review_state.input_buffer.insert_char(c);
        }
        _ => {}
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
        KeyCode::Backspace => {
            app.review_state.search_query.delete_backward();
            app.review_state.apply_filter();
        }
        KeyCode::Delete => {
            app.review_state.search_query.delete_forward();
            app.review_state.apply_filter();
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.review_state.search_query.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.review_state.search_query.move_word_right();
        }
        KeyCode::Left => { app.review_state.search_query.move_left(); }
        KeyCode::Right => { app.review_state.search_query.move_right(); }
        KeyCode::Home => { app.review_state.search_query.move_home(); }
        KeyCode::End => { app.review_state.search_query.move_end(); }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.review_state.search_query.select_all_and_clear();
            app.review_state.apply_filter();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.review_state.search_query, false);
            app.review_state.apply_filter();
        }
        KeyCode::Char(c) => {
            app.review_state.search_query.insert_char(c);
            app.review_state.apply_filter();
        }
        _ => {}
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
            if count > 0 && app.switch_branch.selected + 1 < count {
                app.switch_branch.selected += 1;
            }
        }
        KeyCode::Up => {
            if app.switch_branch.selected > 0 {
                app.switch_branch.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let filtered = app.filtered_switch_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.switch_branch.selected) {
                let Some(branch) = app.switch_branch.branches.get(original_idx).cloned() else {
                    return;
                };
                app.switch_branch.active = false;
                app.switch_branch.filter.clear();
                app.create_worktree_from_remote(&branch);
            }
        }
        KeyCode::Esc => {
            app.switch_branch.active = false;
            app.switch_branch.filter.clear();
        }
        KeyCode::Backspace => {
            app.switch_branch.filter.delete_backward();
            app.switch_branch.selected = 0;
        }
        KeyCode::Delete => {
            app.switch_branch.filter.delete_forward();
            app.switch_branch.selected = 0;
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.switch_branch.filter.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.switch_branch.filter.move_word_right();
        }
        KeyCode::Left => { app.switch_branch.filter.move_left(); }
        KeyCode::Right => { app.switch_branch.filter.move_right(); }
        KeyCode::Home => { app.switch_branch.filter.move_home(); }
        KeyCode::End => { app.switch_branch.filter.move_end(); }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.switch_branch.filter.select_all_and_clear();
            app.switch_branch.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.switch_branch.filter, false);
            app.switch_branch.selected = 0;
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            app.switch_branch.filter.insert_char(c);
            app.switch_branch.selected = 0;
        }
        _ => {}
    }
}

// ── Overlay: grab ───────────────────────────────────────────────────────

pub(super) fn handle_grab_key(app: &mut App, key: KeyEvent) {
    let count = app.grab.branches.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.grab.selected + 1 < count {
                app.grab.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.grab.selected > 0 {
                app.grab.selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(branch) = app.grab.branches.get(app.grab.selected).cloned() {
                app.grab.active = false;
                app.execute_grab(&branch);
            }
        }
        KeyCode::Esc => {
            app.grab.active = false;
        }
        _ => {}
    }
}

// ── Overlay: prune ──────────────────────────────────────────────────────

pub(super) fn handle_prune_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.prune.active = false;
            app.execute_prune();
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.prune.active = false;
            app.prune.stale.clear();
            app.set_status("Prune cancelled.".to_string(), StatusLevel::Warning);
        }
        _ => {}
    }
}

// ── Overlay: command palette ─────────────────────────────────────────────

pub(super) fn handle_command_palette_key(app: &mut App, key: KeyEvent) {
    use crate::command_palette;

    let filtered = command_palette::filter_commands(&app.command_palette.filter);
    let count = filtered.len();

    match key.code {
        KeyCode::Down => {
            if count > 0 && app.command_palette.selected + 1 < count {
                app.command_palette.selected += 1;
            }
        }
        KeyCode::Up => {
            if app.command_palette.selected > 0 {
                app.command_palette.selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(scored) = filtered.get(app.command_palette.selected) {
                let id = command_palette::COMMANDS[scored.index].id;
                app.command_palette.active = false;
                app.command_palette.filter.clear();
                app.execute_palette_command(id);
            }
        }
        KeyCode::Esc => {
            app.command_palette.active = false;
            app.command_palette.filter.clear();
        }
        KeyCode::Backspace => {
            app.command_palette.filter.delete_backward();
            app.command_palette.selected = 0;
        }
        KeyCode::Delete => {
            app.command_palette.filter.delete_forward();
            app.command_palette.selected = 0;
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.command_palette.filter.move_word_left();
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.command_palette.filter.move_word_right();
        }
        KeyCode::Left => { app.command_palette.filter.move_left(); }
        KeyCode::Right => { app.command_palette.filter.move_right(); }
        KeyCode::Home => { app.command_palette.filter.move_home(); }
        KeyCode::End => { app.command_palette.filter.move_end(); }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.command_palette.filter.select_all_and_clear();
            app.command_palette.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.command_palette.filter, false);
            app.command_palette.selected = 0;
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            app.command_palette.filter.insert_char(c);
            app.command_palette.selected = 0;
        }
        _ => {}
    }
}
