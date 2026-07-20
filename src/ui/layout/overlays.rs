//! Overlay/modal dispatch and the small generic confirm/skip-reason popups
//! shared by them.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;

/// Render every overlay/modal that can appear on top of the panels: worktree
/// and review text-input modals, the `ActiveOverlay` popups (switch-branch,
/// cherry-pick, help, etc.), the filename-search modal, the update dialog, the
/// references/symbol-action popups, and the skip-reason modal. Shared by the
/// normal accordion layout and review mode, so overlays keep working
/// (comments, help, the command palette, …) no matter which layout is showing.
pub(super) fn render_overlays(frame: &mut Frame, area: Rect, app: &mut App) {
    // Worktree input mode overlays (not part of ActiveOverlay enum).
    match app.worktree_mgr.input_mode {
        crate::app::WorktreeInputMode::CreatingWorktree => {
            super::super::dashboard::render_worktree_input_overlay(frame, area, app);
        }
        crate::app::WorktreeInputMode::CreatingWorktreeBase => {
            super::super::dashboard::render_worktree_base_input_overlay(frame, area, app);
        }
        crate::app::WorktreeInputMode::ConfirmingDelete => {
            render_confirming_delete_overlay(frame, area, app);
        }
        crate::app::WorktreeInputMode::ConfirmingDeleteBranch => {
            super::super::dashboard::render_delete_branch_confirm_overlay(frame, area, app);
        }
        crate::app::WorktreeInputMode::ConfirmingUngrab => {
            render_confirm_overlay(frame, area, app, " Confirm Ungrab ", app.theme.warning);
        }
        crate::app::WorktreeInputMode::ConfirmingReset => {
            render_confirm_overlay(frame, area, app, " Confirm Reset ", app.theme.error);
        }
        crate::app::WorktreeInputMode::SmartDescription => {
            super::super::dashboard::render_smart_description_overlay(frame, area, app);
        }
        crate::app::WorktreeInputMode::Normal => {}
    }
    // Review input overlays (not part of ActiveOverlay enum). A new comment with
    // an anchor renders as an inline compose box in the viewer instead of this
    // modal, so suppress the modal in that case.
    if app.review_state.input_mode == crate::review_state::ReviewInputMode::ConfirmingDelete {
        super::super::review::render_delete_confirm_overlay(frame, area, app);
    } else if app.review_state.input_mode != crate::review_state::ReviewInputMode::Normal {
        let inline_new_comment = app.review_state.input_mode
            == crate::review_state::ReviewInputMode::AddingComment
            && app
                .review_state
                .input_anchor
                .as_ref()
                .is_some_and(|(f, _, _)| {
                    Some(f.as_str()) == app.viewer_state.content.current_file.as_deref()
                });
        if !inline_new_comment {
            super::super::review::render_input_overlay(frame, area, app);
        }
    }
    if app.review_state.template_picker_active {
        super::super::review::render_template_picker_overlay(
            frame,
            area,
            &app.review_state,
            &app.theme,
        );
    }
    if app.review_state.comment_detail_active {
        super::super::review::render_comment_detail_overlay(frame, area, app);
    }
    if app.viewer_state.explorer.walkthrough_detail_active {
        super::super::walkthrough_pane::render_detail_overlay(frame, area, app);
    }
    // ActiveOverlay-based overlays.
    match app.overlays.active {
        crate::overlay::ActiveOverlay::None => {}
        crate::overlay::ActiveOverlay::History => {
            super::super::dashboard::render_history_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::CherryPick => {
            super::super::dashboard::render_cherry_pick_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::SwitchBranch => {
            super::super::dashboard::render_switch_branch_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::Grab => {
            super::super::dashboard::render_grab_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::Prune => {
            super::super::dashboard::render_prune_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::RepoSelector => {
            super::super::dashboard::render_repo_selector_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::OpenRepo => {
            super::super::dashboard::render_open_repo_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::PrInput => {
            super::super::dashboard::render_pr_input_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::ResumeSession => {
            super::super::dashboard::render_resume_session_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::GrepSearch => {
            super::super::grep_search::render_grep_search_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::CommandPalette => {
            super::super::dashboard::render_command_palette_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::Help => {
            super::super::dashboard::render_help_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::WorktreeSwitcher => {
            super::super::worktree_bar::render_switcher_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::CommentList => {
            super::super::explorer_panel::render_comment_list_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::ThemePicker => {
            super::super::theme_picker::render_theme_picker_overlay(frame, area, app);
        }
    }
    // Fuzzy filename-search ("jump to file") modal — rendered at the top level
    // so it works even when the explorer column is collapsed (viewer maximized).
    if app.viewer_state.filename_search.filename_search_active {
        super::super::dashboard::render_filename_search_overlay(frame, area, app);
    }
    match app.update_state {
        crate::app::UpdateState::Confirming => {
            super::super::dashboard::render_update_confirm_overlay(frame, area, app);
        }
        crate::app::UpdateState::InProgress
        | crate::app::UpdateState::Restarting
        | crate::app::UpdateState::Failed => {
            super::super::dashboard::render_update_progress_overlay(frame, area, app);
        }
        crate::app::UpdateState::Idle => {}
    }
    if app.publish_confirm.is_some() {
        super::super::dashboard::render_publish_confirm_overlay(frame, area, app);
    }

    // ── References overlay (panel-level, not part of OverlayManager) ──
    if app.references_overlay.active {
        crate::ui::references::render_references_overlay(frame, area, app);
    }

    // ── Symbol action overlay (after hint selection) ──
    if app.symbol_action_overlay.active {
        crate::ui::symbol_action::render_symbol_action_overlay(frame, area, app);
    }

    // ── Skip reason modal ────────────────────────────────────────────
    if let Some(ref reason) = app.worktree_mgr.skip_reason {
        render_skip_reason_overlay(frame, area, reason, &app.theme);
    }
}

/// Render a small confirmation overlay for worktree deletion.
fn render_confirming_delete_overlay(frame: &mut Frame, area: Rect, app: &App) {
    render_confirm_overlay(frame, area, app, " Confirm Delete ", app.theme.error);
}

/// Generic small confirmation overlay with a customizable title and border color.
fn render_confirm_overlay(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    border_color: ratatui::style::Color,
) {
    if let Some(ref status_msg) = app.status_message {
        let msg = &status_msg.text;
        let popup_height = 3_u16;
        let popup_width = area.width.saturating_sub(8).min(60);
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + area.height.saturating_sub(popup_height + 2);
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(ratatui::widgets::Clear, popup_area);

        let block = ratatui::widgets::Block::default()
            .title(title)
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(border_color));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Span::styled(
            msg.as_str(),
            ratatui::style::Style::default().fg(app.theme.fg),
        ));
        frame.render_widget(paragraph, inner);
    }
}

/// Render a skip-reason informational popup.
fn render_skip_reason_overlay(
    frame: &mut Frame,
    area: Rect,
    reason: &str,
    theme: &crate::theme::Theme,
) {
    let popup_height = 5_u16;
    let popup_width = area.width.saturating_sub(8).min(60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = ratatui::widgets::Block::default()
        .title(" Skipped ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(theme.warning));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let text = vec![
        ratatui::text::Line::from(ratatui::text::Span::styled(
            reason,
            ratatui::style::Style::default().fg(theme.warning),
        )),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "(Esc) 閉じる",
            ratatui::style::Style::default().fg(theme.muted),
        )),
    ];
    let paragraph =
        ratatui::widgets::Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}
