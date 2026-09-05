//! Viewer のキー入力。

use conductor_core::keymap::Action;
use conductor_core::review_store::ReviewComment;
use crossterm::event::{KeyCode, KeyEvent};

use super::{H_STEP, HALF_PAGE, ViewerPanel, diff, search};
use crate::comment_list::flip_status;
use crate::effect::Effect;
use crate::modal::{CommentEditor, Modal, Prompt};
use crate::review::{ReviewState, anchor_for, anchors, innermost};
use crate::task::{ReviewWrite, Task};
use crate::workspace::{Ctx, Focus, StatusLevel};

impl ViewerPanel {
    pub fn awaiting_chord(&self) -> bool {
        self.pending_fold || self.awaiting_nav()
    }

    /// z と g の 2 打鍵目。どちらもキーマップに載せていないので、解決の前にここへ来る。
    pub fn chord_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        if !self.pending_fold {
            return self.nav_chord(key, ctx);
        }
        self.pending_fold = false;
        match key.code {
            KeyCode::Char(c) => self.fold_chord(c),
            _ => Vec::new(),
        }
    }

    /// 折りたたみの操作。z の 2 打鍵目とコマンドの両方がここへ来る。
    pub fn fold_chord(&mut self, c: char) -> Vec<Effect> {
        let line = self.cursor_line();
        let mut depth = None;
        match c {
            'a' => {
                self.fold.toggle(line);
            }
            'c' => {
                self.fold.close(line);
            }
            'o' => {
                self.fold.open(line);
            }
            'm' => depth = self.fold.collapse_deepest(),
            'r' => depth = self.fold.expand_shallowest(),
            'R' => self.fold.open_all(),
            'M' => self.fold.close_all(),
            _ => return Vec::new(),
        }
        // 畳んだ結果カーソル行が隠れることがある。
        self.scroll.line = self.fold.visible_anchor(self.cursor_line()) - 1;
        depth
            .map(|d| {
                vec![Effect::Status(
                    StatusLevel::Info,
                    format!("fold level {}/{}", d.level, d.max),
                )]
            })
            .unwrap_or_default()
    }

    /// カーソル行 (1 始まり)。
    pub(super) fn cursor_line(&self) -> usize {
        self.scroll.line + 1
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        if action == Action::ToggleMarkdownRender {
            return Some(self.toggle_markdown());
        }
        if action == Action::OpenInEditor {
            return Some(self.open_in_editor());
        }
        // 行に紐づくキーは素通しして global へ落ち、そこから戻ってもまたここへ来る
        // ので何も起きない。畳む対象を並べた表を持たずに済む。
        if self.is_showing_rendered_markdown() {
            return self.rendered_key(action);
        }
        if let Some(effects) = self.comment_key(action, ctx.review) {
            return Some(effects);
        }
        if let Some(effects) = self.nav_key(action, ctx) {
            return Some(effects);
        }
        match action {
            Action::ExitToExplorer => {
                if self.nav.hover.take().is_some() {
                    // 出ているポップアップが先。畳む前に diff を抜けると、
                    // 見えているものと esc の意味がずれる。
                } else if !self.selection.is_empty() {
                    self.selection.clear();
                } else if self.diff.active {
                    self.diff.clear();
                } else {
                    return Some(vec![Effect::Focus(Focus::Explorer)]);
                }
                return Some(Vec::new());
            }
            Action::NextViewerTab => return Some(self.step_tab(1)),
            Action::PrevViewerTab => return Some(self.step_tab(-1)),
            Action::CloseViewerTab => return Some(self.close_tab(self.active)),
            Action::SearchFilename => {
                return Some(vec![crate::panels::explorer::find_file_modal()]);
            }
            Action::SearchInFile => {
                return Some(vec![Effect::PushModal(Modal::Prompt(Prompt::single(
                    "Search in file",
                    |q| vec![Effect::SearchInFile(q)],
                )))]);
            }
            _ => {}
        }
        if self.common_key(action) {
            return Some(Vec::new());
        }
        if self.diff.active {
            self.diff_key(action)
        } else {
            self.file_key(action)
        }
    }

    /// レンダリング表示のキー。
    fn rendered_key(&mut self, action: Action) -> Option<Vec<Effect>> {
        let last = self.content.rendered.len().saturating_sub(1);
        match action {
            Action::NavigateDown => self.scroll.md = (self.scroll.md + 1).min(last),
            Action::NavigateUp => self.scroll.md = self.scroll.md.saturating_sub(1),
            Action::ScrollHalfPageDown => {
                self.scroll.md = (self.scroll.md + HALF_PAGE as usize).min(last);
            }
            Action::ScrollHalfPageUp => {
                self.scroll.md = self.scroll.md.saturating_sub(HALF_PAGE as usize);
            }
            Action::GoToTop => self.scroll.md = 0,
            Action::GoToBottom => self.scroll.md = last,
            Action::ExitToExplorer => return Some(vec![Effect::Focus(Focus::Explorer)]),
            Action::NextViewerTab => return Some(self.step_tab(1)),
            Action::PrevViewerTab => return Some(self.step_tab(-1)),
            Action::CloseViewerTab => return Some(self.close_tab(self.active)),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// レビューコメントのキー。素の本文と diff のどちらでも同じ意味になる。
    fn comment_key(&mut self, action: Action, review: &ReviewState) -> Option<Vec<Effect>> {
        if !matches!(
            action,
            Action::AddComment
                | Action::ToggleInlineThread
                | Action::ReplyToComment
                | Action::ToggleResolve
                | Action::NextComment
                | Action::PrevComment
        ) {
            return None;
        }
        let path = self.content.path.clone()?;
        let comments = review.for_file(&path);
        let Some(line) = self.comment_line() else {
            return Some(no_place_to_comment());
        };
        let effects = match action {
            Action::AddComment => self.start_comment(line),
            Action::ToggleInlineThread => {
                if let Some(anchor) = anchor_for(&comments, line) {
                    self.threads.flip(anchor);
                }
                Vec::new()
            }
            Action::ReplyToComment => match innermost(&comments, line) {
                Some(comment) => vec![Effect::PushModal(Modal::CommentEditor(
                    CommentEditor::reply_to(comment),
                ))],
                None => no_comment_here(),
            },
            Action::ToggleResolve => match innermost(&comments, line) {
                Some(comment) => vec![Effect::Spawn(Task::WriteReview(ReviewWrite::SetStatus {
                    id: comment.id.clone(),
                    status: flip_status(comment.status),
                }))],
                None => no_comment_here(),
            },
            _ => {
                let forward = action == Action::NextComment;
                match step_anchor(&comments, line, forward) {
                    Some(next) => self.goto_line(next),
                    None => return Some(no_comment_here()),
                }
                Vec::new()
            }
        };
        Some(effects)
    }

    pub(super) fn start_comment(&mut self, line_1: usize) -> Vec<Effect> {
        let Some(path) = self.content.path.clone() else {
            return Vec::new();
        };
        let (start, end) = self.comment_range(line_1);
        self.selection.clear();
        vec![Effect::PushModal(Modal::CommentEditor(
            CommentEditor::new_comment(path, start, end),
        ))]
    }

    /// コメントを付ける行 (1 始まり)。削除行のように新ファイル側の行番号を持たない
    /// 位置は None — コメントのキーは新ファイル側の行番号なので、置き場所が無い。
    pub(super) fn comment_line(&self) -> Option<usize> {
        if !self.diff.active {
            return (!self.content.lines.is_empty()).then(|| self.scroll.line + 1);
        }
        self.diff.entries.get(self.scroll.diff)?.new_line_no()
    }

    fn comment_range(&self, line: usize) -> (u32, Option<u32>) {
        match self.selection.range() {
            Some((start, end)) if !self.diff.active => (start as u32, Some(end as u32)),
            _ => (line as u32, None),
        }
    }

    /// 素の本文と diff のどちらで見ていても同じ意味になるキー。片方に足し忘れると
    /// 「diff では効くが本文では効かない」が静かに出る。
    fn common_key(&mut self, action: Action) -> bool {
        match action {
            Action::NextSearchMatch => {
                if let Some(line) = self.search.advance() {
                    self.goto_line(line + 1);
                }
            }
            Action::PrevSearchMatch => {
                if let Some(line) = self.search.retreat() {
                    self.goto_line(line + 1);
                }
            }
            Action::ScrollLeft => self.scroll.column = self.scroll.column.saturating_sub(H_STEP),
            Action::ScrollRight => self.scroll_right(),
            Action::ScrollHome => self.scroll.column = 0,
            _ => return false,
        }
        true
    }

    fn file_key(&mut self, action: Action) -> Option<Vec<Effect>> {
        let total = self.content.lines.len();
        match action {
            Action::NavigateDown => self.move_cursor(1),
            Action::NavigateUp => self.move_cursor(-1),
            Action::ScrollHalfPageDown => self.move_cursor(HALF_PAGE),
            Action::ScrollHalfPageUp => self.move_cursor(-HALF_PAGE),
            Action::GoToTop => self.scroll.line = 0,
            Action::GoToBottom => {
                self.scroll.line = self.fold.last_visible(total).saturating_sub(1);
            }
            Action::FoldPrefix => self.pending_fold = true,
            _ => return None,
        }
        Some(Vec::new())
    }

    fn diff_key(&mut self, action: Action) -> Option<Vec<Effect>> {
        let last = self.diff.entries.len().saturating_sub(1);
        match action {
            Action::NavigateDown => self.scroll.diff = (self.scroll.diff + 1).min(last),
            Action::NavigateUp => self.scroll.diff = self.scroll.diff.saturating_sub(1),
            Action::ScrollHalfPageDown => {
                self.scroll.diff = (self.scroll.diff + HALF_PAGE as usize).min(last);
            }
            Action::ScrollHalfPageUp => {
                self.scroll.diff = self.scroll.diff.saturating_sub(HALF_PAGE as usize);
            }
            Action::GoToTop => self.scroll.diff = 0,
            Action::GoToBottom => self.scroll.diff = last,
            Action::NextHunk => {
                if let Some(idx) = diff::next_block(&self.diff.entries, self.scroll.diff) {
                    self.scroll.diff = idx;
                }
            }
            Action::PrevHunk => {
                if let Some(idx) = diff::prev_block(&self.diff.entries, self.scroll.diff) {
                    self.scroll.diff = idx;
                }
            }
            Action::ExpandContext | Action::ExpandAllContext => {
                let all = action == Action::ExpandAllContext;
                let height = (self.body.height as usize).max(1);
                if let Some(idx) = self.diff.visible_expandable(self.scroll.diff, height) {
                    self.diff.expand(idx, all, &self.content.lines);
                }
            }
            Action::ToggleDiffView => self.diff.side_by_side = !self.diff.side_by_side,
            Action::NextChangedFile => return Some(vec![Effect::StepChangedFile(1)]),
            Action::PrevChangedFile => return Some(vec![Effect::StepChangedFile(-1)]),
            Action::ToggleViewed => {
                let path = self.content.path.clone()?;
                return Some(vec![Effect::ToggleViewed(path)]);
            }
            _ => return None,
        }
        // diff を歩いた結果を素の本文側のカーソルへ写す。検索は本文の行で数える。
        if let Some(line) = search::file_line_at(&self.diff.entries, self.scroll.diff) {
            self.scroll.line = line;
        }
        Some(Vec::new())
    }
}

pub(super) fn no_place_to_comment() -> Vec<Effect> {
    vec![Effect::Status(
        StatusLevel::Warning,
        "a deleted line has no place to hang a comment".into(),
    )]
}

fn no_comment_here() -> Vec<Effect> {
    vec![Effect::Status(
        StatusLevel::Info,
        "no comment on this line".into(),
    )]
}

/// 前後のスレッドの行。今いる行のスレッドには止まらない。
fn step_anchor(comments: &[&ReviewComment], line: usize, forward: bool) -> Option<usize> {
    let mut found: Vec<usize> = anchors(comments).into_iter().collect();
    found.sort_unstable();
    if forward {
        found.into_iter().find(|a| *a > line)
    } else {
        found.into_iter().rev().find(|a| *a < line)
    }
}
