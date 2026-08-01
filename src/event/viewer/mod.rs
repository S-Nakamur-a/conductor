//! Viewer panel key handling.
//!
//! Entry point ([`handle_viewer_key`]) plus the plain-file, unified-diff, and
//! summary-pseudo-file navigation dispatch. Supporting concerns live in
//! submodules: [`diff_nav`] (pure change-block/comment navigation helpers for
//! the diff view), [`inline_reply`] (inline comment thread toggling and
//! reply composition), and [`code_nav`] (go-to-definition / implementation /
//! references, triggered by the `g` prefix).

mod code_nav;
mod diff_nav;
mod inline_reply;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::keymap::{Action, KeyContext};

use super::explorer::open_viewer_comment_detail;

use code_nav::{handle_find_references, handle_go_to_definition, handle_go_to_implementation};
use diff_nav::{next_change_block, next_comment_line, prev_change_block, prev_comment_line};
use inline_reply::{handle_inline_reply_input, start_inline_reply, toggle_inline_thread};

/// 'g' — show symbol hints and wait for a second key (gd, gi, gr, gg, or a
/// hint label). Shared by the plain-file view and unified-diff view so `g`
/// means the same thing in both; the caller is responsible for syncing
/// `content.file_scroll` to the diff cursor first when in diff mode; hints
/// are built from that same synced position.
fn enter_g_prefix_mode(app: &mut App) {
    app.viewer_state.pending_g_key = true;
    // Build hints using an estimated viewer height (will be clipped by actual content).
    let hints = app.build_symbol_hints(50);
    app.code_nav.symbol_hint.active = !hints.is_empty();
    app.code_nav.symbol_hint.hints = hints;
    app.code_nav.symbol_hint.input.clear();
}

/// Handle keys when the Viewer panel is focused.
pub(super) fn handle_viewer_key(app: &mut App, key: KeyEvent) {
    // ── Inline reply input mode ──────────────────────────────────
    if app.viewer_state.explorer.inline_reply_line.is_some() {
        handle_inline_reply_input(app, key);
        return;
    }

    // Summary pseudo-file view has its own (simple) scroll navigation.
    if app.viewer_state.is_summary() {
        handle_viewer_summary_mode_key(app, key);
        return;
    }

    // Rendered markdown: the prose carries no line numbers, so only whole-view
    // navigation applies. Checked before everything below — including the `g`
    // symbol-hint prefix, which resolves hints by line.
    if app.viewer_state.is_showing_rendered_markdown() {
        handle_viewer_markdown_mode_key(app, key);
        return;
    }

    // ── pending 'g' key — symbol hints are shown, waiting for second key ──
    // Checked before the diff-mode dispatch below since gd/gi/gr/gg apply the
    // same way whether the viewer is showing a plain file or a unified diff.
    if app.viewer_state.pending_g_key {
        app.viewer_state.pending_g_key = false;
        match key.code {
            KeyCode::Char('d') => {
                app.code_nav.symbol_hint = Default::default();
                handle_go_to_definition(app);
                return;
            }
            KeyCode::Char('i') => {
                app.code_nav.symbol_hint = Default::default();
                handle_go_to_implementation(app);
                return;
            }
            KeyCode::Char('r') => {
                app.code_nav.symbol_hint = Default::default();
                handle_find_references(app);
                return;
            }
            // gK / gh — hover info. Uppercase K (Vim-LSP convention) plus `h`
            // for "hover"; both are safe here because hint labels are always
            // lowercase and never start with these carved-out keys. Works in
            // both plain-file and diff mode: the diff-mode `g` handler already
            // synced content.file_scroll to the diff cursor before this.
            KeyCode::Char('K') | KeyCode::Char('h') => {
                app.code_nav.symbol_hint = Default::default();
                app.show_hover_info();
                return;
            }
            KeyCode::Char('g') => {
                // gg = go to top
                app.code_nav.symbol_hint = Default::default();
                if app.viewer_state.diff_view.diff_mode {
                    app.viewer_state.diff_view.diff_view_scroll = 0;
                } else {
                    app.viewer_state.content.file_scroll = 0;
                }
                return;
            }
            KeyCode::Esc => {
                app.code_nav.symbol_hint = Default::default();
                return;
            }
            KeyCode::Char(c) if c.is_ascii_lowercase() => {
                // First character of a hint label — enter hint input mode.
                app.code_nav.symbol_hint.input.push(c);
                return;
            }
            _ => {
                // Unknown second key — dismiss hints.
                app.code_nav.symbol_hint = Default::default();
            }
        }
    }

    // Unified diff mode has its own navigation.
    if app.viewer_state.diff_view.diff_mode {
        handle_viewer_diff_mode_key(app, key);
        return;
    }

    let total = app.viewer_state.content.file_content.len();
    let action = app.keymap.resolve(&key, KeyContext::Viewer);

    if let Some(Action::ExitToExplorer) = action {
        if app.viewer_state.selection != crate::viewer::LineSelection::None {
            app.viewer_state.clear_selection();
        } else {
            app.set_focus(crate::app::Focus::Explorer);
        }
        return;
    }

    // Fuzzy filename jump — handled before the empty-buffer guard so it works
    // even when no file is open, and keeps the viewer maximized after jumping.
    if let Some(Action::SearchFilename) = action {
        super::open_filename_search(app);
        return;
    }

    // Hand off to an external editor — before the empty-buffer guard so a
    // missing file flashes a hint instead of silently doing nothing.
    if let Some(Action::OpenInEditor) = action {
        app.open_in_editor();
        return;
    }

    if total == 0 {
        return;
    }

    match action {
        Some(Action::NavigateDown) if app.viewer_state.content.file_scroll + 1 < total => {
            app.viewer_state.content.file_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.content.file_scroll =
                app.viewer_state.content.file_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.content.file_scroll =
                (app.viewer_state.content.file_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.content.file_scroll =
                app.viewer_state.content.file_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => enter_g_prefix_mode(app),
        Some(Action::GoToBottom) => {
            app.viewer_state.content.file_scroll = total.saturating_sub(1);
        }
        Some(Action::SearchInFile) => {
            app.viewer_state.search.search_active = true;
            app.viewer_state.search.search_query.clear();
        }
        Some(Action::NextSearchMatch) => {
            app.viewer_state.next_search_match();
        }
        Some(Action::PrevSearchMatch) => {
            app.viewer_state.prev_search_match();
        }
        Some(Action::ScrollLeft) => {
            app.viewer_state.content.h_scroll = app.viewer_state.content.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer_state.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer_state.content.h_scroll = 0;
        }
        Some(Action::ToggleInlineThread) => {
            toggle_inline_thread(app);
        }
        Some(Action::InlineReply) => {
            start_inline_reply(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        Some(Action::AddComment) => {
            app.cmd_add_review_comment();
        }
        Some(Action::JumpBack) => {
            app.jump_back();
        }
        Some(Action::JumpForward) => {
            app.jump_forward();
        }
        Some(Action::ShowHoverInfo) => {
            app.show_hover_info();
        }
        Some(Action::ToggleMarkdownRender) => {
            app.cmd_toggle_markdown_render();
        }
        _ => {}
    }
}

/// Navigate the rendered-markdown view: scroll, jump to ends, switch back to
/// raw, or leave the panel.
///
/// Deliberately an allowlist rather than the full viewer dispatch: every action
/// omitted here (comment creation, inline threads, in-file search, hover/symbol
/// jumps, horizontal scroll) addresses content by *source line*, and rendered
/// prose has no line numbers to address. Resolution still uses the ordinary
/// `Viewer` context so the toggle keeps one binding across both modes.
pub(super) fn handle_viewer_markdown_mode_key(app: &mut App, key: KeyEvent) {
    let total = app.viewer_state.md_total_lines;
    let action = app.keymap.resolve(&key, KeyContext::Viewer);

    match action {
        Some(Action::ToggleMarkdownRender) => app.cmd_toggle_markdown_render(),
        Some(Action::ExitToExplorer) => app.set_focus(crate::app::Focus::Explorer),
        Some(Action::SearchFilename) => super::open_filename_search(app),
        // File-level, not line-level: handing the file to $EDITOR is just as
        // meaningful from the rendered view.
        Some(Action::OpenInEditor) => app.open_in_editor(),
        Some(Action::NavigateDown) if app.viewer_state.md_scroll + 1 < total => {
            app.viewer_state.md_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.md_scroll = app.viewer_state.md_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.md_scroll =
                (app.viewer_state.md_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.md_scroll = app.viewer_state.md_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => app.viewer_state.md_scroll = 0,
        Some(Action::GoToBottom) => {
            app.viewer_state.md_scroll = total.saturating_sub(1);
        }
        _ => {}
    }
}

/// Key handling for the viewer panel in unified diff mode.
/// Navigate the summary pseudo-file view: scroll, jump to ends, or exit back to
/// the Explorer. Reuses the diff-mode key context so j/k/d/u/g/G/Esc behave the
/// same as everywhere else.
pub(super) fn handle_viewer_summary_mode_key(app: &mut App, key: KeyEvent) {
    let total = app.viewer_state.summary_total_lines;
    let action = app.keymap.resolve(&key, KeyContext::ViewerDiffMode);

    match action {
        Some(Action::ExitToExplorer) => {
            app.viewer_state.exit_diff_mode(); // also clears show_summary
            app.set_focus(crate::app::Focus::Explorer);
        }
        Some(Action::SearchFilename) => super::open_filename_search(app),
        Some(Action::NavigateDown) if app.viewer_state.summary_scroll + 1 < total => {
            app.viewer_state.summary_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.summary_scroll = app.viewer_state.summary_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.summary_scroll =
                (app.viewer_state.summary_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.summary_scroll = app.viewer_state.summary_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => app.viewer_state.summary_scroll = 0,
        Some(Action::GoToBottom) => {
            app.viewer_state.summary_scroll = total.saturating_sub(1);
        }
        _ => {}
    }
}

pub(super) fn handle_viewer_diff_mode_key(app: &mut App, key: KeyEvent) {
    let total = app.viewer_state.diff_view.diff_view_lines.len();
    let action = app.keymap.resolve(&key, KeyContext::ViewerDiffMode);

    if let Some(Action::ExitToExplorer) = action {
        if app.viewer_state.selection != crate::viewer::LineSelection::None {
            app.viewer_state.clear_selection();
        } else {
            app.viewer_state.exit_diff_mode();
            app.set_focus(crate::app::Focus::Explorer);
        }
        return;
    }

    // Fuzzy filename jump — also reachable from the maximized diff viewer.
    if let Some(Action::SearchFilename) = action {
        super::open_filename_search(app);
        return;
    }

    // Jump to the next/previous changed file (GitHub-style "next file").
    if let Some(Action::NextChangedFile) = action {
        app.jump_to_changed_file(true);
        return;
    }
    if let Some(Action::PrevChangedFile) = action {
        app.jump_to_changed_file(false);
        return;
    }

    // Hand off the file under review to an external editor for a quick manual
    // fix — before the empty-buffer guard, so it also works on an empty diff.
    if let Some(Action::OpenInEditor) = action {
        app.open_in_editor();
        return;
    }

    if total == 0 {
        return;
    }

    match action {
        Some(Action::NavigateDown) if app.viewer_state.diff_view.diff_view_scroll + 1 < total => {
            app.viewer_state.diff_view.diff_view_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.diff_view.diff_view_scroll = app
                .viewer_state
                .diff_view
                .diff_view_scroll
                .saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.diff_view.diff_view_scroll =
                (app.viewer_state.diff_view.diff_view_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.diff_view.diff_view_scroll = app
                .viewer_state
                .diff_view
                .diff_view_scroll
                .saturating_sub(15);
        }
        Some(Action::GoToTop) => {
            // 'g' now matches the plain-file view's symbol-hint prefix (gd,
            // gi, gr, gg, or a hint label) instead of jumping to the top
            // directly — the previous single-`g`-jumps-to-top behavior moved
            // to `gg` so `g` means the same thing in both views. Sync
            // `content.file_scroll` to the line under the diff cursor first,
            // since symbol lookup and hint-building read that field.
            app.viewer_state.sync_file_scroll_to_diff_scroll();
            enter_g_prefix_mode(app);
        }
        Some(Action::GoToBottom) => {
            app.viewer_state.diff_view.diff_view_scroll = total.saturating_sub(1);
        }
        Some(Action::SearchInFile) => {
            app.viewer_state.sync_file_scroll_to_diff_scroll();
            app.viewer_state.search.search_active = true;
            app.viewer_state.search.search_query.clear();
        }
        Some(Action::NextSearchMatch) => {
            app.viewer_state.next_search_match();
        }
        Some(Action::PrevSearchMatch) => {
            app.viewer_state.prev_search_match();
        }
        Some(Action::NextHunk) => {
            let lines = &app.viewer_state.diff_view.diff_view_lines;
            if let Some(idx) = next_change_block(lines, app.viewer_state.diff_view.diff_view_scroll)
            {
                app.viewer_state.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::PrevHunk) => {
            let lines = &app.viewer_state.diff_view.diff_view_lines;
            if let Some(idx) = prev_change_block(lines, app.viewer_state.diff_view.diff_view_scroll)
            {
                app.viewer_state.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::NextComment) => {
            let idx = next_comment_line(
                &app.viewer_state.diff_view.diff_view_lines,
                &app.review_state.file_comments,
                app.viewer_state.diff_view.diff_view_scroll,
            );
            if let Some(idx) = idx {
                app.viewer_state.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::PrevComment) => {
            let idx = prev_comment_line(
                &app.viewer_state.diff_view.diff_view_lines,
                &app.review_state.file_comments,
                app.viewer_state.diff_view.diff_view_scroll,
            );
            if let Some(idx) = idx {
                app.viewer_state.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::ScrollLeft) => {
            app.viewer_state.content.h_scroll = app.viewer_state.content.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer_state.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer_state.content.h_scroll = 0;
        }
        Some(Action::ToggleInlineThread) => {
            toggle_inline_thread(app);
        }
        Some(Action::InlineReply) => {
            start_inline_reply(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        Some(Action::AddComment) => {
            app.cmd_add_review_comment();
        }
        Some(Action::ExpandContext) => {
            // Expand 10 lines at the first visible ExpandableContext.
            if let Some(idx) = app.viewer_state.find_visible_expandable(50) {
                app.viewer_state.expand_context_at(idx, false);
            }
        }
        Some(Action::ExpandAllContext) => {
            // Expand all lines at the first visible ExpandableContext.
            if let Some(idx) = app.viewer_state.find_visible_expandable(50) {
                app.viewer_state.expand_context_at(idx, true);
            }
        }
        Some(Action::ToggleViewed) => {
            if let Some(path) = app.viewer_state.content.current_file.clone() {
                app.toggle_path_viewed(&path);
            }
        }
        Some(Action::ShowHoverInfo) => {
            app.viewer_state.sync_file_scroll_to_diff_scroll();
            app.show_hover_info();
        }
        _ => {}
    }
}
