//! Explorer が返した [Intent] を App が解釈する。
//!
//! パネルが他パネルへ書く経路はここ 1 本に集まっている。分割前は Explorer の
//! 入力とマウス処理から 13 のフィールドパスへ直接書いていた。

use crate::app::App;
use crate::overlay::ActiveOverlay;
use crate::types::Focus;

use super::intent::{Intent, SectionOp};

impl App {
    pub fn apply_explorer_intent(&mut self, intent: Intent) {
        match intent {
            Intent::OpenFile { path } => {
                let tab_width = self.config.viewer.tab_width;
                let root = self.explorer.root().to_path_buf();
                self.viewer.open_file(&root, &path, tab_width);
                self.rehighlight_viewer();
                self.review_state.build_file_comment_cache(&path);
                self.set_focus(Focus::Viewer);
            }
            Intent::PreviewFile { path } => {
                let tab_width = self.config.viewer.tab_width;
                let root = self.explorer.root().to_path_buf();
                self.viewer.open_file_preview(&root, &path, tab_width);
                self.rehighlight_viewer();
                self.review_state.build_file_comment_cache(&path);
            }
            Intent::OpenSelectedChange => {
                self.open_diff_file_at_selected();
                self.set_focus(Focus::Viewer);
            }
            Intent::OpenSummary => {
                self.viewer.enter_summary_view();
                self.set_focus(Focus::Viewer);
            }
            Intent::RevealComment {
                comment,
                focus_viewer,
            } => self.reveal_comment(comment, focus_viewer),
            Intent::OpenSelectedCommentDetail => self.open_selected_comment_detail(),
            Intent::BeginReplyToSelected => self.begin_reply_to_selected(),
            Intent::ToggleCommentExpansion => self.toggle_comment_expansion(),
            Intent::EditSelectedComment => self.start_edit_selected_review_item(),
            Intent::DeleteSelectedComment => self.request_delete_selected_review_item(),
            Intent::ToggleCommentResolved => self.toggle_selected_review_status(),
            Intent::Section { op } => {
                let row = self.explorer.changes_cursor.selected();
                match op {
                    SectionOp::Toggle => {
                        self.diff_state.toggle_section(row);
                    }
                    SectionOp::Expand => self.diff_state.expand_section(row),
                    SectionOp::Collapse => self.diff_state.collapse_section(row),
                }
                // 開閉で行数が変わるが、収め直しはしない。窓の高さを知っているのは
                // 次にキーを受け取る入口だけで、そこが先頭で clamp する。
            }
            Intent::ToggleSelectedViewed => {
                let row = self.explorer.changes_cursor.selected();
                if let Some(file) = self.diff_state.resolve_file(row) {
                    let path = file.path.clone();
                    self.toggle_path_viewed(&path);
                }
            }
            Intent::CloseModal => self.overlays.active = ActiveOverlay::None,
            Intent::AskClaudeAboutChanges => self.ask_claude_about_changes(),
            Intent::OpenFilenameSearch => crate::event::open_filename_search(self),
        }
    }
}

impl App {
    /// コメントのファイルと行へ Viewer を寄せ、その行を選択状態にする。
    fn reveal_comment(&mut self, comment: usize, focus_viewer: bool) {
        let Some(c) = self.review_state.comments.get(comment) else {
            return;
        };
        let file_path = c.file_path.clone();
        let line = c.line_start as usize;
        let tab_width = self.config.viewer.tab_width;
        let root = self.explorer.root().to_path_buf();
        self.viewer.open_file(&root, &file_path, tab_width);
        self.rehighlight_viewer();
        self.viewer.content.file_scroll = line.saturating_sub(1);
        // コメントはソース行に紐づくので source を出す。markdown 描画のままだと
        // 本文の先頭へ飛ばされ、選択箇所が見えない。
        self.viewer.show_raw_for_line_target();
        self.viewer.selection = crate::viewer::LineSelection::Selected {
            start: line,
            end: line,
        };
        self.review_state.build_file_comment_cache(&file_path);
        if focus_viewer {
            self.set_focus(Focus::Viewer);
        }
    }

    fn open_selected_comment_detail(&mut self) {
        let row = self.explorer.comments_cursor.selected();
        let Some(comment) = self.review_state.selected_comment_idx(row) else {
            return;
        };
        self.review_state.comment_detail_idx = comment;
        self.review_state.comment_detail_scroll = 0;
        self.review_state.comment_detail_active = true;
        let Some(id) = self
            .review_state
            .comments
            .get(comment)
            .map(|c| c.id.clone())
        else {
            return;
        };
        if !self.review_state.cached_replies.contains_key(&id)
            && let Some(store) = self.review_store.as_ref()
            && let Ok(replies) = store.get_replies(&id)
        {
            self.review_state.cached_replies.insert(id, replies);
        }
    }

    fn begin_reply_to_selected(&mut self) {
        let row = self.explorer.comments_cursor.selected();
        let Some(comment) = self.review_state.selected_comment_idx(row) else {
            return;
        };
        self.review_state.input_buffer.clear();
        self.review_state.input_mode = crate::review_state::ReviewInputMode::ReplyingToComment;
        self.review_state.selected = comment;
        self.review_state.status_message =
            Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
    }

    /// 未対応のコメントをまとめて Claude へ送る。ID を付けないので一括モードになる。
    fn ask_claude_about_changes(&mut self) {
        let prompt = "/conductor:address-conductor-comment\n".to_string();
        let Some(idx) = self.terminal.claude.active_session else {
            self.set_status(
                "No active Claude Code session".to_string(),
                crate::app::StatusLevel::Warning,
            );
            return;
        };
        if self.terminal.pty_manager.is_waiting_for_input(idx) {
            let _ = self
                .terminal
                .pty_manager
                .write_chunked_to_session(idx, &prompt);
        } else {
            self.terminal.deferred_prompts.insert(idx, prompt);
        }
        self.set_focus(Focus::TerminalClaude);
        self.set_status(
            "Sent all comments to Claude".to_string(),
            crate::app::StatusLevel::Info,
        );
    }
}
