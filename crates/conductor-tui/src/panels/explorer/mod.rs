//! ファイルツリーと変更ファイル一覧。上下 2 区画で、キーを受けるのはどちらか一方。

pub mod render;
pub mod tree;

use std::path::{Path, PathBuf};

use conductor_core::diff_state::{DiffListEntry, DiffState};
use conductor_core::keymap::{Action, KeyContext};

use crate::comment_list::CommentList;
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::list::{ListCursor, Viewport};
use crate::modal::{Modal, Prompt};
use crate::review::ReviewState;
use crate::task::{Task, TaskResult};
use crate::workspace::{Ctx, StatusLevel};

use tree::FileTree;

/// キーを受け取っている区画。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Tree,
    Bottom,
}

/// 下区画に何を出しているか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BottomView {
    #[default]
    Changes,
    Comments,
}

#[derive(Debug)]
pub struct ExplorerPanel {
    tree: FileTree,
    diff: DiffState,
    pane: Pane,
    bottom: BottomView,
    tree_cursor: ListCursor,
    changes_cursor: ListCursor,
    pub comments: CommentList,
    tree_view: Viewport,
    changes_view: Viewport,
    /// 投げたまま結果がまだ届いていない Task の数。
    pending: usize,
}

impl Default for ExplorerPanel {
    fn default() -> Self {
        Self {
            tree: FileTree::default(),
            diff: DiffState::new("main"),
            pane: Pane::default(),
            bottom: BottomView::default(),
            tree_cursor: ListCursor::default(),
            changes_cursor: ListCursor::default(),
            comments: CommentList::default(),
            tree_view: Viewport::default(),
            changes_view: Viewport::default(),
            pending: 0,
        }
    }
}

impl ExplorerPanel {
    pub fn root(&self) -> &Path {
        self.tree.root()
    }

    pub fn tree(&self) -> &FileTree {
        &self.tree
    }

    pub fn diff(&self) -> &DiffState {
        &self.diff
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    pub fn is_loading(&self) -> bool {
        self.pending > 0
    }

    pub fn bottom(&self) -> BottomView {
        self.bottom
    }

    pub fn tree_cursor(&self) -> ListCursor {
        self.tree_cursor
    }

    pub fn changes_cursor(&self) -> ListCursor {
        self.changes_cursor
    }

    pub fn tree_viewport(&self) -> Viewport {
        self.tree_view
    }

    pub fn changes_viewport(&self) -> Viewport {
        self.changes_view
    }

    pub fn key_context(&self) -> KeyContext {
        match (self.pane, self.bottom) {
            (Pane::Tree, _) => KeyContext::Explorer,
            (Pane::Bottom, BottomView::Changes) => KeyContext::ExplorerDiffList,
            (Pane::Bottom, BottomView::Comments) => KeyContext::ExplorerCommentList,
        }
    }

    pub fn sync_layout(&mut self, layout: &Layout) {
        if let Some(rect) = layout.rect(Region::ExplorerTree) {
            self.tree_view = Viewport::inside(rect, 0);
        }
        if let Some(rect) = layout.rect(Region::ExplorerChanges) {
            self.changes_view = Viewport::inside(rect, self.banner_rows());
            self.comments.set_viewport(Viewport::inside(rect, 0));
        }
    }

    /// 一覧の先頭でバナーが使う行数。
    pub fn banner_rows(&self) -> usize {
        usize::from(self.diff.error.is_some())
    }

    /// worktree が変わった。
    pub fn set_root(&mut self, root: PathBuf) -> Vec<Effect> {
        if self.tree.root() == root {
            return Vec::new();
        }
        self.tree.set_root(root);
        self.tree_cursor = ListCursor::default();
        self.changes_cursor = ListCursor::default();
        self.diff = DiffState::new(&self.diff.base_branch);
        self.refresh()
    }

    /// ツリーと diff を投げ直す。
    pub fn refresh(&mut self) -> Vec<Effect> {
        self.pending += 2;
        vec![
            Effect::Spawn(Task::LoadTree {
                root: self.tree.root().to_path_buf(),
                expanded: self.tree.expanded_dirs(),
            }),
            Effect::Spawn(Task::ComputeDiff {
                worktree: self.tree.root().to_path_buf(),
            }),
        ]
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        self.clamp();
        self.comments.clamp(ctx.review);
        match action {
            Action::ShowDiffList => return Some(self.show(BottomView::Changes)),
            Action::ShowCommentList => return Some(self.show(BottomView::Comments)),
            _ => {}
        }
        match (self.pane, self.bottom) {
            (Pane::Tree, _) => self.tree_key(action),
            (Pane::Bottom, BottomView::Changes) => self.changes_key(action),
            (Pane::Bottom, BottomView::Comments) => match action {
                Action::ExitSubPanel => {
                    self.pane = Pane::Tree;
                    Some(Vec::new())
                }
                _ => self.comments.update(action, ctx.review),
            },
        }
    }

    /// 一覧を替えると同時にフォーカスも下区画へ移す。見えない相手にキーが飛ぶと迷う。
    pub fn show(&mut self, view: BottomView) -> Vec<Effect> {
        self.bottom = view;
        self.pane = Pane::Bottom;
        Vec::new()
    }

    /// 中身が入れ替わっていることがあるので、窓の高さを知っているここで収め直す。
    fn clamp(&mut self) {
        self.tree_cursor
            .clamp(self.tree.visible().len(), self.tree_view);
        self.changes_cursor
            .clamp(self.diff.display_list.len(), self.changes_view);
    }

    fn tree_key(&mut self, action: Action) -> Option<Vec<Effect>> {
        let visible = self.tree.visible();
        let len = visible.len();
        match action {
            Action::NavigateDown => self.tree_cursor.step(1, len, self.tree_view),
            Action::NavigateUp => self.tree_cursor.step(-1, len, self.tree_view),
            Action::GoToTop => self.tree_cursor.select(0, len, self.tree_view),
            Action::GoToBottom => self.tree_cursor.select(usize::MAX, len, self.tree_view),
            Action::SearchFilename => return Some(vec![find_file_modal()]),
            Action::Select | Action::ExpandOrRight | Action::CollapseOrLeft => {
                let idx = *visible.get(self.tree_cursor.selected())?;
                let entry = self.tree.get(idx)?;
                if !entry.is_dir {
                    let path = entry.path.clone();
                    return (action == Action::Select).then(|| vec![self.open(&path, false)]);
                }
                match action {
                    Action::CollapseOrLeft => self.tree.collapse(idx),
                    Action::ExpandOrRight => self.tree.expand(idx),
                    _ => self.tree.toggle(idx),
                }
                self.tree_cursor
                    .clamp(self.tree.visible().len(), self.tree_view);
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn changes_key(&mut self, action: Action) -> Option<Vec<Effect>> {
        let len = self.diff.display_list.len();
        let row = self.changes_cursor.selected();
        match action {
            Action::ExitSubPanel => self.pane = Pane::Tree,
            Action::ToggleViewed => {
                let path = self.diff.resolve_file(row)?.path.clone();
                return Some(vec![Effect::ToggleViewed(path)]);
            }
            Action::NavigateDown => self.changes_cursor.step(1, len, self.changes_view),
            Action::NavigateUp => self.changes_cursor.step(-1, len, self.changes_view),
            Action::GoToTop => self.changes_cursor.select(0, len, self.changes_view),
            Action::GoToBottom => self
                .changes_cursor
                .select(usize::MAX, len, self.changes_view),
            Action::CollapseOrLeft => self.diff.collapse_section(row),
            Action::ExpandOrRight => self.diff.expand_section(row),
            Action::Select => return Some(self.activate_change(row)),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// ツリーが出すのはファイルの中身そのもので diff ではない。同じファイルを
    /// diff で見ている最中でも素の表示へ戻す。
    fn open(&self, path: &str, preview: bool) -> Effect {
        Effect::OpenFile {
            path: PathBuf::from(path),
            line: None,
            diff: None,
            preview,
        }
    }

    fn open_diff(&self, row: usize) -> Option<Effect> {
        let file = self.diff.resolve_file(row)?;
        Some(Effect::OpenFile {
            path: PathBuf::from(&file.path),
            line: None,
            diff: Some(Box::new(file.clone())),
            preview: false,
        })
    }

    /// 行のクリック。ファイルは preview で開き、ディレクトリは開閉する。
    /// 区画の外なら何も起きない。
    pub fn click(&mut self, y: u16, review: &ReviewState) -> Vec<Effect> {
        let visible = self.tree.visible();
        if let Some(row) = self.tree_cursor.index_at(y, visible.len(), self.tree_view) {
            self.pane = Pane::Tree;
            self.tree_cursor.select(row, visible.len(), self.tree_view);
            let Some(&idx) = visible.get(row) else {
                return Vec::new();
            };
            let Some(entry) = self.tree.get(idx) else {
                return Vec::new();
            };
            if entry.is_dir {
                self.tree.toggle(idx);
                return Vec::new();
            }
            let path = entry.path.clone();
            return vec![self.open(&path, true)];
        }
        if self.bottom == BottomView::Comments {
            self.pane = Pane::Bottom;
            return self.comments.click(y, review);
        }
        let len = self.diff.display_list.len();
        let Some(row) = self.changes_cursor.index_at(y, len, self.changes_view) else {
            return Vec::new();
        };
        self.pane = Pane::Bottom;
        self.changes_cursor.select(row, len, self.changes_view);
        self.activate_change(row)
    }

    /// 変更一覧の 1 行を発火させる。ディレクトリは開閉し、ファイルは diff を開く。
    fn activate_change(&mut self, row: usize) -> Vec<Effect> {
        match self.diff.display_list.get(row) {
            Some(DiffListEntry::Directory { .. }) => {
                self.diff.toggle_section(row);
                Vec::new()
            }
            Some(DiffListEntry::File { .. }) => self.open_diff(row).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// ホイール。選択は動かさず窓だけ送る。
    pub fn scroll(&mut self, region: Region, delta: isize, review: &ReviewState) {
        match region {
            Region::ExplorerChanges if self.bottom == BottomView::Comments => {
                self.comments.scroll(delta, review)
            }
            Region::ExplorerChanges => {
                self.changes_cursor
                    .pan(delta, self.diff.display_list.len(), self.changes_view)
            }
            _ => self
                .tree_cursor
                .pan(delta, self.tree.visible().len(), self.tree_view),
        }
    }

    /// 変更ファイル一覧を 1 つ送り、その diff を開く。ディレクトリ行は飛ばす。
    pub fn step_changed_file(&mut self, delta: isize) -> Option<Effect> {
        let files: Vec<usize> = self
            .diff
            .display_list
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, DiffListEntry::File { .. }))
            .map(|(i, _)| i)
            .collect();
        let row = self.changes_cursor.selected();
        let at = files.partition_point(|&i| i < row);
        let next = if delta > 0 {
            *files.get(at + usize::from(files.get(at) == Some(&row)))?
        } else {
            *files.get(at.checked_sub(1)?)?
        };
        self.changes_cursor
            .select(next, self.diff.display_list.len(), self.changes_view);
        self.open_diff(next)
    }

    /// 変更ファイルとして開く。折りたたまれたディレクトリの中にあれば先に展開する
    /// — 展開するまで表示行が無く、一覧の選択が動かせない。
    pub fn open_changed(&mut self, path: &str, line: Option<usize>) -> Option<Effect> {
        let path = self.diff.resolve_changed_path(path)?;
        let row = self.diff.reveal_path(&path)?;
        self.changes_cursor
            .select(row, self.diff.display_list.len(), self.changes_view);
        let file = self.diff.resolve_file(row)?;
        Some(Effect::OpenFile {
            path: PathBuf::from(&file.path),
            line,
            diff: Some(Box::new(file.clone())),
            preview: false,
        })
    }

    /// あいまい検索で最も近いファイルを開き、ツリー上でも選択する。
    pub fn find_file(&mut self, query: &str) -> Option<Effect> {
        let path = best_match(self.tree.all_files(), query)?.to_string();
        if let Some(at) = self.tree.reveal(&path) {
            self.tree_cursor
                .select(at, self.tree.visible().len(), self.tree_view);
        }
        Some(self.open(&path, false))
    }

    pub fn apply_result(&mut self, result: TaskResult) -> Vec<Effect> {
        match result {
            TaskResult::Tree(snapshot) => {
                self.pending = self.pending.saturating_sub(1);
                self.tree.install(*snapshot);
                self.clamp();
                Vec::new()
            }
            TaskResult::Diff(diff) => {
                self.pending = self.pending.saturating_sub(1);
                let selected = self
                    .diff
                    .resolve_file(self.changes_cursor.selected())
                    .map(|f| f.path.clone());
                self.diff = *diff;
                // 添字ではなくパスで選び直す。ファイルが増減しても指す先が動かない。
                if let Some(at) = selected.and_then(|p| self.diff.display_index_for_path(&p)) {
                    self.changes_cursor.place(at, self.diff.display_list.len());
                }
                self.clamp();
                match self.diff.error.clone() {
                    Some(e) => vec![Effect::Status(StatusLevel::Warning, e)],
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }
}

pub(crate) fn find_file_modal() -> Effect {
    Effect::PushModal(Modal::Prompt(Prompt {
        title: "Find file".into(),
        input: Default::default(),
        on_submit: |query| match query.trim() {
            "" => Vec::new(),
            query => vec![Effect::FindFile(query.to_string())],
        },
    }))
}

/// 部分列一致するもののうち最も点の高いパス。
fn best_match<'a>(paths: &'a [String], query: &str) -> Option<&'a str> {
    let query = query.to_lowercase();
    paths
        .iter()
        .filter_map(|path| Some((score(path, &query)?, path)))
        // 同点はパスの短い順。深い階層の同名ファイルより手前のものを先に出す。
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.len().cmp(&a.1.len())))
        .map(|(_, path)| path.as_str())
}

/// query の文字が順に現れなければ None。現れれば当たり方の良さを点にする。
fn score(path: &str, query: &str) -> Option<i32> {
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower).to_string();
    let mut chars = lower.chars();
    if !query.chars().all(|q| chars.any(|c| c == q)) {
        return None;
    }
    let mut score = 10;
    if name.starts_with(query) {
        score += 100;
    }
    if name.contains(query) {
        score += 50;
    }
    if lower.contains(query) {
        score += 30;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Focus, Workspace};
    use conductor_core::diff_state::FileDiff;

    fn file(path: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            added_lines: 1,
            deleted_lines: 0,
            hunks: Vec::new(),
        }
    }

    fn with_changes(paths: &[&str]) -> Workspace {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::Explorer;
        let mut diff = DiffState::new("main");
        diff.files = paths.iter().map(|p| file(p)).collect();
        diff.rebuild_display_list();
        ws.panels.explorer.diff = diff;
        ws.panels.explorer.pane = Pane::Bottom;
        ws.panels.explorer.changes_view = Viewport::new(0, 20);
        ws
    }

    fn drive(ws: &mut Workspace, actions: &[Action]) {
        let mut svc = conductor_svc::Services::new();
        for action in actions {
            let effects = ws.dispatch(*action).unwrap_or_default();
            crate::effect::apply(ws, &mut svc, effects);
        }
    }

    #[test]
    fn 区画ごとにキーマップの層が変わる() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::Explorer;
        assert_eq!(ws.key_context(), KeyContext::Explorer);
        drive(&mut ws, &[Action::ShowDiffList]);
        assert_eq!(ws.key_context(), KeyContext::ExplorerDiffList);
        drive(&mut ws, &[Action::ExitSubPanel]);
        assert_eq!(ws.key_context(), KeyContext::Explorer);
    }

    #[test]
    fn 変更一覧の移動は両端で止まる() {
        use Action::{GoToBottom, GoToTop, NavigateDown, NavigateUp};
        let cases: [(&[Action], usize); 5] = [
            (&[], 0),
            (&[NavigateDown], 1),
            (&[NavigateDown, NavigateDown, NavigateDown], 2),
            (&[NavigateUp], 0),
            (&[GoToBottom, GoToTop], 0),
        ];
        for (actions, expected) in cases {
            let mut ws = with_changes(&["a.rs", "b.rs", "c.rs"]);
            drive(&mut ws, actions);
            assert_eq!(
                ws.panels.explorer.changes_cursor.selected(),
                expected,
                "{actions:?}"
            );
        }
    }

    #[test]
    fn 変更一覧のenterはdiffを添えて開く() {
        let mut ws = with_changes(&["a.rs", "b.rs"]);
        let effects = ws.dispatch(Action::Select).unwrap();
        let [Effect::OpenFile { path, diff, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(path.to_str(), Some("a.rs"));
        assert_eq!(diff.as_ref().map(|d| d.path.as_str()), Some("a.rs"));
    }

    #[test]
    fn viewedは選択中のファイルの印を反転させる() {
        let mut ws = with_changes(&["a.rs"]);
        let effects = ws.dispatch(Action::ToggleViewed).unwrap();
        let [Effect::ToggleViewed(path)] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(path, "a.rs");
    }

    #[test]
    fn cとdで下区画の中身が入れ替わりキーの層も変わる() {
        let mut ws = with_changes(&["a.rs"]);
        ws.review.install(Ok(crate::review::Snapshot {
            branch: "main".into(),
            comments: vec![crate::review::tests::comment("a", "a.rs", 4, None)],
            ..crate::review::Snapshot::default()
        }));

        drive(&mut ws, &[Action::ShowCommentList]);
        assert_eq!(ws.panels.explorer.bottom(), BottomView::Comments);
        assert_eq!(ws.key_context(), KeyContext::ExplorerCommentList);

        let effects = ws.dispatch(Action::Select).unwrap();
        let [Effect::OpenFile { path, line, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!((path.to_str(), *line), (Some("a.rs"), Some(4)));

        drive(&mut ws, &[Action::ShowDiffList]);
        assert_eq!(ws.panels.explorer.bottom(), BottomView::Changes);
        assert_eq!(ws.key_context(), KeyContext::ExplorerDiffList);

        drive(&mut ws, &[Action::ShowCommentList, Action::ExitSubPanel]);
        assert_eq!(ws.key_context(), KeyContext::Explorer, "escで上区画へ戻る");
    }

    #[test]
    fn diffの入れ替えは選択をパスで持ち越す() {
        let mut ws = with_changes(&["a.rs", "b.rs", "c.rs"]);
        drive(&mut ws, &[Action::GoToBottom]);
        assert_eq!(ws.panels.explorer.changes_cursor.selected(), 2);

        let mut next = DiffState::new("main");
        next.files = vec![file("c.rs")];
        next.rebuild_display_list();
        ws.panels
            .explorer
            .apply_result(TaskResult::Diff(Box::new(next)));
        assert_eq!(
            ws.panels
                .explorer
                .diff
                .resolve_file(0)
                .map(|f| f.path.as_str()),
            Some("c.rs")
        );
        assert_eq!(ws.panels.explorer.changes_cursor.selected(), 0);
    }

    #[test]
    fn baseを解決できなければ理由をステータスに出す() {
        let mut ws = with_changes(&["a.rs"]);
        let mut broken = DiffState::new("main");
        broken.error = Some("no such ref".into());
        let effects = ws
            .panels
            .explorer
            .apply_result(TaskResult::Diff(Box::new(broken)));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ));
        assert_eq!(ws.panels.explorer.banner_rows(), 1);
    }

    /// 覗くだけで選択が動くと、そのまま Enter を押したときに別のファイルが開く。
    #[test]
    fn ホイールは選択を動かさず窓だけ送る() {
        let mut ws = with_changes(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
        ws.panels.explorer.changes_view = Viewport::new(0, 2);
        ws.panels
            .explorer
            .scroll(Region::ExplorerChanges, 2, &ReviewState::default());
        assert_eq!(ws.panels.explorer.changes_cursor.scroll(), 2);
        assert_eq!(ws.panels.explorer.changes_cursor.selected(), 0);
        ws.panels
            .explorer
            .scroll(Region::ExplorerChanges, -9, &ReviewState::default());
        assert_eq!(ws.panels.explorer.changes_cursor.scroll(), 0);
    }

    #[test]
    fn あいまい検索は前方一致を先に出す() {
        let paths: Vec<String> = ["src/main.rs", "src/manifest/mod.rs", "docs/manual.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for (query, expected) in [
            ("main", "src/main.rs"),
            ("manu", "docs/manual.md"),
            ("srcmod", "src/manifest/mod.rs"),
        ] {
            assert_eq!(best_match(&paths, query), Some(expected), "{query}");
        }
        assert_eq!(best_match(&paths, "zzz"), None);
    }
}
