//! ファイルツリーと Git Changes / コメント一覧。上下 2 区画で、キーを受けるのはどちらか一方。

pub mod git_changes;
pub mod render;
pub mod tree;

use std::path::{Path, PathBuf};

use conductor_core::diff_state::DiffState;
use conductor_core::keymap::{Action, KeyContext};

use crate::click::ClickTracker;
use crate::comment_list::CommentList;
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::list::{ListCursor, Viewport};
use crate::modal::{Modal, Prompt};
use crate::review::ReviewState;
use crate::task::{Task, TaskResult};
use crate::workspace::Ctx;

use git_changes::GitChanges;
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
    GitChanges,
    Comments,
}

#[derive(Debug, Default)]
pub struct ExplorerPanel {
    tree: FileTree,
    pane: Pane,
    bottom: BottomView,
    tree_cursor: ListCursor,
    pub changes: GitChanges,
    pub comments: CommentList,
    tree_view: Viewport,
    tree_clicks: ClickTracker,
    /// 投げたまま結果がまだ届いていないツリー読みの数。
    pending: usize,
}

impl ExplorerPanel {
    pub fn root(&self) -> &Path {
        self.tree.root()
    }

    pub fn tree(&self) -> &FileTree {
        &self.tree
    }

    pub fn diff(&self) -> &DiffState {
        self.changes.diff()
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

    pub fn tree_viewport(&self) -> Viewport {
        self.tree_view
    }

    pub fn key_context(&self) -> KeyContext {
        match (self.pane, self.bottom) {
            (Pane::Tree, _) => KeyContext::Explorer,
            (Pane::Bottom, BottomView::GitChanges) => self.changes.key_context(),
            (Pane::Bottom, BottomView::Comments) => KeyContext::ExplorerCommentList,
        }
    }

    pub fn sync_layout(&mut self, layout: &Layout) {
        if let Some(rect) = layout.rect(Region::ExplorerTree) {
            self.tree_view = Viewport::inside(rect, 0);
        }
        if let Some(rect) = layout.rect(Region::ExplorerChanges) {
            self.changes
                .set_viewport(Viewport::inside(rect, self.changes.banner_rows()));
            self.comments.set_viewport(Viewport::inside(rect, 0));
        }
    }

    /// worktree が変わった。base は作業ツリー差分の比較先。
    pub fn set_root(&mut self, root: PathBuf, base: &str) -> Vec<Effect> {
        if self.tree.root() == root {
            return Vec::new();
        }
        self.tree.set_root(root.clone());
        self.tree_cursor = ListCursor::default();
        self.changes.reset(root, base);
        self.refresh()
    }

    /// ツリーと diff を投げ直す。
    pub fn refresh(&mut self) -> Vec<Effect> {
        self.pending += 1;
        vec![
            Effect::Spawn(Task::LoadTree {
                root: self.tree.root().to_path_buf(),
                expanded: self.tree.expanded_dirs(),
            }),
            self.changes.reload(),
        ]
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        self.clamp();
        self.comments.clamp(ctx.review);
        match action {
            Action::ShowDiffList => {
                self.changes.show_files();
                return Some(self.show(BottomView::GitChanges));
            }
            Action::ShowCommitLog => return Some(self.show_commit_log()),
            Action::ShowCommentList => return Some(self.show(BottomView::Comments)),
            Action::ExitSubPanel if self.pane == Pane::Bottom => {
                if self.bottom != BottomView::GitChanges || !self.changes.leave_log() {
                    self.pane = Pane::Tree;
                }
                return Some(Vec::new());
            }
            _ => {}
        }
        match (self.pane, self.bottom) {
            (Pane::Tree, _) => self.tree_key(action),
            (Pane::Bottom, BottomView::GitChanges) => self.changes.update(action),
            (Pane::Bottom, BottomView::Comments) => self.comments.update(action, ctx.review),
        }
    }

    /// 一覧を替えると同時にフォーカスも下区画へ移す。見えない相手にキーが飛ぶと迷う。
    pub fn show(&mut self, view: BottomView) -> Vec<Effect> {
        self.bottom = view;
        self.pane = Pane::Bottom;
        Vec::new()
    }

    pub fn show_commit_log(&mut self) -> Vec<Effect> {
        self.show(BottomView::GitChanges);
        self.changes.show_log()
    }

    /// 中身が入れ替わっていることがあるので、窓の高さを知っているここで収め直す。
    fn clamp(&mut self) {
        self.tree_cursor
            .clamp(self.tree.visible().len(), self.tree_view);
        self.changes.clamp();
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

    /// 行のクリック。ディレクトリは開閉し、ファイルは preview で開いて 2 回目で固定する
    /// — preview のタブは 1 枚しか残らないので、開いたタブが溜まらない。
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
            let preview = !self.tree_clicks.is_double(row);
            return vec![self.open(&path, preview)];
        }
        match self.bottom {
            BottomView::Comments => {
                self.pane = Pane::Bottom;
                self.comments.click(y, review)
            }
            BottomView::GitChanges => match self.changes.click(y) {
                Some(effects) => {
                    self.pane = Pane::Bottom;
                    effects
                }
                None => Vec::new(),
            },
        }
    }

    /// ホイール。選択は動かさず窓だけ送る。
    pub fn scroll(&mut self, region: Region, delta: isize, review: &ReviewState) {
        match (region, self.bottom) {
            (Region::ExplorerChanges, BottomView::Comments) => self.comments.scroll(delta, review),
            (Region::ExplorerChanges, BottomView::GitChanges) => self.changes.scroll(delta),
            _ => self
                .tree_cursor
                .pan(delta, self.tree.visible().len(), self.tree_view),
        }
    }

    pub fn step_changed_file(&mut self, delta: isize) -> Option<Effect> {
        self.changes.step_file(delta)
    }

    pub fn open_changed(&mut self, path: &str, line: Option<usize>) -> Option<Effect> {
        self.changes.open_path(path, line)
    }

    /// あいまい検索で最も近いファイルを開く。
    pub fn find_file(&mut self, query: &str) -> Option<Effect> {
        let path = best_match(self.tree.all_files(), query)?.to_string();
        Some(self.open(&path, false))
    }

    pub fn reveal_in_tree(&mut self, path: &str) {
        if let Some(at) = self.tree.reveal(path) {
            self.tree_cursor
                .select(at, self.tree.visible().len(), self.tree_view);
        }
    }

    pub fn apply_result(&mut self, result: TaskResult) -> Vec<Effect> {
        match result {
            TaskResult::Tree(snapshot) => {
                self.pending = self.pending.saturating_sub(1);
                self.tree.install(*snapshot);
                self.clamp();
                Vec::new()
            }
            TaskResult::Diff(diff) => self.changes.install(*diff),
            TaskResult::HeadLog { skip, commits } => self.changes.install_log(skip, commits),
            _ => Vec::new(),
        }
    }
}

pub(crate) fn find_file_modal() -> Effect {
    Effect::PushModal(Modal::Prompt(Prompt::single(
        "Find file",
        |query| match query.trim() {
            "" => Vec::new(),
            query => vec![Effect::FindFile(query.to_string())],
        },
    )))
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
    use crate::workspace::{Focus, StatusLevel, Workspace};
    use conductor_core::diff_state::{DiffSource, FileDiff};

    fn file(path: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            added_lines: 1,
            deleted_lines: 0,
            hunks: Vec::new(),
        }
    }

    fn working_tree(paths: &[&str]) -> DiffState {
        let mut diff = DiffState::new(DiffSource::working_tree("main"));
        diff.files = paths.iter().map(|p| file(p)).collect();
        diff.rebuild_display_list();
        diff
    }

    fn with_changes(paths: &[&str]) -> Workspace {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::Explorer;
        ws.panels.explorer.changes.install(working_tree(paths));
        ws.panels.explorer.pane = Pane::Bottom;
        ws.panels
            .explorer
            .changes
            .set_viewport(Viewport::new(0, 20));
        ws
    }

    fn drive(ws: &mut Workspace, actions: &[Action]) {
        let mut svc = conductor_svc::Services::new();
        for action in actions {
            let effects = ws.dispatch(*action).unwrap_or_default();
            crate::effect::apply(ws, &mut svc, effects);
        }
    }

    fn selected(ws: &Workspace) -> usize {
        ws.panels.explorer.changes.cursor().selected()
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
            assert_eq!(selected(&ws), expected, "{actions:?}");
        }
    }

    #[test]
    fn 変更一覧のenterは出どころ付きのdiffを添えて開く() {
        let mut ws = with_changes(&["a.rs", "b.rs"]);
        let effects = ws.dispatch(Action::Select).unwrap();
        let [Effect::OpenFile { path, diff, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(path.to_str(), Some("a.rs"));
        let diff = diff.as_ref().unwrap();
        assert_eq!(diff.file.path, "a.rs");
        assert_eq!(diff.source, DiffSource::working_tree("main"));
    }

    #[test]
    fn ツリーの2回目のクリックは固定して開く() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        let mut ex = ExplorerPanel::default();
        ex.tree.install(tree::survey(dir.path(), &[]));
        ex.tree_view = Viewport::new(0, 20);
        let review = ReviewState::default();

        for want_preview in [true, false] {
            let effects = ex.click(0, &review);
            let [Effect::OpenFile { path, preview, .. }] = effects.as_slice() else {
                panic!("{effects:?}");
            };
            assert_eq!((path.to_str(), *preview), (Some("a.rs"), want_preview));
        }
    }

    #[test]
    fn 変更一覧の2回目のクリックは固定して開く() {
        let mut ws = with_changes(&["a.rs", "b.rs"]);
        let review = ReviewState::default();

        for want_preview in [true, false] {
            let effects = ws.panels.explorer.click(0, &review);
            let [Effect::OpenFile { path, preview, .. }] = effects.as_slice() else {
                panic!("{effects:?}");
            };
            assert_eq!((path.to_str(), *preview), (Some("a.rs"), want_preview));
        }
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
        assert_eq!(ws.panels.explorer.bottom(), BottomView::GitChanges);
        assert_eq!(ws.key_context(), KeyContext::ExplorerDiffList);

        drive(&mut ws, &[Action::ShowCommentList, Action::ExitSubPanel]);
        assert_eq!(ws.key_context(), KeyContext::Explorer, "escで上区画へ戻る");
    }

    #[test]
    fn diffの入れ替えは選択をパスで持ち越す() {
        let mut ws = with_changes(&["a.rs", "b.rs", "c.rs"]);
        drive(&mut ws, &[Action::GoToBottom]);
        assert_eq!(selected(&ws), 2);

        ws.panels
            .explorer
            .apply_result(TaskResult::Diff(Box::new(working_tree(&["c.rs"]))));
        assert_eq!(
            ws.panels
                .explorer
                .diff()
                .resolve_file(0)
                .map(|f| f.path.as_str()),
            Some("c.rs")
        );
        assert_eq!(selected(&ws), 0);
    }

    #[test]
    fn 頼んだ出どころと違うdiffは捨てる() {
        let mut ws = with_changes(&["a.rs"]);
        ws.panels
            .explorer
            .changes
            .set_source(DiffSource::commit("b"));

        let mut stale = DiffState::new(DiffSource::commit("a"));
        stale.files = vec![file("stale.rs")];
        stale.rebuild_display_list();
        ws.panels
            .explorer
            .apply_result(TaskResult::Diff(Box::new(stale)));
        assert!(ws.panels.explorer.changes.is_loading());
        assert_eq!(
            ws.panels.explorer.diff().files[0].path,
            "a.rs",
            "古い出どころの結果で上書きしない"
        );
    }

    #[test]
    fn worktreeを替えると出どころは作業ツリーに戻る() {
        let mut ws = with_changes(&["a.rs"]);
        ws.panels
            .explorer
            .changes
            .set_source(DiffSource::commit("b"));
        ws.panels
            .explorer
            .set_root(PathBuf::from("/tmp/elsewhere"), "develop");
        assert_eq!(
            ws.panels.explorer.changes.source(),
            &DiffSource::working_tree("develop")
        );
    }

    #[test]
    fn コミット一覧で選んだコミットが出どころになり完全なハッシュが出る() {
        let mut ws = with_changes(&["a.rs"]);
        drive(&mut ws, &[Action::ShowCommitLog]);
        assert_eq!(ws.key_context(), KeyContext::ExplorerCommitLog);

        let oid = "0123456789abcdef0123456789abcdef01234567";
        ws.panels.explorer.apply_result(TaskResult::HeadLog {
            skip: 0,
            commits: Ok(vec![git_changes::log::tests::commit(oid)]),
        });
        drive(&mut ws, &[Action::NavigateDown]);
        assert_eq!(
            ws.panels
                .explorer
                .changes
                .log()
                .selected_commit()
                .map(|c| c.oid.as_str()),
            Some(oid)
        );

        let effects = ws.dispatch(Action::Select).unwrap();
        let [
            Effect::Spawn(Task::ComputeDiff { source, .. }),
            Effect::Status(StatusLevel::Info, text),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(source, &DiffSource::commit(oid));
        assert!(text.starts_with(oid), "{text}");
        assert_eq!(
            ws.panels.explorer.changes.source(),
            &DiffSource::commit(oid)
        );
        assert_eq!(
            ws.key_context(),
            KeyContext::ExplorerDiffList,
            "ファイル一覧へ移る"
        );
    }

    /// ページが届く前に動かしたカーソルが、届いた瞬間に出どころの行へ戻ると、
    /// そのまま Enter を押した人は違う行を選ぶ。
    #[test]
    fn ページが届く前に動かしたカーソルは戻さない() {
        let mut ws = with_changes(&["a.rs"]);
        drive(&mut ws, &[Action::ShowCommitLog, Action::NavigateDown]);
        let commits = (0..3)
            .map(|i| git_changes::log::tests::commit(&format!("{i:040}")))
            .collect();
        ws.panels.explorer.apply_result(TaskResult::HeadLog {
            skip: 0,
            commits: Ok(commits),
        });
        assert_eq!(ws.panels.explorer.changes.log().cursor().selected(), 1);
    }

    #[test]
    fn コミット一覧のescはファイル一覧へ戻りもう1回でツリーへ() {
        let mut ws = with_changes(&["a.rs"]);
        drive(&mut ws, &[Action::ShowCommitLog, Action::ExitSubPanel]);
        assert_eq!(ws.key_context(), KeyContext::ExplorerDiffList);
        drive(&mut ws, &[Action::ExitSubPanel]);
        assert_eq!(ws.key_context(), KeyContext::Explorer);
    }

    #[test]
    fn 作業ツリーの行を選ぶと出どころが戻る() {
        let mut ws = with_changes(&["a.rs"]);
        ws.panels
            .explorer
            .changes
            .set_source(DiffSource::commit("b"));
        drive(&mut ws, &[Action::ShowCommitLog, Action::GoToTop]);
        let effects = ws.dispatch(Action::Select).unwrap();
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Spawn(Task::ComputeDiff { .. })]
            ),
            "{effects:?}"
        );
        assert_eq!(
            ws.panels.explorer.changes.source(),
            &DiffSource::working_tree("main")
        );
    }

    #[test]
    fn コミットを見ている間の外からの開く要求は作業ツリーへ戻してから開く() {
        let mut ws = with_changes(&["a.rs"]);
        ws.panels
            .explorer
            .changes
            .set_source(DiffSource::commit("b"));
        let effect = ws.panels.explorer.open_changed("a.rs", Some(3));
        assert!(
            matches!(&effect, Some(Effect::Spawn(Task::ComputeDiff { source, .. })) if source == &DiffSource::working_tree("main")),
            "{effect:?}"
        );

        let effects = ws
            .panels
            .explorer
            .apply_result(TaskResult::Diff(Box::new(working_tree(&["a.rs"]))));
        let [Effect::OpenChangedFile { path, line }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!((path.as_str(), *line), ("a.rs", Some(3)));
    }

    #[test]
    fn baseを解決できなければ理由をステータスに出す() {
        let mut ws = with_changes(&["a.rs"]);
        let mut broken = working_tree(&[]);
        broken.error = Some("no such ref".into());
        let effects = ws
            .panels
            .explorer
            .apply_result(TaskResult::Diff(Box::new(broken)));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ));
        assert_eq!(ws.panels.explorer.changes.banner_rows(), 1);
    }

    /// 覗くだけで選択が動くと、そのまま Enter を押したときに別のファイルが開く。
    #[test]
    fn ホイールは選択を動かさず窓だけ送る() {
        let mut ws = with_changes(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
        ws.panels.explorer.changes.set_viewport(Viewport::new(0, 2));
        ws.panels
            .explorer
            .scroll(Region::ExplorerChanges, 2, &ReviewState::default());
        assert_eq!(ws.panels.explorer.changes.cursor().scroll(), 2);
        assert_eq!(selected(&ws), 0);
        ws.panels
            .explorer
            .scroll(Region::ExplorerChanges, -9, &ReviewState::default());
        assert_eq!(ws.panels.explorer.changes.cursor().scroll(), 0);
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
