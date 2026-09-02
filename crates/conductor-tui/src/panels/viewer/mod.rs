//! 開いているファイルを読む場所。素の本文と unified / 左右 2 列の diff。
//!
//! 相対パスの解決先はここが持つ。書き換えるのは [ViewerPanel::set_root] だけで、
//! 表示中のツリーと開く先が食い違う窓を作らない。

pub mod content;
pub mod diff;
pub mod fold;
pub mod render;
pub mod search;
pub mod syntax;
pub mod tabs;
pub mod thread;

use std::path::{Path, PathBuf};

use conductor_core::diff_state::FileDiff;
use conductor_core::keymap::{Action, KeyContext};
use conductor_core::review_store::ReviewComment;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use crate::comment_list::flip_status;
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::modal::{CommentEditor, Modal, Prompt};
use crate::review::{ReviewState, anchor_for, anchors, innermost};
use crate::task::{ReviewWrite, Task, TaskResult};
use crate::workspace::{Ctx, Focus, StatusLevel};

use content::Content;
use diff::DiffPane;
use fold::FoldState;
use search::Search;
use syntax::Highlighter;
use tabs::{Tab, TabStatus};
use thread::ThreadFolds;

/// 半ページの行数。
const HALF_PAGE: isize = 15;

/// 横スクロールの 1 回分。
const H_STEP: usize = 4;

/// ガターの桁が持っている意味。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Comment,
    Fold,
    Text,
}

/// 行の選択。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    range: Option<(usize, usize)>,
    /// shift 付きクリックの起点。0 は「まだ無い」。
    anchor: usize,
}

impl Selection {
    /// 1 始まり・両端含む・start <= end に正規化した範囲。
    pub fn range(&self) -> Option<(usize, usize)> {
        self.range
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
    }

    pub fn contains(&self, line_1: usize) -> bool {
        self.range()
            .is_some_and(|(start, end)| line_1 >= start && line_1 <= end)
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_none()
    }

    pub fn clear(&mut self) {
        self.range = None;
    }

    /// 起点は shift 無しのクリックでしか動かない。連続する shift クリックは常に
    /// 同じ所から伸びる。
    pub fn click(&mut self, line_1: usize, extend: bool) {
        if extend && self.anchor != 0 {
            self.range = Some((self.anchor, line_1));
            return;
        }
        self.range = Some((line_1, line_1));
        self.anchor = line_1;
    }
}

/// スクロール位置。素の本文と diff で別々に覚える。
#[derive(Debug, Default, Clone, Copy)]
pub struct Scroll {
    /// 素の本文の最上行 (0 始まり)。Viewer は独立したカーソルを持たず、これが兼ねる。
    pub line: usize,
    pub column: usize,
    /// diff のエントリ列への添字。
    pub diff: usize,
}

/// 読み込みを投げてから結果が返るまでの覚え書き。
#[derive(Debug)]
struct Load {
    seq: u64,
    path: String,
    /// 読み終わってから組む。エントリの末尾はファイルの行数で決まる。
    diff: Option<Box<FileDiff>>,
    line: Option<usize>,
    /// タブを開き直しただけなら、読んでいた位置に戻す。
    keep_scroll: bool,
}

pub struct ViewerPanel {
    root: PathBuf,
    tabs: Vec<Tab>,
    active: usize,
    pub content: Content,
    pub diff: DiffPane,
    pub search: Search,
    pub fold: FoldState,
    pub selection: Selection,
    pub scroll: Scroll,
    pub threads: ThreadFolds,
    /// 構築が重いので最初に必要になるまで作らない。
    highlighter: Option<Highlighter>,
    load: Option<Load>,
    seq: u64,
    /// z の 2 打鍵目を待っている。
    pending_fold: bool,
    /// タブ帯の窓の左端。
    tab_scroll: usize,
    /// 本文の矩形。レイアウトから引く。当たり判定と半ページの歩幅が読む。
    body: Rect,
    tab_row: Rect,
}

impl ViewerPanel {
    pub fn new(config: &conductor_core::config::Config) -> Self {
        Self {
            root: PathBuf::new(),
            tabs: Vec::new(),
            active: 0,
            content: Content::default(),
            diff: DiffPane {
                side_by_side: config.diff.default_view
                    == conductor_core::diff_state::DiffView::SideBySide,
                ..DiffPane::default()
            },
            search: Search::default(),
            fold: FoldState::default(),
            selection: Selection::default(),
            scroll: Scroll::default(),
            threads: ThreadFolds::default(),
            highlighter: None,
            load: None,
            seq: 0,
            pending_fold: false,
            tab_scroll: 0,
            body: Rect::default(),
            tab_row: Rect::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_loading(&self) -> bool {
        self.load.is_some()
    }

    /// diff を見ている間はキーの層が変わる。ハンク送りや文脈の展開は素の本文に
    /// 意味を持たない。
    pub fn key_context(&self) -> KeyContext {
        if self.diff.active {
            KeyContext::ViewerDiffMode
        } else {
            KeyContext::Viewer
        }
    }

    pub fn sync_layout(&mut self, layout: &Layout) {
        let Some(rect) = layout.rect(Region::Viewer) else {
            return;
        };
        let inner = crate::list::inner(rect);
        self.tab_row = Rect {
            height: render::TAB_ROW,
            ..inner
        };
        self.body = Rect {
            y: inner.y + render::TAB_ROW,
            height: inner.height.saturating_sub(render::TAB_ROW),
            ..inner
        };
        self.reveal_tab(self.tab_row.width);
    }

    /// 画面上の 1 点のクリック。shift 付きは起点から範囲を伸ばす。
    pub fn click(&mut self, x: u16, y: u16, extend: bool, review: &ReviewState) -> Vec<Effect> {
        if y < self.body.y {
            return self.click_tab_row(x.saturating_sub(self.tab_row.x), self.tab_row.width);
        }
        if self.diff.active {
            return Vec::new();
        }
        let offset = (y - self.body.y) as usize;
        let total = self.content.lines.len();
        let Some(line) = self
            .fold
            .visible_from(self.scroll.line + 1, total)
            .nth(offset)
        else {
            return Vec::new();
        };
        let comments = self
            .content
            .path
            .as_deref()
            .map_or_else(Vec::new, |path| review.for_file(path));
        match self.gutter_zone(x.saturating_sub(self.body.x), total, comments.is_empty()) {
            Zone::Comment => {
                if let Some(anchor) = anchor_for(&comments, line) {
                    self.threads.flip(anchor);
                }
            }
            Zone::Fold => {
                self.fold.toggle(line);
                self.scroll.line = self.fold.visible_anchor(line) - 1;
            }
            Zone::Text => self.selection.click(line, extend),
        }
        Vec::new()
    }

    /// ガターの桁割り。render の組み方と 1 対 1 で、印の下を押せば印の意味になる。
    fn gutter_zone(&self, column: u16, total: usize, no_comments: bool) -> Zone {
        let mark = if no_comments { 0 } else { render::MARK };
        let column = column as usize;
        if column < mark {
            return Zone::Comment;
        }
        let digits = render::digit_count(if self.diff.active {
            self.diff.max_line_no
        } else {
            total
        });
        if column == mark + digits {
            return Zone::Fold;
        }
        Zone::Text
    }

    /// worktree が変わった。相対パスは根が変わると別のファイルを指すので、
    /// 新しい根に無いファイルのタブは閉じる。
    pub fn set_root(&mut self, root: PathBuf) -> Vec<Effect> {
        if self.root == root {
            return Vec::new();
        }
        self.root = root;
        self.prune_tabs_to_root()
    }

    /// ファイルを開く唯一の入口。既に開いていればタブを増やさず、そこへ戻る。
    pub fn open(
        &mut self,
        path: &Path,
        line: Option<usize>,
        file_diff: Option<Box<FileDiff>>,
        preview: bool,
    ) -> Vec<Effect> {
        let Some(relative) = self.relative(path) else {
            return vec![Effect::Status(
                StatusLevel::Error,
                format!("{} is outside the worktree", path.display()),
            )];
        };
        let status = if preview {
            TabStatus::Preview
        } else {
            TabStatus::Persistent
        };
        let fresh = self.activate_tab_for(&relative, status);
        vec![self.request(relative, file_diff, line, !fresh)]
    }

    /// 根の外のパスは開かない。絶対パスは端末のリンクから来る。
    fn relative(&self, path: &Path) -> Option<String> {
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root).ok()?
        } else {
            path
        };
        Some(relative.to_string_lossy().to_string())
    }

    fn request(
        &mut self,
        path: String,
        file_diff: Option<Box<FileDiff>>,
        line: Option<usize>,
        keep_scroll: bool,
    ) -> Effect {
        self.seq += 1;
        let task = Task::LoadFile {
            root: self.root.clone(),
            path: path.clone(),
            seq: self.seq,
        };
        self.load = Some(Load {
            seq: self.seq,
            path,
            diff: file_diff,
            line,
            keep_scroll,
        });
        Effect::Spawn(task)
    }

    pub fn apply_result(&mut self, result: TaskResult) -> Vec<Effect> {
        let TaskResult::FileLoaded { seq, loaded } = result else {
            return Vec::new();
        };
        let Some(load) = self.load.take_if(|load| load.seq == seq) else {
            return Vec::new();
        };

        self.content.path = Some(load.path.clone());
        self.content.highlighted.clear();
        self.content.highlight_key = None;
        self.search.matches.clear();
        self.diff.clear();
        self.threads.clear();

        match loaded {
            Ok(file) => {
                self.content.lines = file.lines;
                self.content.error = None;
                self.fold.install(file.folds, &load.path);
            }
            Err(reason) => {
                self.content.lines.clear();
                self.content.error = Some(reason);
                self.fold.clear();
            }
        }

        let last = self.content.lines.len().saturating_sub(1);
        self.scroll.line = if load.keep_scroll {
            self.scroll.line.min(last)
        } else {
            0
        };
        self.scroll.diff = 0;
        if let Some(file_diff) = load.diff {
            self.diff.build(&file_diff, self.content.lines.len());
        }
        if let Some(line) = load.line {
            self.goto_line(line);
        }
        Vec::new()
    }

    /// 1 始まりの行へ寄せる。畳んで隠れていれば開いてから。
    fn goto_line(&mut self, line_1: usize) {
        self.fold.reveal(line_1);
        self.scroll.line = line_1.saturating_sub(1).min(self.last_line());
        if self.diff.active
            && let Some(idx) = search::diff_index_for(&self.diff.entries, self.scroll.line)
        {
            self.scroll.diff = idx;
        }
    }

    fn last_line(&self) -> usize {
        self.content.lines.len().saturating_sub(1)
    }

    /// 描く直前に構文ハイライトを整える。render は状態を書けないのでここでやる。
    pub fn refresh_highlight(&mut self, config: &conductor_core::config::Config) {
        if self.content.lines.is_empty() {
            return;
        }
        let highlighter = self
            .highlighter
            .get_or_insert_with(|| Highlighter::new(config));
        highlighter.adopt(config);
        let key = syntax::cache_key(
            highlighter.id(),
            self.content.path.as_deref(),
            &self.content.lines,
        );
        if self.content.highlight_key == Some(key) {
            return;
        }
        self.content.highlighted =
            highlighter.highlight(self.content.path.as_deref(), &self.content.lines);
        self.content.highlight_key = Some(key);
    }

    pub fn awaiting_chord(&self) -> bool {
        self.pending_fold
    }

    /// z の 2 打鍵目。
    pub fn chord_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.pending_fold = false;
        let line = self.cursor_line();
        let mut depth = None;
        match key.code {
            KeyCode::Char('a') => {
                self.fold.toggle(line);
            }
            KeyCode::Char('c') => {
                self.fold.close(line);
            }
            KeyCode::Char('o') => {
                self.fold.open(line);
            }
            KeyCode::Char('m') => depth = self.fold.collapse_deepest(),
            KeyCode::Char('r') => depth = self.fold.expand_shallowest(),
            KeyCode::Char('R') => self.fold.open_all(),
            KeyCode::Char('M') => self.fold.close_all(),
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
    fn cursor_line(&self) -> usize {
        self.scroll.line + 1
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        if let Some(effects) = self.comment_key(action, ctx.review) {
            return Some(effects);
        }
        match action {
            Action::ExitToExplorer => {
                if !self.selection.is_empty() {
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
                return Some(vec![Effect::PushModal(Modal::Prompt(Prompt {
                    title: "Search in file".into(),
                    input: Default::default(),
                    on_submit: |q| vec![Effect::SearchInFile(q)],
                }))]);
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
            return Some(vec![Effect::Status(
                StatusLevel::Warning,
                "a deleted line has no place to hang a comment".into(),
            )]);
        };
        let effects = match action {
            Action::AddComment => {
                let (start, end) = self.comment_range(line);
                self.selection.clear();
                vec![Effect::PushModal(Modal::CommentEditor(
                    CommentEditor::new_comment(path, start, end),
                ))]
            }
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

    /// コメントを付ける行 (1 始まり)。削除行のように新ファイル側の行番号を持たない
    /// 位置は None — コメントのキーは新ファイル側の行番号なので、置き場所が無い。
    fn comment_line(&self) -> Option<usize> {
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

    /// 検索し直して最初の当たりへ寄せる。
    pub fn search_for(&mut self, query: &str) -> Vec<Effect> {
        self.search.query.set_text(query);
        match self.search.run(&self.content.lines, self.scroll.line) {
            Some(line) => {
                self.goto_line(line + 1);
                Vec::new()
            }
            None if query.is_empty() => Vec::new(),
            None => vec![Effect::Status(
                StatusLevel::Info,
                format!("no match for '{query}'"),
            )],
        }
    }

    /// ホイール。diff とそれ以外で数える単位が違う。
    pub fn scroll_lines(&mut self, delta: isize) {
        if self.diff.active {
            let last = self.diff.entries.len().saturating_sub(1);
            self.scroll.diff = (self.scroll.diff as isize + delta).clamp(0, last as isize) as usize;
            return;
        }
        self.move_cursor(delta);
    }

    /// 可視行を delta 行ぶん動かす。畳んだ中にカーソルが入らない。
    fn move_cursor(&mut self, delta: isize) {
        let total = self.content.lines.len();
        if total == 0 {
            return;
        }
        self.scroll.line = self.fold.step(self.cursor_line(), delta, total) - 1;
    }

    /// 最も長い行を超えて右へ流れないようにする。
    fn scroll_right(&mut self) {
        let widest = if self.diff.active {
            self.diff
                .entries
                .iter()
                .filter_map(|e| match e {
                    diff::Entry::Line { content, .. } => Some(content.chars().count()),
                    _ => None,
                })
                .max()
        } else {
            self.content.lines.iter().map(|l| l.chars().count()).max()
        };
        let limit = widest.unwrap_or(0).saturating_sub(H_STEP);
        self.scroll.column = (self.scroll.column + H_STEP).min(limit);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::apply;
    use crate::testing::pump;
    use crate::workspace::Workspace;
    use conductor_svc::Services;
    use tempfile::TempDir;

    fn fixture(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, body) in files {
            std::fs::write(dir.path().join(path), body).unwrap();
        }
        dir
    }

    struct Harness {
        ws: Workspace,
        svc: Services<TaskResult>,
    }

    impl Harness {
        fn at(dir: &Path) -> Self {
            let mut harness = Self {
                ws: Workspace::for_test(),
                svc: Services::new(),
            };
            harness.ws.focus = Focus::Viewer;
            let effects = harness.ws.panels.viewer.set_root(dir.to_path_buf());
            harness.run(effects);
            harness
        }

        fn run(&mut self, effects: Vec<Effect>) {
            apply(&mut self.ws, &mut self.svc, effects);
            pump(&mut self.ws, &mut self.svc);
        }

        fn open(&mut self, path: &str) {
            let effects = self.viewer().open(Path::new(path), None, None, false);
            self.run(effects);
        }

        fn peek(&mut self, path: &str) {
            let effects = self.viewer().open(Path::new(path), None, None, true);
            self.run(effects);
        }

        fn act(&mut self, action: Action) {
            let effects = self.ws.dispatch(action).unwrap_or_default();
            self.run(effects);
        }

        fn viewer(&mut self) -> &mut ViewerPanel {
            &mut self.ws.panels.viewer
        }

        fn click(&mut self, x: u16, y: u16, extend: bool) -> Vec<Effect> {
            let Workspace { panels, review, .. } = &mut self.ws;
            panels.viewer.click(x, y, extend, review)
        }

        fn install(&mut self, comments: Vec<conductor_core::review_store::ReviewComment>) {
            self.ws.focus = Focus::Viewer;
            self.ws.review.install(Ok(crate::review::Snapshot {
                branch: "main".into(),
                comments,
                ..crate::review::Snapshot::default()
            }));
        }

        fn body(&self) -> &[String] {
            &self.ws.panels.viewer.content.lines
        }

        fn tabs(&self) -> Vec<&str> {
            self.ws
                .panels
                .viewer
                .tabs()
                .iter()
                .map(|t| t.path.as_str())
                .collect()
        }
    }

    #[test]
    fn 開くとタブが増え同じファイルは使い回す() {
        let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut h = Harness::at(dir.path());

        h.open("a.txt");
        h.open("b.txt");
        assert_eq!(h.tabs(), ["a.txt", "b.txt"]);

        h.open("a.txt");
        assert_eq!(h.tabs(), ["a.txt", "b.txt"], "既に開いているファイル");
        assert_eq!(h.ws.panels.viewer.active_path(), Some("a.txt"));
        assert_eq!(h.body(), ["A"]);
    }

    /// タブごとに読んでいた位置を持つ。戻ったときに先頭へ巻き戻ると、差分レビュー中に
    /// 行き来する用途では複数タブの意味が無くなる。
    #[test]
    fn タブを移ると読みかけの位置が戻りディスクから読み直す() {
        let long: String = (0..50).map(|i| format!("line{i}\n")).collect();
        let dir = fixture(&[("a.txt", long.as_str()), ("b.txt", long.as_str())]);
        let mut h = Harness::at(dir.path());

        h.open("a.txt");
        h.viewer().scroll.line = 30;
        h.open("b.txt");
        assert_eq!(h.ws.panels.viewer.scroll.line, 0, "新しいタブは先頭から");
        h.viewer().scroll.line = 10;

        std::fs::write(dir.path().join("a.txt"), "NEW\n").unwrap();
        h.act(Action::PrevViewerTab);
        assert_eq!(h.ws.panels.viewer.active_path(), Some("a.txt"));
        assert_eq!(h.body(), ["NEW"], "非アクティブ中の書き換えが反映される");

        h.act(Action::NextViewerTab);
        assert_eq!(h.ws.panels.viewer.active_path(), Some("b.txt"));
        assert_eq!(h.ws.panels.viewer.scroll.line, 10);
    }

    #[test]
    fn タブを閉じると隣へ移り最後は未選択になる() {
        let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.open("b.txt");

        h.act(Action::CloseViewerTab);
        assert_eq!(h.tabs(), ["a.txt"]);
        assert_eq!(h.body(), ["A"]);

        h.act(Action::CloseViewerTab);
        assert!(h.tabs().is_empty());
        assert_eq!(h.ws.panels.viewer.content.path, None);
        assert!(h.body().is_empty());
    }

    /// クリックするたびにタブが増えるのを防ぐのが preview の本題。
    #[test]
    fn previewのタブは1枚だけで永続で開き直すと固定される() {
        let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n"), ("c.txt", "C\n")]);
        let mut h = Harness::at(dir.path());

        h.peek("a.txt");
        h.peek("b.txt");
        assert_eq!(h.tabs(), ["b.txt"], "preview は同時に 1 枚");
        assert_eq!(h.body(), ["B"]);

        // 永続で開くと残っていた preview は閉じる。
        h.open("c.txt");
        assert_eq!(h.tabs(), ["c.txt"]);
        assert!(!h.ws.panels.viewer.tabs()[0].status.is_preview());

        // 同じファイルを永続で開き直すと固定され、次を開いても残る。
        h.peek("a.txt");
        h.open("a.txt");
        h.peek("b.txt");
        assert_eq!(h.tabs(), ["c.txt", "a.txt", "b.txt"]);
    }

    #[test]
    fn 別のタブへ移るとpreviewは閉じる() {
        let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n"), ("c.txt", "C\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.open("b.txt");
        h.peek("c.txt");
        assert_eq!(h.tabs().len(), 3);

        let effects = h.viewer().focus_tab(0);
        h.run(effects);
        assert_eq!(h.tabs(), ["a.txt", "b.txt"]);
        assert_eq!(h.ws.panels.viewer.active_tab(), 0);
        assert_eq!(h.body(), ["A"]);
    }

    #[test]
    fn 根が変わると無いファイルのタブは落ちる() {
        let a = fixture(&[("both.txt", "A\n"), ("only_a.txt", "A\n")]);
        let b = fixture(&[("both.txt", "B\n")]);
        let mut h = Harness::at(a.path());
        h.open("both.txt");
        h.open("only_a.txt");
        assert_eq!(h.tabs().len(), 2);

        let effects = h.viewer().set_root(b.path().to_path_buf());
        h.run(effects);
        assert_eq!(h.tabs(), ["both.txt"]);
        assert_eq!(h.body(), ["B"], "新しい根の中身を読む");
    }

    #[test]
    fn 開けなかった理由を残す() {
        let dir = fixture(&[("ok.txt", "OK\n")]);
        let mut h = Harness::at(dir.path());

        h.open("missing.txt");
        assert!(h.body().is_empty());
        assert_eq!(
            h.ws.panels.viewer.content.path.as_deref(),
            Some("missing.txt")
        );
        assert!(h.ws.panels.viewer.content.error.is_some());

        // 持ち越すと直後の正常なファイルまでエラー表示になる。
        h.open("ok.txt");
        assert!(h.ws.panels.viewer.content.error.is_none());
    }

    #[test]
    fn 遅れて届いた古い読み込みは捨てる() {
        let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.open("b.txt");

        let stale = TaskResult::FileLoaded {
            seq: 1,
            loaded: Ok(content::Loaded {
                lines: vec!["STALE".into()],
                folds: Vec::new(),
            }),
        };
        h.viewer().apply_result(stale);
        assert_eq!(h.body(), ["B"]);
    }

    #[test]
    fn diffを添えて開くと差分になりescで素の本文へ戻る() {
        let dir = fixture(&[("a.txt", "one\ntwo\nthree\n")]);
        let mut h = Harness::at(dir.path());
        let file_diff = FileDiff {
            path: "a.txt".into(),
            added_lines: 1,
            deleted_lines: 0,
            hunks: vec![conductor_core::diff_state::DiffHunk {
                lines: vec![conductor_core::diff_state::DiffLine {
                    tag: conductor_core::diff_state::DiffLineTag::Insert,
                    old_line_no: None,
                    new_line_no: Some(2),
                    inline_segments: Vec::new(),
                    content: "two".into(),
                }],
                func_header: None,
            }],
        };
        let effects = h
            .viewer()
            .open(Path::new("a.txt"), None, Some(Box::new(file_diff)), false);
        h.run(effects);

        assert!(h.ws.panels.viewer.diff.active);
        assert_eq!(h.ws.key_context(), KeyContext::ViewerDiffMode);
        h.act(Action::ExitToExplorer);
        assert!(
            !h.ws.panels.viewer.diff.active,
            "esc は先に diff から抜ける"
        );
        assert_eq!(h.ws.key_context(), KeyContext::Viewer);
        assert_eq!(h.body(), ["one", "two", "three"]);
    }

    #[test]
    fn 検索は当たった行へ寄せて次と前に送る() {
        let dir = fixture(&[("a.txt", "alpha\nbeta\nalpha\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");

        let effects = h.viewer().search_for("alpha");
        h.run(effects);
        assert_eq!(h.ws.panels.viewer.scroll.line, 0);
        h.act(Action::NextSearchMatch);
        assert_eq!(h.ws.panels.viewer.scroll.line, 2);
        h.act(Action::PrevSearchMatch);
        assert_eq!(h.ws.panels.viewer.scroll.line, 0);

        let effects = h.viewer().search_for("zzz");
        assert!(matches!(effects.as_slice(), [Effect::Status(..)]));
    }

    #[test]
    fn 折りたたみの2打鍵目はパネルが直接読む() {
        let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.rs");

        h.act(Action::FoldPrefix);
        assert!(h.ws.panels.viewer.awaiting_chord());
        h.viewer().scroll.line = 1;
        let effects = h.viewer().chord_key(KeyEvent::from(KeyCode::Char('c')));
        h.run(effects);
        assert!(h.ws.panels.viewer.fold.is_collapsed(1));
        assert_eq!(
            h.ws.panels.viewer.scroll.line, 0,
            "隠れた行から見出しへ寄る"
        );
        assert!(!h.ws.panels.viewer.awaiting_chord());
    }

    /// 畳んだ行を飛ばして数えるので、画面 3 行目が 3 行目とは限らない。
    #[test]
    fn クリックした画面行はその位置の可視行を選ぶ() {
        let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\nfn d() {}\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.rs");
        h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);

        h.viewer().click(20, 11, false, &ReviewState::default());
        assert_eq!(h.ws.panels.viewer.selection.range(), Some((2, 2)));
        h.viewer().click(20, 13, true, &ReviewState::default());
        assert_eq!(h.ws.panels.viewer.selection.range(), Some((2, 4)));

        h.viewer().fold.close(1);
        h.viewer().click(20, 11, false, &ReviewState::default());
        assert_eq!(
            h.ws.panels.viewer.selection.range(),
            Some((5, 5)),
            "閉じ括弧まで畳むので 2..4 を飛ばす"
        );
    }

    /// ガターの桁ごとに意味が違う。印の下を押せば印の意味になる。
    #[test]
    fn ガターの桁は印と畳みと本文で意味が分かれる() {
        let comments = vec![crate::review::tests::comment("a", "a.rs", 2, None)];
        let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\nfn d() {}\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.rs");
        h.install(comments.clone());
        h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);
        let all: Vec<&conductor_core::review_store::ReviewComment> = comments.iter().collect();

        // 印は 0..2、行番号は 2、その右の 3 が畳みの印、以降が本文。
        assert!(h.ws.panels.viewer.threads.is_open(&all, 2));
        h.click(0, 11, false);
        assert!(
            !h.ws.panels.viewer.threads.is_open(&all, 2),
            "印でスレッドを畳む"
        );
        assert!(h.ws.panels.viewer.selection.is_empty());

        h.click(3, 10, false);
        assert!(h.ws.panels.viewer.fold.is_collapsed(1), "畳みの印で畳む");
        assert!(h.ws.panels.viewer.selection.is_empty());

        h.click(20, 10, false);
        assert_eq!(h.ws.panels.viewer.selection.range(), Some((1, 1)));
    }

    #[test]
    fn ホイールは畳みを跨いで送り差分では行ではなくエントリを送る() {
        let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\nfn d() {}\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.rs");
        h.viewer().fold.close(1);
        h.viewer().scroll_lines(1);
        assert_eq!(h.ws.panels.viewer.scroll.line, 4);
        h.viewer().scroll_lines(-1);
        assert_eq!(h.ws.panels.viewer.scroll.line, 0);
    }

    /// zm / zr は何段畳んだかを返す。押した結果が画面に出ないと、どこまで畳んだのか
    /// 分からないまま連打することになる。
    #[test]
    fn 深さ単位の畳みは段数をステータスに出す() {
        let dir = fixture(&[("a.rs", "fn a() {\n    if x {\n        y();\n    }\n}\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.rs");

        let effects = h.viewer().chord_key(KeyEvent::from(KeyCode::Char('m')));
        let [Effect::Status(StatusLevel::Info, text)] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(text, "fold level 1/2");

        let effects = h.viewer().chord_key(KeyEvent::from(KeyCode::Char('a')));
        assert!(effects.is_empty(), "行単位の畳みは段数を出さない");
    }

    /// 選択があれば範囲、無ければカーソル行。どちらもコメント側の座標で渡す。
    #[test]
    fn cは選択の範囲をそのままコメントのアンカーにする() {
        let dir = fixture(&[("a.txt", "one\ntwo\nthree\nfour\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.ws.focus = Focus::Viewer;

        h.viewer().scroll.line = 2;
        let effects = h.ws.dispatch(Action::AddComment).unwrap();
        let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert!(matches!(
            editor.target,
            crate::modal::EditTarget::New {
                line_start: 3,
                line_end: None,
                ..
            }
        ));

        h.viewer().selection.click(2, false);
        h.viewer().selection.click(4, true);
        let effects = h.ws.dispatch(Action::AddComment).unwrap();
        let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert!(matches!(
            editor.target,
            crate::modal::EditTarget::New {
                line_start: 2,
                line_end: Some(4),
                ..
            }
        ));
        assert!(
            h.ws.panels.viewer.selection.is_empty(),
            "書き始めたら選択は畳む"
        );
    }

    /// コメントの座標は新ファイル側の行番号なので、削除行には置き場所が無い。
    #[test]
    fn 削除行ではコメントを始められない() {
        let dir = fixture(&[("a.txt", "one\n")]);
        let mut h = Harness::at(dir.path());
        let file_diff = FileDiff {
            path: "a.txt".into(),
            added_lines: 0,
            deleted_lines: 1,
            hunks: vec![conductor_core::diff_state::DiffHunk {
                lines: vec![conductor_core::diff_state::DiffLine {
                    tag: conductor_core::diff_state::DiffLineTag::Delete,
                    old_line_no: Some(1),
                    new_line_no: None,
                    inline_segments: Vec::new(),
                    content: "gone".into(),
                }],
                func_header: None,
            }],
        };
        let effects = h
            .viewer()
            .open(Path::new("a.txt"), None, Some(Box::new(file_diff)), false);
        h.run(effects);
        h.ws.focus = Focus::Viewer;

        let entry = h.ws.panels.viewer.diff.entries.iter().position(|e| {
            matches!(
                e,
                diff::Entry::Line {
                    new_line_no: None,
                    ..
                }
            )
        });
        h.viewer().scroll.diff = entry.expect("削除行");
        assert_eq!(h.ws.panels.viewer.comment_line(), None);
        let effects = h.ws.dispatch(Action::AddComment).unwrap();
        assert!(
            matches!(effects.as_slice(), [Effect::Status(..)]),
            "{effects:?}"
        );
    }

    #[test]
    fn spaceはカーソル行を覆うスレッドを開閉する() {
        let comments = vec![crate::review::tests::comment("a", "a.txt", 2, Some(4))];
        let dir = fixture(&[("a.txt", "1\n2\n3\n4\n5\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.install(comments.clone());
        let all: Vec<&conductor_core::review_store::ReviewComment> = comments.iter().collect();

        h.viewer().scroll.line = 2;
        assert!(
            h.ws.panels.viewer.threads.is_open(&all, 4),
            "未解決は既定で開く"
        );
        h.ws.dispatch(Action::ToggleInlineThread).unwrap();
        assert!(
            !h.ws.panels.viewer.threads.is_open(&all, 4),
            "範囲の途中から終端のスレッドを閉じる"
        );
    }

    #[test]
    fn 返信と解決はカーソル行のコメントに効きコメントが無ければ知らせる() {
        let comments = vec![crate::review::tests::comment("a", "a.txt", 2, None)];
        let dir = fixture(&[("a.txt", "1\n2\n3\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.install(comments);

        h.viewer().scroll.line = 1;
        let effects = h.ws.dispatch(Action::ReplyToComment).unwrap();
        assert!(matches!(
            effects.as_slice(),
            [Effect::PushModal(Modal::CommentEditor(_))]
        ));
        let effects = h.ws.dispatch(Action::ToggleResolve).unwrap();
        assert!(matches!(effects.as_slice(), [Effect::Spawn(_)]));

        h.viewer().scroll.line = 2;
        for action in [Action::ReplyToComment, Action::ToggleResolve] {
            let effects = h.ws.dispatch(action).unwrap();
            assert!(
                matches!(effects.as_slice(), [Effect::Status(..)]),
                "{action:?}"
            );
        }
    }

    #[test]
    fn コメント間の移動は今の行を飛ばして両端で止まる() {
        let comments = vec![
            crate::review::tests::comment("a", "a.txt", 2, None),
            crate::review::tests::comment("b", "a.txt", 5, None),
        ];
        let dir = fixture(&[("a.txt", "1\n2\n3\n4\n5\n6\n")]);
        let mut h = Harness::at(dir.path());
        h.open("a.txt");
        h.install(comments);

        let step = |h: &mut Harness, action| {
            let effects = h.ws.dispatch(action).unwrap();
            (h.ws.panels.viewer.scroll.line, effects)
        };
        assert_eq!(step(&mut h, Action::NextComment).0, 1);
        assert_eq!(step(&mut h, Action::NextComment).0, 4);
        let (line, effects) = step(&mut h, Action::NextComment);
        assert_eq!(line, 4, "末尾では動かない");
        assert!(matches!(effects.as_slice(), [Effect::Status(..)]));
        assert_eq!(step(&mut h, Action::PrevComment).0, 1);
    }

    #[test]
    fn 選択は起点から伸び上向きでも正規化される() {
        /// クリック列 (行, shift), 期待する範囲。
        type Case = (&'static [(usize, bool)], Option<(usize, usize)>);
        let cases: [Case; 4] = [
            (&[(7, false)], Some((7, 7))),
            (&[(5, false), (9, true)], Some((5, 9))),
            (&[(9, false), (4, true)], Some((4, 9))),
            (&[(3, true)], Some((3, 3))),
        ];
        for (clicks, expected) in cases {
            let mut selection = Selection::default();
            for (line, extend) in clicks {
                selection.click(*line, *extend);
            }
            assert_eq!(selection.range(), expected, "{clicks:?}");
        }
    }

    #[test]
    fn 選択の判定は両端を含む() {
        let mut selection = Selection::default();
        selection.click(3, false);
        selection.click(5, true);
        assert!(!selection.contains(2));
        assert!(selection.contains(3) && selection.contains(5));
        assert!(!selection.contains(6));
        selection.clear();
        assert!(selection.is_empty() && !selection.contains(4));
    }
}
