//! 開いているファイルを読む場所。素の本文と unified / 左右 2 列の diff。
//!
//! 相対パスの解決先はここが持つ。書き換えるのは [ViewerPanel::set_root] だけで、
//! 表示中のツリーと開く先が食い違う窓を作らない。

pub mod code_nav;
pub mod content;
pub mod diff;
pub mod fold;
pub mod hover;
mod input;
pub mod media;
mod mouse;
pub mod render;
pub mod search;
pub mod syntax;
pub mod tabs;
pub mod thread;

use std::path::{Path, PathBuf};

use conductor_core::diff_state::OpenDiff;
use conductor_core::keymap::KeyContext;
use ratatui::layout::Rect;

use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::task::{Task, TaskResult};
use crate::workspace::StatusLevel;

use code_nav::CodeNav;
use content::Content;
use diff::DiffPane;
use fold::FoldState;
pub use mouse::Selection;
use search::Search;
use syntax::Highlighter;
use tabs::{Tab, TabStatus};
use thread::ThreadFolds;

/// 半ページの行数。
const HALF_PAGE: isize = 15;

/// 横スクロールの 1 回分。
const H_STEP: usize = 4;

/// スクロール位置。素の本文と diff で別々に覚える。
#[derive(Debug, Default, Clone, Copy)]
pub struct Scroll {
    /// 素の本文の最上行 (0 始まり)。Viewer は独立したカーソルを持たず、これが兼ねる。
    pub line: usize,
    pub column: usize,
    /// diff のエントリ列への添字。
    pub diff: usize,
    /// レンダリング済み markdown の最上行。折り返し後の行なので line とは別。
    pub md: usize,
}

/// 読み込みを投げてから結果が返るまでの覚え書き。
#[derive(Debug)]
struct Load {
    seq: u64,
    path: String,
    /// 読み終わってから組む。エントリの末尾はファイルの行数で決まる。
    diff: Option<Box<OpenDiff>>,
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
    pub nav: CodeNav,
    /// 素のソースではなく文章として描く。ファイルをまたいで持続する — 読み物を
    /// 続けて開くたびに押し直させない。
    md_rendered: bool,
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
            nav: CodeNav::default(),
            md_rendered: false,
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

    /// 構文定義とテーマ。トランスクリプトも markdown を描くので同じものを見る —
    /// 別に持たせると SyntaxSet が 2 つ載る。
    pub(crate) fn highlighter(&mut self, config: &conductor_core::config::Config) -> &Highlighter {
        let highlighter = self
            .highlighter
            .get_or_insert_with(|| Highlighter::new(config));
        highlighter.adopt(config);
        highlighter
    }

    /// 描画は `&Workspace` しか持てず [Self::highlighter] を呼べないので、構築済みのものだけを読む。
    pub(crate) fn highlighter_ref(&self) -> Option<&Highlighter> {
        self.highlighter.as_ref()
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
        self.reveal_tab(self.tab_strip_width(self.tab_row.width));
    }

    /// Raw と Rendered のトグルが意味を持つか。素の markdown ファイルを開いている
    /// ときだけ — unified diff は本文を組み直すと +/- の構造が壊れる。
    pub fn markdown_toggle_available(&self) -> bool {
        !self.diff.active
            && self.content.error.is_none()
            && self
                .content
                .path
                .as_deref()
                .is_some_and(crate::markdown::is_markdown_path)
    }

    /// レンダリング済み markdown を描いているか。行に紐づく機能は全てこれで判定する。
    pub fn is_showing_rendered_markdown(&self) -> bool {
        self.md_rendered && self.markdown_toggle_available()
    }

    /// 開いているファイルを埋め込みエディタへ渡す。開いていなければ何も起こさない。
    fn open_in_editor(&self) -> Vec<Effect> {
        match self.active_path() {
            Some(path) => vec![Effect::OpenInEditor(self.root.join(path))],
            None => vec![Effect::Status(
                StatusLevel::Warning,
                "no file open to edit".into(),
            )],
        }
    }

    /// 素のソースとレンダリング表示を切り替える。切り替えたら必ず先頭に着地し、
    /// 行に紐づいた途中の操作 (選択、ホバー、ジャンプ先の札) は畳む。
    pub fn toggle_markdown(&mut self) -> Vec<Effect> {
        if !self.markdown_toggle_available() {
            return vec![Effect::Status(
                StatusLevel::Info,
                "only a markdown file can be rendered".into(),
            )];
        }
        self.md_rendered = !self.md_rendered;
        self.scroll.md = 0;
        self.selection.clear();
        self.nav.hover = None;
        self.nav.labels = None;
        self.pending_fold = false;
        Vec::new()
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
        file_diff: Option<Box<OpenDiff>>,
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
        self.request(relative, file_diff, line, !fresh)
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
        file_diff: Option<Box<OpenDiff>>,
        line: Option<usize>,
        keep_scroll: bool,
    ) -> Vec<Effect> {
        let reveal = Effect::RevealInTree(path.clone());
        if media::is_image_path(&path) {
            self.show_image(path);
            return vec![reveal];
        }
        self.seq += 1;
        let task = Task::LoadFile {
            root: self.root.clone(),
            path: path.clone(),
            seq: self.seq,
            diff_of: file_diff.as_ref().map(|d| d.source.clone()),
        };
        self.load = Some(Load {
            seq: self.seq,
            path,
            diff: file_diff,
            line,
            keep_scroll,
        });
        vec![reveal, Effect::Spawn(task)]
    }

    /// 画像へ切り替える。描くのは区画の大きさが分かってからなので、ここでは
    /// 空けるだけ ([ViewerPanel::prepare] が実際の描画を頼む)。
    fn show_image(&mut self, path: String) {
        self.load = None;
        self.clear_for_new_file();
        self.content.path = Some(path);
        self.content.media = Some(media::Preview::Loading);
        self.content.media_key = None;
        self.scroll = Scroll::default();
    }

    /// 前のファイルの残りを落とす。読み込みの成否によらず先に通る。
    fn clear_for_new_file(&mut self) {
        self.content.lines.clear();
        self.content.error = None;
        self.content.highlighted.clear();
        self.content.highlight_key = None;
        self.content.rendered.clear();
        self.content.rendered_key = None;
        self.content.tests.clear();
        self.content.media = None;
        self.search.matches.clear();
        self.selection.clear();
        self.diff.clear();
        self.threads.clear();
        self.fold.clear();
    }

    pub fn apply_result(&mut self, result: TaskResult) -> Vec<Effect> {
        match result {
            TaskResult::FileLoaded { seq, loaded } => self.accept_file(seq, loaded),
            TaskResult::MediaRendered { key, rendered } => self.accept_media(key, rendered),
            _ => Vec::new(),
        }
    }

    /// 描き終えた絵を受け取る。頼んだときと鍵が違えば (別のファイルへ移った、
    /// 区画の大きさが変わった) 捨てる。
    fn accept_media(
        &mut self,
        key: media::Key,
        rendered: Result<Box<media::Rendered>, String>,
    ) -> Vec<Effect> {
        if self.content.media_key.as_ref() != Some(&key) {
            return Vec::new();
        }
        self.content.media = Some(match rendered {
            Ok(rendered) => media::Preview::Ready(rendered),
            Err(reason) => media::Preview::Failed(reason),
        });
        Vec::new()
    }

    fn accept_file(&mut self, seq: u64, loaded: Result<content::Loaded, String>) -> Vec<Effect> {
        let Some(load) = self.load.take_if(|load| load.seq == seq) else {
            return Vec::new();
        };

        self.clear_for_new_file();
        self.content.path = Some(load.path.clone());

        match loaded {
            Ok(file) => {
                self.content.lines = file.lines;
                self.content.tests = file.tests;
                self.fold.install(file.folds, &load.path);
                self.nav.reset_for_file(file.mask);
            }
            Err(reason) => {
                self.content.error = Some(reason);
                self.nav.reset_for_file(Default::default());
            }
        }

        let last = self.content.lines.len().saturating_sub(1);
        self.scroll.line = if load.keep_scroll {
            self.scroll.line.min(last)
        } else {
            0
        };
        self.scroll.diff = 0;
        self.scroll.md = 0;
        if let Some(diff) = load.diff {
            self.diff.build(&diff.file, self.content.lines.len());
        }
        if let Some(line) = load.line {
            self.goto_line(line);
        }
        Vec::new()
    }

    /// 1 始まりの行へ寄せる。畳んで隠れていれば開いてから。
    ///
    /// レンダリング表示にはソースの行が無いので、ここで素のソースへ抜ける。
    /// 行を指す経路 (ジャンプ、検索、コメント送り、file:line) は全部ここを通るので、
    /// 抜け忘れると要求された行が黙って無視される。
    fn goto_line(&mut self, line_1: usize) {
        self.md_rendered = false;
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

    /// 描く直前に、本文から導かれる重い成果物を整える。render は状態を書けないので
    /// ここでやる。画像だけは戻り値の Task に載る (デコードは UI スレッドで走らせない)。
    pub fn prepare(
        &mut self,
        config: &conductor_core::config::Config,
        theme: &conductor_core::theme::Theme,
    ) -> Vec<Effect> {
        if self.content.media.is_some() {
            return self.request_image();
        }
        if self.content.lines.is_empty() {
            return Vec::new();
        }
        let rendered = self.is_showing_rendered_markdown();
        let width = render::md_width(self.body);
        let highlighter = self
            .highlighter
            .get_or_insert_with(|| Highlighter::new(config));
        highlighter.adopt(config);
        if rendered {
            refresh_markdown(&mut self.content, highlighter, theme, width);
        } else {
            refresh_highlight(&mut self.content, highlighter);
        }
        Vec::new()
    }

    /// 区画の大きさが変わっていれば描き直しを頼む。同じ鍵なら何もしない。
    fn request_image(&mut self) -> Vec<Effect> {
        let Some(path) = self.content.path.clone() else {
            return Vec::new();
        };
        let (cols, rows) = render::media_area(self.body);
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let key = (path.clone(), cols, rows);
        if self.content.media_key.as_ref() == Some(&key) {
            return Vec::new();
        }
        self.content.media_key = Some(key);
        self.content.media = Some(media::Preview::Loading);
        vec![Effect::Spawn(Task::RenderMedia {
            root: self.root.clone(),
            path,
            cols,
            rows,
        })]
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

fn refresh_highlight(content: &mut Content, highlighter: &Highlighter) {
    let key = syntax::cache_key(highlighter.id(), content.path.as_deref(), &content.lines);
    if content.highlight_key == Some(key) {
        return;
    }
    content.highlighted = highlighter.highlight(content.path.as_deref(), &content.lines);
    content.highlight_key = Some(key);
}

/// 折り返し幅も配色も描画のたびに変わりうるので、指紋に畳んで比べる。
fn refresh_markdown(
    content: &mut Content,
    highlighter: &Highlighter,
    theme: &conductor_core::theme::Theme,
    width: usize,
) {
    let id = format!("{}|{}|{width}", highlighter.id(), theme.name);
    let key = syntax::cache_key(&id, content.path.as_deref(), &content.lines);
    if content.rendered_key == Some(key) {
        return;
    }
    content.rendered = crate::markdown::render(
        &content.lines.join("\n"),
        width,
        theme,
        highlighter.syntax_set(),
        highlighter.theme(),
        crate::markdown::Flavor::Rich,
    );
    content.rendered_key = Some(key);
}

#[cfg(test)]
mod tests;
