//! Event handling — maps keyboard and mouse events to application actions.
//!
//! Focus-based dispatching: Tab / Shift+Tab cycle between panels.
//! Overlay handlers (worktree input, cherry-pick, etc.) take priority.
//! Terminal-focused panels forward keys to the active PTY session.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, StatusLevel};
use crate::git_engine;
use crate::review_state::ReviewInputMode;
use crate::review_store::{Author, CommentKind};

/// Process a single key event, updating application state as needed.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // ── 1. Overlay handlers — consume ALL keys when active ────────────

    if app.review_state.comment_detail_active {
        handle_comment_detail_key(app, key);
        return;
    }
    if app.review_state.input_mode != ReviewInputMode::Normal {
        handle_review_input_key(app, key);
        return;
    }
    if app.worktree_input_mode != crate::app::WorktreeInputMode::Normal {
        handle_worktree_input_key(app, key);
        return;
    }
    if app.switch_branch_active {
        handle_switch_branch_key(app, key);
        return;
    }
    if app.grab_active {
        handle_grab_key(app, key);
        return;
    }
    if app.prune_active {
        handle_prune_key(app, key);
        return;
    }
    if app.cherry_pick_active {
        handle_cherry_pick_key(app, key);
        return;
    }
    if app.viewer_state.search_active {
        handle_viewer_search_key(app, key);
        return;
    }
    if app.review_state.search_active {
        handle_review_search_key(app, key);
        return;
    }
    if app.review_state.template_picker_active {
        handle_review_template_key(app, key);
        return;
    }
    if app.history_active {
        handle_history_key(app, key);
        return;
    }
    if app.resume_session_active {
        handle_resume_session_key(app, key);
        return;
    }
    if app.repo_selector_active {
        handle_repo_selector_key(app, key);
        return;
    }
    if app.open_repo_active {
        handle_open_repo_key(app, key);
        return;
    }
    if app.help_active {
        handle_help_key(app, key);
        return;
    }
    if app.command_palette_active {
        handle_command_palette_key(app, key);
        return;
    }

    // ── 2. Terminal focus — Ctrl+Esc leaves terminal, everything else → PTY ─

    if app.focus == Focus::TerminalClaude || app.focus == Focus::TerminalShell {
        if key.code == KeyCode::Esc && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.set_focus(Focus::Explorer);
            return;
        }

        // Ctrl+w — jump to Worktree panel.
        if key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.set_focus(Focus::Worktree);
            return;
        }

        // ── Scrollback navigation (Shift+PageUp/PageDown/Home/End) ──
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::PageUp => {
                    let page = match app.focus {
                        Focus::TerminalClaude => app.terminal_size_claude.0 as usize / 2,
                        Focus::TerminalShell => app.terminal_size_shell.0 as usize / 2,
                        _ => unreachable!(),
                    };
                    let scroll = match app.focus {
                        Focus::TerminalClaude => &mut app.terminal_scroll_claude,
                        Focus::TerminalShell => &mut app.terminal_scroll_shell,
                        _ => unreachable!(),
                    };
                    *scroll = scroll.saturating_add(page.max(1));
                    return;
                }
                KeyCode::PageDown => {
                    let page = match app.focus {
                        Focus::TerminalClaude => app.terminal_size_claude.0 as usize / 2,
                        Focus::TerminalShell => app.terminal_size_shell.0 as usize / 2,
                        _ => unreachable!(),
                    };
                    let scroll = match app.focus {
                        Focus::TerminalClaude => &mut app.terminal_scroll_claude,
                        Focus::TerminalShell => &mut app.terminal_scroll_shell,
                        _ => unreachable!(),
                    };
                    *scroll = scroll.saturating_sub(page.max(1));
                    return;
                }
                KeyCode::End => {
                    // Snap to live view.
                    match app.focus {
                        Focus::TerminalClaude => app.terminal_scroll_claude = 0,
                        Focus::TerminalShell => app.terminal_scroll_shell = 0,
                        _ => unreachable!(),
                    }
                    return;
                }
                KeyCode::Home => {
                    // Scroll to top of scrollback buffer.
                    match app.focus {
                        Focus::TerminalClaude => app.terminal_scroll_claude = 1000,
                        Focus::TerminalShell => app.terminal_scroll_shell = 1000,
                        _ => unreachable!(),
                    }
                    return;
                }
                _ => {}
            }
        }

        // Ctrl+p — command palette (intercepted before PTY forward).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            app.command_palette_active = true;
            app.command_palette_filter.clear();
            app.command_palette_selected = 0;
            return;
        }

        // Forward all keys (including Esc, Ctrl+*, Tab) to the active PTY session.
        let session_idx = match app.focus {
            Focus::TerminalClaude => app.active_claude_session,
            Focus::TerminalShell => app.active_shell_session,
            _ => unreachable!(),
        };
        if let Some(idx) = session_idx {
            forward_key_to_pty(app, idx, key);
        } else if key.code == KeyCode::Enter {
            // No active session — Enter spawns a new one.
            spawn_terminal_session(app);
        }
        return;
    }

    // ── 3. Global Ctrl shortcuts (non-terminal panels only) ─────

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            // Ctrl+p — command palette.
            KeyCode::Char('p') => {
                app.command_palette_active = true;
                app.command_palette_filter.clear();
                app.command_palette_selected = 0;
                return;
            }
            // Ctrl+w — jump to Worktree panel.
            KeyCode::Char('w') => {
                app.set_focus(Focus::Worktree);
                return;
            }
            // Ctrl+n — new Claude Code session.
            KeyCode::Char('n') => {
                app.set_status("Starting Claude Code...".to_string(), StatusLevel::Info);
                if let Err(e) = app.spawn_claude_code() {
                    app.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
                    log::warn!("failed to spawn Claude Code session: {e}");
                } else {
                    app.status_message = None;
                }
                if app.focus != Focus::TerminalClaude {
                    app.set_focus(Focus::TerminalClaude);
                }
                return;
            }
            // Ctrl+t — new Shell session.
            KeyCode::Char('t') => {
                app.set_status("Starting shell...".to_string(), StatusLevel::Info);
                if let Err(e) = app.spawn_shell() {
                    app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
                    log::warn!("failed to spawn shell session: {e}");
                } else {
                    app.status_message = None;
                }
                if app.focus != Focus::TerminalShell {
                    app.set_focus(Focus::TerminalShell);
                }
                return;
            }
            // Ctrl+o — open repository path input.
            KeyCode::Char('o') => {
                app.open_repo_active = true;
                app.open_repo_buffer = app.repo_path.display().to_string();
                return;
            }
            // Ctrl+r — repository selector.
            KeyCode::Char('r') => {
                if app.repo_list.len() > 1 {
                    app.repo_selector_active = true;
                    app.repo_selector_selected = app.repo_list_index;
                }
                return;
            }
            // (Ctrl+p is now command palette — handled above)
            _ => {}
        }
    }

    // ── 4. Non-terminal panels — Tab/BackTab cycle focus ─────────────

    match key.code {
        KeyCode::Tab => {
            app.cycle_focus_forward();
            return;
        }
        KeyCode::BackTab => {
            app.cycle_focus_backward();
            return;
        }
        _ => {}
    }

    // ── 5. Non-terminal q/Q quit, ? help ──────────────────────────────

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.quit();
            return;
        }
        KeyCode::Char('?') => {
            app.help_context = app.focus;
            app.help_active = true;
            return;
        }
        KeyCode::Char(':') => {
            app.command_palette_active = true;
            app.command_palette_filter.clear();
            app.command_palette_selected = 0;
            return;
        }
        _ => {}
    }

    // ── 6. Focus-specific keybindings ────────────────────────────────

    match app.focus {
        Focus::Worktree => handle_worktree_key(app, key),
        Focus::Explorer => handle_explorer_key(app, key),
        Focus::Viewer => handle_viewer_key(app, key),
        Focus::TerminalClaude | Focus::TerminalShell => unreachable!(),
    }
}

// ── Worktree panel ──────────────────────────────────────────────────────

fn handle_worktree_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.worktrees.is_empty() {
                app.selected_worktree = (app.selected_worktree + 1) % app.worktrees.len();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !app.worktrees.is_empty() {
                app.selected_worktree = if app.selected_worktree == 0 {
                    app.worktrees.len() - 1
                } else {
                    app.selected_worktree - 1
                };
            }
        }
        KeyCode::Enter => {
            // Confirm worktree selection, refresh all panels, and move to Explorer.
            app.on_worktree_changed();
            app.set_focus(Focus::Explorer);
        }
        KeyCode::Char('w') => {
            // Step 1: enter branch name for new worktree.
            app.worktree_input_mode = crate::app::WorktreeInputMode::CreatingWorktree;
            app.worktree_input_buffer.clear();
            app.set_status("New branch name (Enter to continue, Esc to cancel):".to_string(), StatusLevel::Info);
        }
        KeyCode::Char('X') => {
            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                if wt.is_main {
                    app.set_status("Cannot delete the main worktree.".to_string(), StatusLevel::Error);
                } else {
                    app.worktree_input_mode = crate::app::WorktreeInputMode::ConfirmingDelete;
                    app.set_status(format!("Delete worktree '{}'? (y/n)", wt.branch), StatusLevel::Warning);
                }
            }
        }
        KeyCode::Char('s') => {
            // Switch: show remote branch picker (uses cached refs, no network fetch).
            app.set_status("Loading branches...".to_string(), StatusLevel::Info);
            app.load_switch_branches();
            if !app.switch_branch_list.is_empty() {
                app.switch_branch_active = true;
                app.status_message = None;
            } else if app.status_message.as_ref().is_some_and(|m| m.text == "Loading branches...") {
                app.set_status("No remote branches found.".to_string(), StatusLevel::Warning);
            }
        }
        KeyCode::Char('g') => {
            if app.grabbed_branch.is_some() {
                app.set_status("Already grabbing a branch. Ungrab first (G).".to_string(), StatusLevel::Warning);
            } else {
                app.load_grab_branches();
                if app.grab_branches.is_empty() {
                    app.set_status("No non-main worktrees to grab.".to_string(), StatusLevel::Warning);
                } else {
                    app.grab_active = true;
                }
            }
        }
        KeyCode::Char('G') => {
            if app.grabbed_branch.is_none() {
                app.set_status("Not grabbing — nothing to ungrab.".to_string(), StatusLevel::Warning);
            } else {
                app.worktree_input_mode = crate::app::WorktreeInputMode::ConfirmingUngrab;
                app.set_status("Ungrab? Main will return to main branch. (y/n)".to_string(), StatusLevel::Warning);
            }
        }
        KeyCode::Char('P') => {
            // Prune: find stale worktrees.
            match git_engine::GitEngine::open(&app.repo_path) {
                Ok(engine) => {
                    match engine.find_stale_worktrees() {
                        Ok(stale) => {
                            if stale.is_empty() {
                                app.set_status("No stale worktrees found.".to_string(), StatusLevel::Info);
                            } else {
                                app.prune_stale = stale;
                                app.prune_active = true;
                            }
                        }
                        Err(e) => {
                            app.set_status(format!("Error: {e}"), StatusLevel::Error);
                        }
                    }
                }
                Err(e) => {
                    app.set_status(format!("Error: {e}"), StatusLevel::Error);
                }
            }
        }
        KeyCode::Char('m') => {
            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                if wt.is_main {
                    app.set_status("Cannot merge main into itself.".to_string(), StatusLevel::Error);
                } else {
                    let branch = wt.branch.clone();
                    let main_branch = app.config.general.main_branch.clone();
                    match git_engine::GitEngine::open(&app.repo_path) {
                        Ok(engine) => match engine.merge_into_main(&branch, &main_branch) {
                            Ok(msg) => {
                                app.set_status(msg, StatusLevel::Success);
                                app.refresh_worktrees();
                            }
                            Err(e) => {
                                app.set_status(format!("Merge error: {e}"), StatusLevel::Error);
                            }
                        },
                        Err(e) => {
                            app.set_status(format!("Error: {e}"), StatusLevel::Error);
                        }
                    }
                }
            }
        }
        KeyCode::Char('H') => {
            app.history_active = true;
            app.load_session_history();
        }
        KeyCode::Char('r') => {
            app.refresh_worktrees();
        }
        KeyCode::Char('R') => {
            let main_branch = app.config.general.main_branch.clone();
            match git_engine::GitEngine::open(&app.repo_path) {
                Ok(engine) => match engine.reset_main_to_origin(&main_branch) {
                    Ok(msg) => {
                        app.set_status(msg, StatusLevel::Success);
                        app.refresh_worktrees();
                    }
                    Err(e) => {
                        app.set_status(format!("Reset error: {e}"), StatusLevel::Error);
                    }
                },
                Err(e) => {
                    app.set_status(format!("Error: {e}"), StatusLevel::Error);
                }
            }
        }
        KeyCode::Char('v') => {
            app.open_pr_in_browser();
        }
        KeyCode::Char('p') => {
            let current_branch = app
                .worktrees
                .get(app.selected_worktree)
                .map(|w| w.branch.clone())
                .unwrap_or_default();
            let source = app
                .worktrees
                .iter()
                .find(|w| w.branch != current_branch)
                .map(|w| w.branch.clone());
            if let Some(branch) = source {
                app.cherry_pick_source_branch = branch;
                app.load_cherry_pick_commits();
                app.cherry_pick_active = true;
            } else {
                app.set_status("No other worktree branches available.".to_string(), StatusLevel::Warning);
            }
        }
        _ => {}
    }
}

// ── Explorer panel ──────────────────────────────────────────────────────

fn handle_explorer_key(app: &mut App, key: KeyEvent) {
    if app.viewer_state.file_tree.is_empty() {
        app.refresh_viewer();
    }

    // d — switch focus to diff list.
    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
        app.viewer_state.explorer_show_comments = false;
        app.viewer_state.explorer_focus_on_diff_list = true;
        return;
    }

    // c — switch focus to comment list.
    if key.code == KeyCode::Char('c') && key.modifiers.is_empty() {
        app.viewer_state.explorer_show_comments = true;
        app.viewer_state.explorer_focus_on_diff_list = true;
        return;
    }

    if app.viewer_state.explorer_focus_on_diff_list {
        if app.viewer_state.explorer_show_comments {
            handle_explorer_comment_list_key(app, key);
        } else {
            handle_explorer_diff_list_key(app, key);
        }
        return;
    }

    let visible = app.viewer_state.visible_indices();
    if visible.is_empty() {
        return;
    }

    let cur_vis = visible
        .iter()
        .position(|&i| i == app.viewer_state.tree_selected)
        .unwrap_or(0);

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if cur_vis + 1 < visible.len() {
                app.viewer_state.tree_selected = visible[cur_vis + 1];
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if cur_vis > 0 {
                app.viewer_state.tree_selected = visible[cur_vis - 1];
            }
        }
        KeyCode::Enter => {
            let idx = app.viewer_state.tree_selected;
            if let Some(entry) = app.viewer_state.file_tree.get(idx).cloned() {
                if entry.is_dir {
                    // Lazy-load children before expanding.
                    if !entry.is_expanded {
                        if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                            app.viewer_state.ensure_children_loaded(idx, &wt.path);
                        }
                    }
                    app.viewer_state.toggle_dir(idx);
                } else {
                    // Open the file in the Viewer panel.
                    if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                        let path = wt.path.clone();
                        app.viewer_state.open_file(&path, &entry.path);
                        app.rehighlight_viewer();
                        app.review_state.build_file_comment_cache(&entry.path);
                        app.set_focus(Focus::Viewer);
                    }
                }
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            let idx = app.viewer_state.tree_selected;
            // Lazy-load children before expanding.
            if let Some(entry) = app.viewer_state.file_tree.get(idx) {
                if entry.is_dir && !entry.is_expanded {
                    if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                        app.viewer_state.ensure_children_loaded(idx, &wt.path);
                    }
                }
            }
            app.viewer_state.expand_dir(idx);
        }
        KeyCode::Char('h') | KeyCode::Left => {
            let idx = app.viewer_state.tree_selected;
            app.viewer_state.collapse_dir(idx);
        }
        KeyCode::Char('g') => {
            if let Some(&first) = visible.first() {
                app.viewer_state.tree_selected = first;
            }
        }
        KeyCode::Char('G') => {
            if let Some(&last) = visible.last() {
                app.viewer_state.tree_selected = last;
            }
        }
        _ => {}
    }

    adjust_tree_scroll(app);
}

// ── Explorer: diff list sub-panel ────────────────────────────────────────

fn handle_explorer_diff_list_key(app: &mut App, key: KeyEvent) {
    let count = app.diff_state.display_list.len();

    match key.code {
        KeyCode::Esc => {
            // Return focus to file tree.
            app.viewer_state.explorer_focus_on_diff_list = false;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.viewer_state.diff_list_selected + 1 < count {
                app.viewer_state.diff_list_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.viewer_state.diff_list_selected > 0 {
                app.viewer_state.diff_list_selected -= 1;
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            // Collapse the section the cursor is on.
            let selected = app.viewer_state.diff_list_selected;
            app.diff_state.collapse_section(selected);
            // Clamp selection.
            let new_count = app.diff_state.display_list.len();
            if new_count > 0 && app.viewer_state.diff_list_selected >= new_count {
                app.viewer_state.diff_list_selected = new_count - 1;
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            // Expand the section the cursor is on.
            let selected = app.viewer_state.diff_list_selected;
            app.diff_state.expand_section(selected);
        }
        KeyCode::Enter => {
            let selected = app.viewer_state.diff_list_selected;
            // If on a section header, toggle collapse.
            if app.diff_state.toggle_section(selected) {
                // Clamp selection after rebuild.
                let new_count = app.diff_state.display_list.len();
                if new_count > 0 && app.viewer_state.diff_list_selected >= new_count {
                    app.viewer_state.diff_list_selected = new_count - 1;
                }
            } else if let Some((file_diff, _section)) = app.diff_state.resolve_file(selected) {
                // Open the selected diff file in the Viewer and scroll to first change.
                let file_path = file_diff.path.clone();
                let first_change_line = file_diff.hunks.iter()
                    .flat_map(|h| h.lines.iter())
                    .find(|l| l.tag != crate::diff_state::DiffLineTag::Equal)
                    .and_then(|l| l.new_line_no.or(l.old_line_no));
                if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let wt_path = wt.path.clone();
                    app.viewer_state.open_file(&wt_path, &file_path);
                    if let Some(line) = first_change_line {
                        app.viewer_state.file_scroll = line.saturating_sub(4);
                    }
                    app.viewer_state.reveal_file_in_tree(&file_path, &wt_path);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&file_path);
                    app.set_focus(Focus::Viewer);
                }
            }
        }
        KeyCode::Char('g') => {
            app.viewer_state.diff_list_selected = 0;
        }
        KeyCode::Char('G') => {
            if count > 0 {
                app.viewer_state.diff_list_selected = count - 1;
            }
        }
        _ => {}
    }

    adjust_diff_list_scroll(app);
}

// ── Explorer: comment list sub-panel ──────────────────────────────────────

fn handle_explorer_comment_list_key(app: &mut App, key: KeyEvent) {
    use crate::review_state::CommentListRow;

    let row_count = app.review_state.comment_list_rows.len();

    match key.code {
        KeyCode::Esc => {
            app.viewer_state.explorer_focus_on_diff_list = false;
        }
        KeyCode::Char('x') => {
            // Delete the selected comment (resolve via parent).
            if row_count > 0 {
                app.delete_selected_review_comment();
            }
        }
        KeyCode::Char('r') => {
            // Toggle resolve status (resolve via parent).
            if row_count > 0 {
                app.toggle_selected_review_status();
            }
        }
        KeyCode::Char('e') => {
            // Edit the selected comment (resolve via parent).
            let comment_idx = app
                .review_state
                .selected_comment_idx(app.viewer_state.comment_list_selected);
            if let Some(comment) = comment_idx.and_then(|idx| app.review_state.comments.get(idx)) {
                app.review_state.input_buffer = comment.body.clone();
                app.review_state.input_mode = ReviewInputMode::EditingComment;
                app.review_state.selected = comment_idx.unwrap();
                app.review_state.status_message =
                    Some("Edit comment (Enter to save, Esc to cancel)".to_string());
            }
        }
        KeyCode::Char('R') => {
            // Reply to the selected comment (resolve via parent).
            if row_count > 0 {
                let comment_idx = app
                    .review_state
                    .selected_comment_idx(app.viewer_state.comment_list_selected);
                if let Some(idx) = comment_idx {
                    app.review_state.input_buffer.clear();
                    app.review_state.input_mode = ReviewInputMode::ReplyingToComment;
                    app.review_state.selected = idx;
                    app.review_state.status_message =
                        Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if row_count > 0 && app.viewer_state.comment_list_selected + 1 < row_count {
                app.viewer_state.comment_list_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.viewer_state.comment_list_selected > 0 {
                app.viewer_state.comment_list_selected -= 1;
            }
        }
        KeyCode::Char('g') => {
            app.viewer_state.comment_list_selected = 0;
        }
        KeyCode::Char('G') => {
            if row_count > 0 {
                app.viewer_state.comment_list_selected = row_count - 1;
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            // Collapse the current thread, or move to parent comment if on a reply row.
            let visual = app.viewer_state.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    // Find the parent comment row and move selection there.
                    if let Some(parent_visual) = app
                        .review_state
                        .comment_list_rows
                        .iter()
                        .position(|r| matches!(r, CommentListRow::Comment { comment_idx: ci } if *ci == comment_idx))
                    {
                        app.viewer_state.comment_list_selected = parent_visual;
                    }
                    // Collapse the parent.
                    app.toggle_comment_expansion();
                }
                Some(CommentListRow::Comment { comment_idx }) => {
                    // If expanded, collapse.
                    if let Some(comment) = app.review_state.comments.get(comment_idx) {
                        if app.review_state.expanded_comments.contains(&comment.id) {
                            app.toggle_comment_expansion();
                        }
                    }
                }
                None => {}
            }
        }
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
            let visual = app.viewer_state.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Comment { comment_idx }) => {
                    let has_replies = app
                        .review_state
                        .comments
                        .get(comment_idx)
                        .and_then(|c| app.review_state.reply_counts.get(&c.id))
                        .copied()
                        .unwrap_or(0)
                        > 0;

                    if has_replies {
                        // Toggle expansion.
                        app.toggle_comment_expansion();
                    } else {
                        // No replies — navigate to file.
                        navigate_to_comment(app, comment_idx);
                    }
                }
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    // Navigate to the parent comment's file location.
                    navigate_to_comment(app, comment_idx);
                }
                None => {}
            }
        }
        KeyCode::Char(' ') => {
            // Open comment detail modal.
            let visual = app.viewer_state.comment_list_selected;
            if let Some(comment_idx) = app.review_state.selected_comment_idx(visual) {
                app.review_state.comment_detail_idx = comment_idx;
                app.review_state.comment_detail_scroll = 0;
                app.review_state.comment_detail_active = true;
                // Ensure replies are loaded for the detail view.
                if let Some(comment) = app.review_state.comments.get(comment_idx) {
                    let cid = comment.id.clone();
                    if !app.review_state.cached_replies.contains_key(&cid) {
                        if let Some(store) = app.review_store.as_ref() {
                            if let Ok(replies) = store.get_replies(&cid) {
                                app.review_state.cached_replies.insert(cid, replies);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    // Adjust scroll for comment list.
    let selected = app.viewer_state.comment_list_selected;
    let page_size = app.viewer_state.explorer_diff_list_height.max(1);
    if selected < app.viewer_state.comment_list_scroll {
        app.viewer_state.comment_list_scroll = selected;
    } else if selected >= app.viewer_state.comment_list_scroll + page_size {
        app.viewer_state.comment_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

/// Navigate to the file and line of the comment at the given index.
fn navigate_to_comment(app: &mut App, comment_idx: usize) {
    navigate_to_comment_with_focus(app, comment_idx, true);
}

fn navigate_to_comment_with_focus(app: &mut App, comment_idx: usize, focus_viewer: bool) {
    if let Some(comment) = app.review_state.comments.get(comment_idx) {
        let file_path = comment.file_path.clone();
        let line = comment.line_start as usize;
        if let Some(wt) = app.worktrees.get(app.selected_worktree) {
            let wt_path = wt.path.clone();
            app.viewer_state.open_file(&wt_path, &file_path);
            app.rehighlight_viewer();
            app.viewer_state.file_scroll = line.saturating_sub(1);
            app.viewer_state.selected_line_start = Some(line);
            app.viewer_state.selected_line_end = None;
            app.review_state.build_file_comment_cache(&file_path);
            if focus_viewer {
                app.set_focus(Focus::Viewer);
            }
        }
    }
}

// ── Viewer panel ────────────────────────────────────────────────────────

fn handle_viewer_key(app: &mut App, key: KeyEvent) {
    // Clear comment preview on any key input.
    app.viewer_state.comment_preview_line = None;

    let total = app.viewer_state.file_content.len();

    // Esc goes back to Explorer.
    if key.code == KeyCode::Esc {
        app.viewer_state.clear_selection();
        app.set_focus(Focus::Explorer);
        return;
    }

    if total == 0 {
        return;
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if app.viewer_state.file_scroll + 1 < total {
                app.viewer_state.file_scroll += 1;
            }

        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.viewer_state.file_scroll = app.viewer_state.file_scroll.saturating_sub(1);

        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.viewer_state.file_scroll =
                (app.viewer_state.file_scroll + 15).min(total.saturating_sub(1));

        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.viewer_state.file_scroll = app.viewer_state.file_scroll.saturating_sub(15);

        }
        KeyCode::Char('g') => {
            app.viewer_state.file_scroll = 0;

        }
        KeyCode::Char('G') => {
            app.viewer_state.file_scroll = total.saturating_sub(1);

        }
        KeyCode::Char('/') => {
            app.viewer_state.search_active = true;
            app.viewer_state.search_query.clear();
        }
        KeyCode::Char('n') => {
            app.viewer_state.next_search_match();
        }
        KeyCode::Char('N') => {
            app.viewer_state.prev_search_match();
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.viewer_state.h_scroll = app.viewer_state.h_scroll.saturating_sub(4);
        }
        KeyCode::Char('l') | KeyCode::Right => {
            app.viewer_state.h_scroll += 4;
        }
        KeyCode::Char('0') => {
            app.viewer_state.h_scroll = 0;
        }
        KeyCode::Char('c') => {
            open_viewer_comment(app);
        }
        KeyCode::Char(' ') => {
            // Open comment detail modal for comments on the current line.
            open_viewer_comment_detail(app);
        }
        _ => {}
    }
}

// ── PTY key forwarding ──────────────────────────────────────────────────

fn forward_key_to_pty(app: &mut App, session_idx: usize, key: KeyEvent) {
    let data: Vec<u8> = match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+<letter> → control byte (Ctrl+A = 0x01, etc.)
                let ctrl_byte = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                vec![ctrl_byte]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        _ => return,
    };

    if let Err(e) = app.pty_manager.write_to_session(session_idx, &data) {
        log::warn!("failed to write to PTY session: {e}");
    } else {
        // Snap to live view when the user types into the terminal.
        match app.focus {
            Focus::TerminalClaude => app.terminal_scroll_claude = 0,
            Focus::TerminalShell => app.terminal_scroll_shell = 0,
            _ => {}
        }
        // Clear CC waiting signal when user sends input to a Claude Code session.
        app.clear_cc_waiting_signal(session_idx);
    }
}

// ── Paste event handling ────────────────────────────────────────────────

/// Handle a bracketed paste event. When the terminal panel is focused,
/// forward the entire pasted text to the PTY in one write, wrapped with
/// bracketed-paste escape sequences so the shell/application treats it as
/// a single paste rather than individual keystrokes.
pub fn handle_paste_event(app: &mut App, data: String) {
    if app.focus != Focus::TerminalClaude && app.focus != Focus::TerminalShell {
        // For non-terminal panels, insert the first line into whichever input
        // buffer is active (e.g. worktree input, search, etc.).
        // For now, only handle terminal paste — overlay paste is not common.
        return;
    }

    let session_idx = match app.focus {
        Focus::TerminalClaude => app.active_claude_session,
        Focus::TerminalShell => app.active_shell_session,
        _ => None,
    };

    if let Some(idx) = session_idx {
        // Wrap the paste data with bracketed-paste escape sequences so
        // that the child process (shell, editor, claude-code) knows this
        // is pasted text and will not execute each line individually.
        let mut buf = Vec::with_capacity(data.len() + 12);
        buf.extend_from_slice(b"\x1b[200~");
        buf.extend_from_slice(data.as_bytes());
        buf.extend_from_slice(b"\x1b[201~");

        if let Err(e) = app.pty_manager.write_to_session(idx, &buf) {
            log::warn!("failed to write paste data to PTY session: {e}");
        } else {
            match app.focus {
                Focus::TerminalClaude => app.terminal_scroll_claude = 0,
                Focus::TerminalShell => app.terminal_scroll_shell = 0,
                _ => {}
            }
            app.clear_cc_waiting_signal(idx);
        }
    }
}

// ── Overlay: worktree input ─────────────────────────────────────────────

fn handle_worktree_input_key(app: &mut App, key: KeyEvent) {
    use crate::app::WorktreeInputMode;

    match app.worktree_input_mode {
        WorktreeInputMode::CreatingWorktree => match key.code {
            KeyCode::Esc => {
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.worktree_input_buffer.clear();
                app.status_message = None;
            }
            KeyCode::Enter => {
                let name = app.worktree_input_buffer.clone();
                if name.is_empty() {
                    app.worktree_input_mode = WorktreeInputMode::Normal;
                    app.worktree_input_buffer.clear();
                    app.set_status("Cancelled (empty name).".to_string(), StatusLevel::Warning);
                } else {
                    // Move to step 2: base branch picker.
                    app.worktree_pending_branch = name;
                    app.worktree_input_buffer.clear();
                    app.worktree_input_mode = WorktreeInputMode::CreatingWorktreeBase;
                    app.load_base_branches();
                    app.status_message = None;
                }
            }
            KeyCode::Backspace => {
                app.worktree_input_buffer.pop();
            }
            KeyCode::Char(c) => {
                app.worktree_input_buffer.push(c);
            }
            _ => {}
        },
        WorktreeInputMode::CreatingWorktreeBase => {
            let filtered = app.filtered_base_branches();
            let count = filtered.len();

            match key.code {
                KeyCode::Esc => {
                    app.worktree_input_mode = WorktreeInputMode::Normal;
                    app.base_branch_filter.clear();
                    app.worktree_pending_branch.clear();
                    app.set_status("Cancelled.".to_string(), StatusLevel::Warning);
                }
                KeyCode::Down => {
                    if count > 0 && app.base_branch_selected + 1 < count {
                        app.base_branch_selected += 1;
                    }
                }
                KeyCode::Up => {
                    if app.base_branch_selected > 0 {
                        app.base_branch_selected -= 1;
                    }
                }
                KeyCode::Enter => {
                    let filtered = app.filtered_base_branches();
                    let base_ref = if let Some(&(original_idx, _)) = filtered.get(app.base_branch_selected) {
                        app.base_branch_list.get(original_idx).cloned().unwrap_or_default()
                    } else if !app.base_branch_filter.is_empty() {
                        // No match — use the filter text as a raw ref.
                        app.base_branch_filter.clone()
                    } else {
                        String::new() // Will default to origin/main
                    };
                    let branch_name = app.worktree_pending_branch.clone();
                    app.worktree_input_mode = WorktreeInputMode::Normal;
                    app.base_branch_filter.clear();
                    app.worktree_pending_branch.clear();
                    app.create_worktree_from_base(&branch_name, &base_ref);
                }
                KeyCode::Backspace => {
                    app.base_branch_filter.pop();
                    app.base_branch_selected = 0;
                }
                KeyCode::Char(c) if key.modifiers.is_empty() => {
                    app.base_branch_filter.push(c);
                    app.base_branch_selected = 0;
                }
                _ => {}
            }
        }
        WorktreeInputMode::ConfirmingDelete => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Save branch name before deleting worktree.
                let branch = app.worktrees
                    .get(app.selected_worktree)
                    .map(|w| w.branch.clone())
                    .unwrap_or_default();
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.delete_selected_worktree();
                // After deletion, ask about branch deletion.
                if !branch.is_empty() {
                    app.delete_branch(&branch, true);
                }
            }
            _ => {
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.set_status("Deletion cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingDeleteBranch => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let branch = app.worktree_pending_delete_branch.clone();
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.worktree_pending_delete_branch.clear();
                app.delete_branch(&branch, false);
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                let branch = app.worktree_pending_delete_branch.clone();
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.worktree_pending_delete_branch.clear();
                app.delete_branch(&branch, true);
            }
            _ => {
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.worktree_pending_delete_branch.clear();
                app.set_status("Branch kept.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingUngrab => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.execute_ungrab();
            }
            _ => {
                app.worktree_input_mode = WorktreeInputMode::Normal;
                app.set_status("Ungrab cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::Normal => unreachable!(),
    }
}

// ── Overlay: cherry-pick ────────────────────────────────────────────────

fn handle_cherry_pick_key(app: &mut App, key: KeyEvent) {
    let count = app.cherry_pick_commits.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.cherry_pick_selected + 1 < count {
                app.cherry_pick_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.cherry_pick_selected > 0 {
                app.cherry_pick_selected -= 1;
            }
        }
        KeyCode::Enter => {
            app.execute_cherry_pick();
            app.cherry_pick_active = false;
        }
        KeyCode::Esc => {
            app.cherry_pick_active = false;
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
                    .position(|b| *b == app.cherry_pick_source_branch)
                    .unwrap_or(0);
                let next_idx = (cur_idx + 1) % other_branches.len();
                app.cherry_pick_source_branch = other_branches[next_idx].clone();
                app.load_cherry_pick_commits();
            }
        }
        _ => {}
    }
}

// ── Overlay: session history ────────────────────────────────────────────

fn handle_history_key(app: &mut App, key: KeyEvent) {
    if app.history_search_active {
        match key.code {
            KeyCode::Enter => {
                app.history_search_active = false;
                app.search_session_history();
            }
            KeyCode::Esc => {
                app.history_search_active = false;
                app.history_search_query.clear();
            }
            KeyCode::Backspace => {
                app.history_search_query.pop();
            }
            KeyCode::Char(c) => {
                app.history_search_query.push(c);
            }
            _ => {}
        }
        return;
    }

    let count = app.history_records.len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.history_selected + 1 < count {
                app.history_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.history_selected > 0 {
                app.history_selected -= 1;
            }
        }
        KeyCode::Esc => {
            app.history_active = false;
            app.history_search_query.clear();
            app.history_search_active = false;
        }
        KeyCode::Char('/') => {
            app.history_search_active = true;
            app.history_search_query.clear();
        }
        KeyCode::Char('s') => {
            app.save_current_session_history();
        }
        _ => {}
    }
}

// ── Overlay: resume Claude session ──────────────────────────────────────

fn handle_resume_session_key(app: &mut App, key: KeyEvent) {
    let filtered_count = app.filtered_resume_sessions().len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if filtered_count > 0 && app.resume_session_selected + 1 < filtered_count {
                app.resume_session_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.resume_session_selected > 0 {
                app.resume_session_selected -= 1;
            }
        }
        KeyCode::Enter => {
            let filtered = app.filtered_resume_sessions();
            if let Some(&(original_idx, _)) = filtered.get(app.resume_session_selected) {
                let Some(session) = app.resume_sessions.get(original_idx).cloned() else {
                    return;
                };
                app.resume_session_active = false;
                app.resume_session_filter.clear();
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
            app.resume_session_active = false;
            app.resume_session_filter.clear();
        }
        KeyCode::Tab => {
            // Toggle between current-repo-only and all-projects mode.
            app.resume_session_all_projects = !app.resume_session_all_projects;
            app.load_resume_sessions();
        }
        KeyCode::Backspace => {
            app.resume_session_filter.pop();
            app.resume_session_selected = 0;
        }
        KeyCode::Char(c) if key.modifiers.is_empty() => {
            app.resume_session_filter.push(c);
            app.resume_session_selected = 0;
        }
        _ => {}
    }
}

// ── Overlay: repo selector ──────────────────────────────────────────────

fn handle_repo_selector_key(app: &mut App, key: KeyEvent) {
    let count = app.repo_list.len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.repo_selector_selected + 1 < count {
                app.repo_selector_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.repo_selector_selected > 0 {
                app.repo_selector_selected -= 1;
            }
        }
        KeyCode::Enter => {
            let selected = app.repo_selector_selected;
            app.repo_selector_active = false;
            app.switch_repo(selected);
        }
        KeyCode::Esc => {
            app.repo_selector_active = false;
        }
        _ => {}
    }
}

// ── Overlay: open repo path input ───────────────────────────────────────

fn handle_open_repo_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.open_repo_active = false;
            app.open_repo_buffer.clear();
        }
        KeyCode::Enter => {
            let buffer = app.open_repo_buffer.clone();
            app.open_repo_active = false;
            app.open_repo_buffer.clear();
            app.open_repo_from_path(&buffer);
        }
        KeyCode::Backspace => {
            app.open_repo_buffer.pop();
        }
        KeyCode::Char(c) => {
            app.open_repo_buffer.push(c);
        }
        _ => {}
    }
}

// ── Overlay: comment detail ─────────────────────────────────────────────

fn handle_comment_detail_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ') => {
            app.review_state.comment_detail_active = false;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.review_state.comment_detail_scroll += 1;
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
                app.review_state.input_buffer = comment.body.clone();
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
        _ => {}
    }
}

// ── Overlay: help ───────────────────────────────────────────────────────

fn handle_help_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.help_active = false;
        }
        // Allow scrolling through help pages by switching context.
        KeyCode::Char('1') => app.help_context = Focus::Worktree,
        KeyCode::Char('2') => app.help_context = Focus::Explorer,
        KeyCode::Char('3') => app.help_context = Focus::Viewer,
        KeyCode::Char('4') => app.help_context = Focus::TerminalClaude,
        _ => {}
    }
}

// ── Overlay: viewer search ──────────────────────────────────────────────

fn handle_viewer_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.search_active = false;
        }
        KeyCode::Enter => {
            app.viewer_state.search_active = false;
            app.viewer_state.execute_search();
        }
        KeyCode::Backspace => {
            app.viewer_state.search_query.pop();
            app.viewer_state.execute_search();
        }
        KeyCode::Char(c) => {
            app.viewer_state.search_query.push(c);
            app.viewer_state.execute_search();
        }
        _ => {}
    }
}

// ── Overlay: review input ───────────────────────────────────────────────

fn handle_review_input_key(app: &mut App, key: KeyEvent) {
    // Alt+Enter inserts a newline (multi-line editing).
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::ALT) {
        app.review_state.input_buffer.push('\n');
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.review_state.input_buffer.clear();
            app.review_state.input_mode = ReviewInputMode::Normal;
            app.review_state.status_message = None;
        }
        KeyCode::Enter => {
            let buffer = app.review_state.input_buffer.clone();
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
            app.review_state.input_buffer.pop();
        }
        KeyCode::Tab if app.review_state.input_mode == ReviewInputMode::AddingComment => {
            app.review_state.input_kind = match app.review_state.input_kind {
                CommentKind::Suggest => CommentKind::Question,
                CommentKind::Question => CommentKind::Suggest,
            };
        }
        KeyCode::Char(c) => {
            app.review_state.input_buffer.push(c);
        }
        _ => {}
    }
}

// ── Overlay: review search ──────────────────────────────────────────────

fn handle_review_search_key(app: &mut App, key: KeyEvent) {
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
            app.review_state.search_query.pop();
            app.review_state.apply_filter();
        }
        KeyCode::Char(c) => {
            app.review_state.search_query.push(c);
            app.review_state.apply_filter();
        }
        _ => {}
    }
}

// ── Overlay: review template picker ─────────────────────────────────────

fn handle_review_template_key(app: &mut App, key: KeyEvent) {
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
                app.review_state.input_buffer = tmpl.body.clone();
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
        KeyCode::Char('x') => {
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

fn handle_switch_branch_key(app: &mut App, key: KeyEvent) {
    let filtered = app.filtered_switch_branches();
    let count = filtered.len();

    match key.code {
        KeyCode::Down => {
            if count > 0 && app.switch_branch_selected + 1 < count {
                app.switch_branch_selected += 1;
            }
        }
        KeyCode::Up => {
            if app.switch_branch_selected > 0 {
                app.switch_branch_selected -= 1;
            }
        }
        KeyCode::Enter => {
            let filtered = app.filtered_switch_branches();
            if let Some(&(original_idx, _)) = filtered.get(app.switch_branch_selected) {
                let Some(branch) = app.switch_branch_list.get(original_idx).cloned() else {
                    return;
                };
                app.switch_branch_active = false;
                app.switch_branch_filter.clear();
                app.create_worktree_from_remote(&branch);
            }
        }
        KeyCode::Esc => {
            app.switch_branch_active = false;
            app.switch_branch_filter.clear();
        }
        KeyCode::Backspace => {
            app.switch_branch_filter.pop();
            app.switch_branch_selected = 0;
        }
        KeyCode::Char(c) if key.modifiers.is_empty() => {
            app.switch_branch_filter.push(c);
            app.switch_branch_selected = 0;
        }
        _ => {}
    }
}

// ── Overlay: grab ───────────────────────────────────────────────────────

fn handle_grab_key(app: &mut App, key: KeyEvent) {
    let count = app.grab_branches.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count > 0 && app.grab_selected + 1 < count {
                app.grab_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.grab_selected > 0 {
                app.grab_selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(branch) = app.grab_branches.get(app.grab_selected).cloned() {
                app.grab_active = false;
                app.execute_grab(&branch);
            }
        }
        KeyCode::Esc => {
            app.grab_active = false;
        }
        _ => {}
    }
}

// ── Overlay: prune ──────────────────────────────────────────────────────

fn handle_prune_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.prune_active = false;
            app.execute_prune();
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.prune_active = false;
            app.prune_stale.clear();
            app.set_status("Prune cancelled.".to_string(), StatusLevel::Warning);
        }
        _ => {}
    }
}

// ── Overlay: command palette ─────────────────────────────────────────────

fn handle_command_palette_key(app: &mut App, key: KeyEvent) {
    use crate::command_palette;

    let filtered = command_palette::filter_commands(&app.command_palette_filter);
    let count = filtered.len();

    match key.code {
        KeyCode::Down => {
            if count > 0 && app.command_palette_selected + 1 < count {
                app.command_palette_selected += 1;
            }
        }
        KeyCode::Up => {
            if app.command_palette_selected > 0 {
                app.command_palette_selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(scored) = filtered.get(app.command_palette_selected) {
                let id = command_palette::COMMANDS[scored.index].id;
                app.command_palette_active = false;
                app.command_palette_filter.clear();
                app.execute_palette_command(id);
            }
        }
        KeyCode::Esc => {
            app.command_palette_active = false;
            app.command_palette_filter.clear();
        }
        KeyCode::Backspace => {
            app.command_palette_filter.pop();
            app.command_palette_selected = 0;
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            app.command_palette_filter.push(c);
            app.command_palette_selected = 0;
        }
        _ => {}
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Adjust `tree_scroll` so that `tree_selected` stays visible.
fn adjust_tree_scroll(app: &mut App) {
    let visible = app.viewer_state.visible_indices();
    let cur_vis = visible
        .iter()
        .position(|&i| i == app.viewer_state.tree_selected)
        .unwrap_or(0);

    let page_size = app.viewer_state.explorer_tree_height.max(1);

    if cur_vis < app.viewer_state.tree_scroll {
        app.viewer_state.tree_scroll = cur_vis;
    } else if cur_vis >= app.viewer_state.tree_scroll + page_size {
        app.viewer_state.tree_scroll = cur_vis.saturating_sub(page_size - 1);
    }
}

/// Adjust `diff_list_scroll` so that `diff_list_selected` stays visible.
fn adjust_diff_list_scroll(app: &mut App) {
    let selected = app.viewer_state.diff_list_selected;
    let page_size = app.viewer_state.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.diff_list_scroll {
        app.viewer_state.diff_list_scroll = selected;
    } else if selected >= app.viewer_state.diff_list_scroll + page_size {
        app.viewer_state.diff_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

/// Open the review comment input from the Viewer, pre-filling the location.
fn open_viewer_comment(app: &mut App) {
    let file_path = match app.viewer_state.current_file.clone() {
        Some(p) => p,
        None => return,
    };

    let location = if let Some((start, end)) = app.viewer_state.selected_range() {
        if start == end {
            format!("{file_path}:{start} ")
        } else {
            format!("{file_path}:{start}-{end} ")
        }
    } else {
        let line = app.viewer_state.file_scroll + 1;
        format!("{file_path}:{line} ")
    };

    app.viewer_state.clear_selection();
    app.review_state.input_buffer = location;
    app.review_state.input_kind = CommentKind::Suggest;
    app.review_state.input_mode = ReviewInputMode::AddingComment;
    app.review_state.status_message =
        Some("Add comment: [s:|q:]file:line body".to_string());
}

/// Open the comment detail modal from the Viewer panel for the current line.
fn open_viewer_comment_detail(app: &mut App) {
    // Determine which line the cursor is on (same logic as preview).
    let cursor_line = if let Some((start, _)) = app.viewer_state.selected_range() {
        start
    } else {
        app.viewer_state.file_scroll + 1
    };

    // Find a comment on that line.
    let comments = match app.review_state.file_comments.get(&cursor_line) {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    // Find the index of the first comment in the master comment list.
    let target_id = &comments[0].id;
    let comment_idx = match app.review_state.comments.iter().position(|c| &c.id == target_id) {
        Some(idx) => idx,
        None => return,
    };

    // Load replies if not cached.
    let cid = target_id.clone();
    if !app.review_state.cached_replies.contains_key(&cid) {
        if let Some(store) = app.review_store.as_ref() {
            if let Ok(replies) = store.get_replies(&cid) {
                app.review_state.cached_replies.insert(cid, replies);
            }
        }
    }

    app.review_state.comment_detail_idx = comment_idx;
    app.review_state.comment_detail_scroll = 0;
    app.review_state.comment_detail_active = true;
}

/// Parse the input buffer and add a new review comment.
///
/// Format: `[s:|q:]file_path:line[-end] body_text`
fn submit_new_comment(app: &mut App, input: &str) {
    let input = input.trim();
    if input.is_empty() {
        app.review_state.status_message = Some("Empty input, cancelled.".to_string());
        return;
    }

    let (kind, rest) = if let Some(stripped) = input.strip_prefix("s:") {
        (CommentKind::Suggest, stripped)
    } else if let Some(stripped) = input.strip_prefix("q:") {
        (CommentKind::Question, stripped)
    } else {
        (app.review_state.input_kind, input)
    };

    let Some(space_pos) = rest.find(' ') else {
        app.review_state.status_message =
            Some("Format: file:line body  (e.g. src/main.rs:42 fix this)".to_string());
        return;
    };

    let location = &rest[..space_pos];
    let body = rest[space_pos + 1..].trim();

    if body.is_empty() {
        app.review_state.status_message = Some("Comment body is empty.".to_string());
        return;
    }

    let Some(colon_pos) = location.rfind(':') else {
        app.review_state.status_message =
            Some("Format: file:line body  (e.g. src/main.rs:42 fix this)".to_string());
        return;
    };

    let file_path = &location[..colon_pos];
    let line_part = &location[colon_pos + 1..];

    // Parse line range: "42" or "42-50".
    let (line_start, line_end) = if let Some(dash_pos) = line_part.find('-') {
        let start_str = &line_part[..dash_pos];
        let end_str = &line_part[dash_pos + 1..];
        let Ok(start) = start_str.parse::<u32>() else {
            app.review_state.status_message =
                Some(format!("Invalid line number: '{start_str}'"));
            return;
        };
        let Ok(end) = end_str.parse::<u32>() else {
            app.review_state.status_message =
                Some(format!("Invalid line number: '{end_str}'"));
            return;
        };
        (start, Some(end))
    } else {
        let Ok(line) = line_part.parse::<u32>() else {
            app.review_state.status_message =
                Some(format!("Invalid line number: '{line_part}'"));
            return;
        };
        (line, None)
    };

    app.add_review_comment(file_path, line_start, line_end, kind, body, Author::User);
}

/// Spawn a new terminal session based on the current focus (Claude Code or Shell).
fn spawn_terminal_session(app: &mut App) {
    match app.focus {
        Focus::TerminalClaude => {
            app.set_status("Starting Claude Code...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
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

// ── Mouse event handling ────────────────────────────────────────────────

/// Handle a click on a terminal tab bar.
/// `is_claude` is `true` for Claude panel, `false` for Shell panel.
fn handle_terminal_tab_click(app: &mut App, click_col: u16, tab_area_x: u16, is_claude: bool) {
    // Collect session info (global index + label) to avoid borrow issues.
    let sessions: Vec<(usize, String)> = if is_claude {
        app.current_worktree_claude_sessions()
            .iter()
            .map(|(idx, s)| (*idx, s.label.clone()))
            .collect()
    } else {
        app.current_worktree_shell_sessions()
            .iter()
            .map(|(idx, s)| (*idx, s.label.clone()))
            .collect()
    };

    if sessions.is_empty() {
        return;
    }

    let prefix = if is_claude { "CC" } else { "SH" };

    // Build tab title strings to compute widths (must match render logic).
    // Each tab renders as: "[CC:1] [x]" — short label + " [x]" suffix.
    let tab_titles: Vec<String> = sessions
        .iter()
        .enumerate()
        .map(|(tab_idx, (_, _label))| format!("[{}:{}]", prefix, tab_idx + 1))
        .collect();

    let close_suffix = " [x]"; // 4 chars
    let close_suffix_len = close_suffix.len() as u16;

    let relative_x = click_col.saturating_sub(tab_area_x);

    // Walk through tab titles to find which tab the click falls on.
    let mut x = 0u16;
    for (i, title) in tab_titles.iter().enumerate() {
        let label_width = UnicodeWidthStr::width(title.as_str()) as u16;
        let total_tab_width = label_width + close_suffix_len;
        if relative_x >= x && relative_x < x + total_tab_width {
            let (global_idx, _) = sessions[i];
            // Check if the click falls on the [x] close button area.
            // Only allow closing the currently active session to prevent accidental closes.
            let active_session = if is_claude {
                app.active_claude_session
            } else {
                app.active_shell_session
            };
            let close_start = x + label_width + 1; // +1 for the space before [x]
            if relative_x >= close_start && relative_x < x + total_tab_width {
                if Some(global_idx) == active_session {
                    app.close_terminal_session(global_idx);
                }
                return;
            }
            // Otherwise, activate the session.
            app.pty_manager.activate_session(global_idx);
            if is_claude {
                app.active_claude_session = Some(global_idx);
                app.terminal_scroll_claude = 0;
            } else {
                app.active_shell_session = Some(global_idx);
                app.terminal_scroll_shell = 0;
            }
            return;
        }
        x += total_tab_width;
        x += 1; // divider " "
    }

    // Check [+] tab.
    if relative_x >= x && relative_x < x + 3 {
        if is_claude {
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
            }
        } else if let Err(e) = app.spawn_shell() {
            app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
        }
        return;
    }
    x += 3; // [+]
    x += 1; // divider " "

    // Check [<=>] toggle.
    if relative_x >= x && relative_x < x + 5 {
        let target = if is_claude { Focus::TerminalClaude } else { Focus::TerminalShell };
        if app.expanded_panel.is_some() {
            app.expanded_panel = None;
        } else {
            app.expanded_panel = Some(target);
        }
    }
}

/// Process a single mouse event, updating application state as needed.
pub fn handle_mouse_event(
    app: &mut App,
    mouse: MouseEvent,
    frame_area: ratatui::layout::Rect,
) {
    use ratatui::layout::{Constraint, Layout};

    // Compute layout regions — must match render_ui in main.rs.
    let notif_height: u16 = if !app.cc_waiting_worktrees.is_empty() { 1 } else { 0 };
    let outer = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Length(notif_height), // notification bar
        Constraint::Min(0),
        Constraint::Length(1), // status bar
    ])
    .split(frame_area);
    let notif_area = outer[1];
    let main_area = outer[2];

    let (left_w, explorer_w, viewer_w) = crate::accordion_widths(app.expanded_panel, main_area.width);

    let left_end = main_area.x + left_w;
    let explorer_end = left_end + explorer_w;
    let viewer_end = explorer_end + viewer_w;

    // Compute explorer panel's 50/50 vertical split — must match explorer_panel::render.
    let explorer_v_split = Layout::vertical([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(ratatui::layout::Rect::new(
        left_end,
        main_area.y,
        explorer_w,
        main_area.height,
    ));
    let explorer_mid_y = explorer_v_split[1].y;

    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(app, col, row, main_area, left_end, explorer_end, viewer_end, explorer_mid_y, 3);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(app, col, row, main_area, left_end, explorer_end, viewer_end, explorer_mid_y, -3);
        }
        MouseEventKind::ScrollLeft => {
            // Horizontal scroll — only affects viewer panel.
            if col >= explorer_end && col < viewer_end {
                app.viewer_state.h_scroll = app.viewer_state.h_scroll.saturating_sub(4);
            }
        }
        MouseEventKind::ScrollRight => {
            if col >= explorer_end && col < viewer_end {
                app.viewer_state.h_scroll += 4;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Notification bar click — check for badge clicks.
            if notif_height > 0 && row == notif_area.y {
                for (start_col, end_col, branch) in &app.notification_bar_badges {
                    if col >= *start_col && col < *end_col {
                        if let Some(wt_idx) =
                            app.worktrees.iter().position(|w| w.branch == *branch)
                        {
                            app.selected_worktree = wt_idx;
                            app.on_worktree_changed();
                            app.set_focus(Focus::TerminalClaude);
                        }
                        return;
                    }
                }
                return;
            }

            // Title bar click — ignore.
            if row < main_area.y {
                return;
            }

            // Only handle clicks in the main area.
            if row >= main_area.y && row < main_area.y + main_area.height {
                // Check for [<=>] expand button clicks on the top border row.
                if row == main_area.y {
                    let expand_btn_target = if col < left_end && left_w >= 7 {
                        let btn_start = main_area.x + left_w - 6;
                        let btn_end = main_area.x + left_w - 1;
                        if col >= btn_start && col < btn_end { Some(Focus::Worktree) } else { None }
                    } else if col >= left_end && col < explorer_end && explorer_w >= 7 {
                        let btn_start = left_end + explorer_w - 6;
                        let btn_end = left_end + explorer_w - 1;
                        if col >= btn_start && col < btn_end { Some(Focus::Explorer) } else { None }
                    } else if col >= explorer_end && col < viewer_end && viewer_w >= 7 {
                        let btn_start = explorer_end + viewer_w - 6;
                        let btn_end = explorer_end + viewer_w - 1;
                        if col >= btn_start && col < btn_end { Some(Focus::Viewer) } else { None }
                    } else {
                        None
                    };
                    if let Some(target) = expand_btn_target {
                        if app.expanded_panel == Some(target) {
                            app.expanded_panel = None;
                        } else {
                            app.expanded_panel = Some(target);
                        }
                        return;
                    }
                }

                if col < left_end {
                    // Click selects and switches to the worktree.
                    let relative_row = (row - main_area.y) as usize;
                    let item_row = relative_row.saturating_sub(1); // row 0 is border

                    if !app.worktrees.is_empty() && item_row < app.worktrees.len() {
                        // Clicked on an actual worktree item.
                        app.selected_worktree = item_row;
                        app.on_worktree_changed();
                        app.set_focus(Focus::Explorer);
                    } else {
                        // Clicked on blank space below worktree items.
                        let now = std::time::Instant::now();
                        let elapsed = now.duration_since(app.worktree_blank_last_click);
                        app.worktree_blank_last_click = now;

                        if elapsed.as_millis() < 400 {
                            // Double-click → open worktree creation dialog.
                            app.worktree_input_mode =
                                crate::app::WorktreeInputMode::CreatingWorktree;
                            app.worktree_input_buffer.clear();
                        } else {
                            // Single click → just focus.
                            app.set_focus(Focus::Worktree);
                        }
                    }
                } else if col < explorer_end {
                    // Explorer column.
                    app.set_focus(Focus::Explorer);

                    // Determine if click is in top half (file tree) or bottom half (diff/comment list).
                    if row >= explorer_mid_y {
                        app.viewer_state.explorer_focus_on_diff_list = true;
                        let inner_y = explorer_mid_y + 1; // inside border
                        if row >= inner_y {
                            let click_offset = (row - inner_y) as usize;

                            if app.viewer_state.explorer_show_comments {
                                // Comment list is displayed — handle comment selection.
                                let idx = app.viewer_state.comment_list_scroll + click_offset;
                                let row_count = app.review_state.comment_list_rows.len();
                                if idx < row_count {
                                    app.viewer_state.comment_list_selected = idx;

                                    // Double-click detection.
                                    let now = std::time::Instant::now();
                                    let elapsed = now.duration_since(app.viewer_state.last_comment_click_time);
                                    let is_double = elapsed.as_millis() < 400
                                        && app.viewer_state.last_comment_click_idx == idx;
                                    app.viewer_state.last_comment_click_time = now;
                                    app.viewer_state.last_comment_click_idx = idx;

                                    // Navigate to the comment's file location.
                                    if let Some(comment_idx) =
                                        app.review_state.selected_comment_idx(idx)
                                    {
                                        // Single click: jump to location, keep focus on comments.
                                        // Double click: jump and focus Viewer.
                                        navigate_to_comment_with_focus(app, comment_idx, is_double);
                                    }
                                }
                            } else {
                                // Diff list is displayed — handle diff selection.
                                let idx = app.viewer_state.diff_list_scroll + click_offset;
                                if idx < app.diff_state.display_list.len() {
                                    app.viewer_state.diff_list_selected = idx;
                                    // Single-click: toggle header or open file in Viewer.
                                    if app.diff_state.toggle_section(idx) {
                                        // Toggled a section header.
                                        let new_count = app.diff_state.display_list.len();
                                        if new_count > 0
                                            && app.viewer_state.diff_list_selected >= new_count
                                        {
                                            app.viewer_state.diff_list_selected = new_count - 1;
                                        }
                                    } else if let Some((file_diff, _section)) =
                                        app.diff_state.resolve_file(idx)
                                    {
                                        let file_path = file_diff.path.clone();
                                        let first_change_line = file_diff
                                            .hunks
                                            .iter()
                                            .flat_map(|h| h.lines.iter())
                                            .find(|l| {
                                                l.tag != crate::diff_state::DiffLineTag::Equal
                                            })
                                            .and_then(|l| l.new_line_no.or(l.old_line_no));
                                        if let Some(wt) =
                                            app.worktrees.get(app.selected_worktree)
                                        {
                                            let wt_path = wt.path.clone();
                                            app.viewer_state.open_file(&wt_path, &file_path);
                                            if let Some(line) = first_change_line {
                                                app.viewer_state.file_scroll =
                                                    line.saturating_sub(4);
                                            }
                                            app.viewer_state
                                                .reveal_file_in_tree(&file_path, &wt_path);
                                            app.rehighlight_viewer();
                                            app.review_state
                                                .build_file_comment_cache(&file_path);
                                            app.set_focus(Focus::Viewer);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        app.viewer_state.explorer_focus_on_diff_list = false;
                        // Select the clicked file tree item.
                        let inner_y = main_area.y + 1; // inside border
                        if row >= inner_y {
                            let click_offset = (row - inner_y) as usize;
                            let visible = app.viewer_state.visible_indices();
                            let idx = app.viewer_state.tree_scroll + click_offset;
                            if let Some(&tree_idx) = visible.get(idx) {
                                app.viewer_state.tree_selected = tree_idx;
                                // Single-click opens the file in Viewer (or toggles dir).
                                if let Some(entry) = app.viewer_state.file_tree.get(tree_idx).cloned() {
                                    if entry.is_dir {
                                        // Lazy-load children before expanding.
                                        if !entry.is_expanded {
                                            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                                                app.viewer_state.ensure_children_loaded(tree_idx, &wt.path);
                                            }
                                        }
                                        app.viewer_state.toggle_dir(tree_idx);
                                    } else if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                                        let wt_path = wt.path.clone();
                                        app.viewer_state.open_file(&wt_path, &entry.path);
                                        app.rehighlight_viewer();
                                        app.review_state.build_file_comment_cache(&entry.path);
                                        app.set_focus(Focus::Viewer);
                                    }
                                }
                            }
                        }
                    }
                } else if col < viewer_end {
                    // Viewer column.
                    app.set_focus(Focus::Viewer);

                    // Detect clicks on the gutter (line number area) for line selection.
                    let panel_x = explorer_end;
                    let inner_x = panel_x + 1; // inside border
                    let inner_y = main_area.y + 1; // inside border

                    let total_lines = app.viewer_state.file_content.len();
                    if total_lines > 0 && row >= inner_y {
                        let gutter_width = {
                            let mut count = 0usize;
                            let mut val = total_lines;
                            if val == 0 { 1 } else {
                                while val > 0 { count += 1; val /= 10; }
                                count
                            }
                        };
                        // Gutter: marker(1) + line_num(gutter_width) + " │ "(3) + badge(2)
                        let gutter_end_x = inner_x + (gutter_width as u16) + 6;

                        if col >= inner_x && col < gutter_end_x {
                            let line_offset = (row - inner_y) as usize;
                            let line_1 = app.viewer_state.file_scroll + line_offset + 1;

                            if line_1 <= total_lines {
                                let has_comment = app.review_state.file_comments.contains_key(&line_1);
                                if has_comment {
                                    // Double-click detection for comment lines
                                    let now = std::time::Instant::now();
                                    let elapsed = now.duration_since(app.viewer_state.last_line_click_time);
                                    let is_double = elapsed.as_millis() < 400
                                        && app.viewer_state.last_line_click_line == line_1;
                                    app.viewer_state.last_line_click_time = now;
                                    app.viewer_state.last_line_click_line = line_1;

                                    if is_double {
                                        app.viewer_state.selected_line_start = Some(line_1);
                                        app.viewer_state.selected_line_end = None;
                                        app.viewer_state.comment_preview_line = None;
                                        open_viewer_comment(app);
                                    } else {
                                        // Single click → comment preview only
                                        app.viewer_state.clear_selection();
                                        app.viewer_state.comment_preview_line = Some(line_1);
                                    }
                                } else {
                                    // No comment → existing line selection behavior
                                    app.viewer_state.comment_preview_line = None;
                                    let is_double = app.viewer_state.click_line_number(line_1);
                                    if is_double {
                                        open_viewer_comment(app);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Right column: top 80% = Claude, bottom 20% = Shell.
                    let terminal_split_y =
                        main_area.y + (main_area.height as u32 * 80 / 100) as u16;
                    let terminal_x = viewer_end;

                    if row < terminal_split_y {
                        app.set_focus(Focus::TerminalClaude);
                        // Click on tab bar (first row of Claude panel).
                        if row == main_area.y {
                            handle_terminal_tab_click(app, col, terminal_x, true);
                        } else if app.current_worktree_claude_sessions().is_empty()
                        {
                            spawn_terminal_session(app);
                        }
                    } else {
                        app.set_focus(Focus::TerminalShell);
                        // Click on tab bar (first row of Shell panel).
                        if row == terminal_split_y {
                            handle_terminal_tab_click(app, col, terminal_x, false);
                        } else if app.current_worktree_shell_sessions().is_empty()
                        {
                            spawn_terminal_session(app);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Scroll the panel under the mouse cursor.
#[allow(clippy::too_many_arguments)]
fn handle_mouse_scroll(
    app: &mut App,
    col: u16,
    row: u16,
    main_area: ratatui::layout::Rect,
    left_end: u16,
    explorer_end: u16,
    viewer_end: u16,
    explorer_mid_y: u16,
    delta: i32,
) {
    if row < main_area.y || row >= main_area.y + main_area.height {
        return;
    }

    if col < left_end {
        // Worktree panel scroll.
        if delta > 0 {
            if !app.worktrees.is_empty() {
                app.selected_worktree = (app.selected_worktree + 1)
                    .min(app.worktrees.len().saturating_sub(1));
            }
        } else {
            app.selected_worktree = app.selected_worktree.saturating_sub(1);
        }
    } else if col < explorer_end {
        // Explorer scroll.
        // Determine if scroll is in top half (file tree) or bottom half (diff list).
        if row >= explorer_mid_y {
            // Diff list scroll.
            let file_count = app.diff_state.display_list.len();
            if file_count > 0 {
                if delta > 0 {
                    app.viewer_state.diff_list_scroll = app
                        .viewer_state
                        .diff_list_scroll
                        .saturating_add(delta.unsigned_abs() as usize)
                        .min(file_count.saturating_sub(1));
                } else {
                    app.viewer_state.diff_list_scroll = app
                        .viewer_state
                        .diff_list_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else {
            // File tree scroll.
            let visible_count = app.viewer_state.visible_indices().len();
            let page = app.viewer_state.explorer_tree_height.max(1);
            let max_scroll = visible_count.saturating_sub(page);
            if delta > 0 {
                app.viewer_state.tree_scroll = app
                    .viewer_state
                    .tree_scroll
                    .saturating_add(delta.unsigned_abs() as usize)
                    .min(max_scroll);
            } else {
                app.viewer_state.tree_scroll = app
                    .viewer_state
                    .tree_scroll
                    .saturating_sub(delta.unsigned_abs() as usize);
            }
        }
    } else if col < viewer_end {
        // Viewer scroll.
        let total = app.viewer_state.file_content.len();
        if total > 0 {
            if delta > 0 {
                app.viewer_state.file_scroll = (app.viewer_state.file_scroll
                    + delta.unsigned_abs() as usize)
                    .min(total.saturating_sub(1));
            } else {
                app.viewer_state.file_scroll = app
                    .viewer_state
                    .file_scroll
                    .saturating_sub(delta.unsigned_abs() as usize);
            }
        }
    } else {
        // Terminal panels (right column).
        // Determine split point: top ~80% is Claude, bottom ~20% is Shell.
        let terminal_split_y = main_area.y + (main_area.height * 4 / 5);
        let abs_delta = delta.unsigned_abs() as usize;
        if row < terminal_split_y {
            if delta < 0 {
                // ScrollUp = scroll into history.
                app.terminal_scroll_claude = app.terminal_scroll_claude.saturating_add(abs_delta);
            } else {
                app.terminal_scroll_claude = app.terminal_scroll_claude.saturating_sub(abs_delta);
            }
        } else if delta < 0 {
            app.terminal_scroll_shell = app.terminal_scroll_shell.saturating_add(abs_delta);
        } else {
            app.terminal_scroll_shell = app.terminal_scroll_shell.saturating_sub(abs_delta);
        }
    }
}
