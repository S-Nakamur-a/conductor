//! オーバーレイ/モーダルの振り分けと、それらが共有する汎用の小さな
//! confirm/skip-reason ポップアップ。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;

/// パネルの上に表示され得るあらゆるオーバーレイ/モーダルを描画する: worktree と
/// review のテキスト入力モーダル、ActiveOverlay 系ポップアップ（ブランチ切替、
/// cherry-pick、ヘルプなど）、ファイル名検索モーダル、更新ダイアログ、
/// references/symbol-action ポップアップ、skip-reason モーダル。通常のアコーディオン
/// レイアウトと review モードの両方で共有されるため、どちらのレイアウトが
/// 表示されていてもオーバーレイ（コメント、ヘルプ、コマンドパレットなど）は動き続ける。
pub(super) fn render_overlays(frame: &mut Frame, area: Rect, app: &mut App) {
    // worktree の入力モードオーバーレイ（ActiveOverlay enum には含まれない）。
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
    // review の入力モードオーバーレイ（ActiveOverlay enum には含まれない）。
    // アンカー付きの新規コメントは、このモーダルではなく viewer 内のインライン
    // コンポーズボックスとして描画されるため、その場合はモーダルを抑制する。
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
                    Some(f.as_str()) == app.viewer.content.current_file.as_deref()
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
            app.config.ui.icon_set(),
        );
    }
    if app.review_state.comment_detail_active {
        super::super::review::render_comment_detail_overlay(frame, area, app);
    }
    // ActiveOverlay に基づくオーバーレイ群。
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
            crate::worktree::bar::render_switcher_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::CommentList => {
            app.render_explorer_overlay(frame, area);
        }
        crate::overlay::ActiveOverlay::ThemePicker => {
            super::super::theme_picker::render_theme_picker_overlay(frame, area, app);
        }
        crate::overlay::ActiveOverlay::RevidereConfirm => {
            super::super::dashboard::render_revidere_confirm_overlay(frame, area, app);
        }
    }
    // ファジーなファイル名検索（「ジャンプ先」）モーダル ―― explorer カラムが
    // 折りたたまれていて（viewer 最大化時）も動くようトップレベルで描画する。
    if app.viewer.filename_search.filename_search_active {
        super::super::dashboard::render_filename_search_overlay(frame, area, app);
    }
    match app.update.state {
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
    if app.publish.confirm.is_some() {
        super::super::dashboard::render_publish_confirm_overlay(frame, area, app);
    }

    // References オーバーレイ（パネルレベル、OverlayManager には含まれない）
    if app.code_nav.references.active {
        crate::viewer::render::references::render_references_overlay(frame, area, app);
    }

    // シンボルアクションオーバーレイ（ヒント選択後）
    if app.code_nav.symbol_action.active {
        crate::viewer::render::symbol_action::render_symbol_action_overlay(frame, area, app);
    }

    // ホバー情報ポップアップ（viewer での K）
    if app.code_nav.hover_info.info.is_some() {
        crate::viewer::render_hover_overlay(frame, area, app);
    }
}

/// worktree 削除用の小さな確認オーバーレイを描画する。
fn render_confirming_delete_overlay(frame: &mut Frame, area: Rect, app: &App) {
    render_confirm_overlay(frame, area, app, " Confirm Delete ", app.theme.error);
}

/// タイトルとボーダー色をカスタマイズできる、汎用の小さな確認オーバーレイ。
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
