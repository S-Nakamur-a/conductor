//! いまテキストを受け取っている入力欄はどれか。
//!
//! キー入力とペーストの両方がこの判定を必要とする。かつては
//! `is_text_input_active` が「入力中か」を bool で答え、`handle_paste_event` が
//! 同じ条件を else-if で並べ直して宛先を決めており、宛先を 1 つ足すたびに
//! 2 箇所を手で揃える必要があった。ここで列挙を 1 つにして、
//! 「入力中か」は [InputTarget::active] の `is_some`、「どこへ入れるか」は
//! その値の `match` から出るようにしている。

use crate::app::{App, WorktreeInputMode};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

/// 印字可能な文字とペーストの宛先。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputTarget {
    InlineReply,
    ReviewInput,
    SmartDescription,
    WorktreeName,
    GrepSearch,
    ViewerSearch,
    FilenameSearch,
    ReviewSearch,
    SwitchBranch,
    CommandPalette,
    OpenRepo,
    PrInput,
    History,
    ResumeSession,
}

impl InputTarget {
    /// いま入力を受け取っている欄。どこも受け取っていなければ `None`。
    ///
    /// 並び順がそのまま優先順位で、上にあるものが勝つ。
    /// `WorktreeInputMode::Confirming*` の y/n サブモードはテキスト入力では
    /// ないので、ここには現れない。
    pub fn active(app: &App) -> Option<Self> {
        if app.viewer.inline.reply_line.is_some() {
            return Some(Self::InlineReply);
        }
        if app.review_state.input_mode != ReviewInputMode::Normal {
            return Some(Self::ReviewInput);
        }
        match app.worktree_mgr.input_mode {
            WorktreeInputMode::SmartDescription => return Some(Self::SmartDescription),
            WorktreeInputMode::CreatingWorktree | WorktreeInputMode::CreatingWorktreeBase => {
                return Some(Self::WorktreeName);
            }
            _ => {}
        }
        if app.overlays.active == ActiveOverlay::GrepSearch {
            return Some(Self::GrepSearch);
        }
        if app.viewer.search.search_active {
            return Some(Self::ViewerSearch);
        }
        if app.viewer.filename_search.filename_search_active {
            return Some(Self::FilenameSearch);
        }
        if app.review_state.search_active {
            return Some(Self::ReviewSearch);
        }
        match app.overlays.active {
            ActiveOverlay::SwitchBranch => Some(Self::SwitchBranch),
            ActiveOverlay::CommandPalette => Some(Self::CommandPalette),
            ActiveOverlay::OpenRepo => Some(Self::OpenRepo),
            ActiveOverlay::PrInput => Some(Self::PrInput),
            ActiveOverlay::History => Some(Self::History),
            ActiveOverlay::ResumeSession => Some(Self::ResumeSession),
            _ => None,
        }
    }

    /// 改行を受け取れる欄か。単一行の欄へは改行を落として入れる。
    pub fn is_multiline(self) -> bool {
        matches!(self, Self::ReviewInput | Self::SmartDescription)
    }

    /// この欄へテキストを挿入する。
    pub fn insert(self, app: &mut App, text: &str) {
        match self {
            Self::InlineReply => app.viewer.inline.reply_buffer.insert_str(text),
            Self::ReviewInput => app.review_state.input_buffer.insert_str(text),
            Self::SmartDescription => app.worktree_mgr.smart_description_buffer.insert_str(text),
            Self::WorktreeName => app.worktree_mgr.input_buffer.insert_str(text),
            Self::GrepSearch => {
                app.overlays.grep_search.query.insert_str(text);
                app.overlays.grep_search.input_focused = true;
                app.overlays.grep_search.schedule();
            }
            Self::ViewerSearch => app.viewer.search.search_query.insert_str(text),
            Self::FilenameSearch => app
                .viewer
                .filename_search
                .filename_search_query
                .insert_str(text),
            Self::ReviewSearch => {
                app.review_state.search_query.insert_str(text);
                app.review_state.apply_filter();
            }
            Self::SwitchBranch => app.overlays.switch_branch.filter.insert_str(text),
            Self::CommandPalette => app.overlays.command_palette.filter.insert_str(text),
            Self::OpenRepo => app.overlays.open_repo.buffer.insert_str(text),
            Self::PrInput => {
                app.overlays.pr_input.buffer.insert_str(text);
                app.overlays.pr_input.error = None;
            }
            Self::History => app.overlays.history.search_query.insert_str(text),
            Self::ResumeSession => app.overlays.resume_session.filter.insert_str(text),
        }
    }
}
