//! AI レビューの読み順ビュー。左に読む順 (項目)、右にその順に並べた diff。
//!
//! アコーディオンの外にあり、main_area を 2 列に割って占有する。狭いペインでは
//! 「項目の説明と diff を同時に読む」という唯一の用途が成立しないため。

pub mod artifact;
pub mod render;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use conductor_core::config::Config;
use conductor_core::keymap::Action;
use conductor_core::theme::Theme;
use ratatui::layout::Rect;
use revidere::Scope;

use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::task::{AnalyzeOutcome, Task};
use crate::workspace::{Ctx, Focus, StatusLevel};

pub use artifact::{Loaded, Outcome, scope_label};

/// キーボードが今どちらの列にいるか。マウスと違って行き先を明示しないと決まらない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Column {
    #[default]
    Order,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    None,
    /// あるが、解析したときのコミットから先へ進んでいる。
    Stale,
    /// いまのコミットを見て作られている。
    Current,
}

#[derive(Debug, Default)]
pub struct RevidereState {
    review: Option<Box<Loaded>>,
    /// 「在るのに読めなかった」理由。読めた時と無い時は None。
    error: Option<String>,
    loading: bool,
    scope: Scope,
    selected: usize,
    column: Column,
    show_overview: bool,
    diff_scroll: usize,
    overview_scroll: usize,
    order_area: Rect,
    diff_area: Rect,
    cache: Option<render::Rendered>,
    /// 成果物の版。差し替えのたびに進み、組み立て済みの列の鍵に入る。
    epoch: u64,
    /// 実行中の解析。ブランチごとに高々 1 本なので worktree 同士が待ち合わせにならない。
    running: HashMap<String, Arc<AtomicBool>>,
}

impl RevidereState {
    pub fn review(&self) -> Option<&Loaded> {
        self.review.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn column(&self) -> Column {
        self.column
    }

    pub fn showing_overview(&self) -> bool {
        self.show_overview
    }

    pub fn show_overview(&mut self, on: bool) {
        self.show_overview = on;
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_running(&self, branch: &str) -> bool {
        self.running.contains_key(branch)
    }

    /// 確認ダイアログの文言と、貯めた応答を捨てるかどうかを決める 1 つの判定。
    ///
    /// 成果物が書き残す HEAD は短縮 oid、worktree 一覧が持つのは完全な oid。
    pub fn artifact(&self, head_oid: Option<&str>) -> Artifact {
        let Some(review) = self.review.as_ref() else {
            return Artifact::None;
        };
        let analysed = review.head();
        match head_oid {
            Some(head) if !analysed.is_empty() && head.starts_with(analysed) => Artifact::Current,
            _ => Artifact::Stale,
        }
    }

    pub fn reload(&mut self, worktree: PathBuf) -> Effect {
        self.loading = true;
        Effect::Spawn(Task::LoadRevidere {
            worktree,
            scope: self.scope,
        })
    }

    pub fn install(&mut self, outcome: Outcome) -> Vec<Effect> {
        self.loading = false;
        match outcome {
            Outcome::Missing => {
                self.replace(None);
                self.error = None;
                Vec::new()
            }
            Outcome::Loaded(review) => {
                self.replace(Some(review));
                self.error = None;
                Vec::new()
            }
            Outcome::Broken(why) => {
                self.replace(None);
                self.error = Some(why.clone());
                vec![Effect::Status(
                    StatusLevel::Error,
                    format!("Review artifact unreadable: {why}"),
                )]
            }
        }
    }

    /// 成果物を差し替え、そこに紐づく選択とスクロールを畳む。選択だけ残すと、
    /// 項目の数が減ったときに存在しない項目を指したままになる。
    fn replace(&mut self, review: Option<Box<Loaded>>) {
        self.review = review;
        self.selected = 0;
        self.column = Column::Order;
        self.diff_scroll = 0;
        // 別の成果物は別の概要。開いた直後と同じく概要から読ませる。
        self.show_overview = true;
        self.overview_scroll = 0;
        self.cache = None;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// 解析の中断旗を捕まえておく。走っている AI コマンドを終了時に kill するのと、
    /// 同じブランチに 2 本目を走らせないのに要る。
    pub fn note_spawned(&mut self, task: &Task) {
        if let Task::Analyze { branch, cancel, .. } = task {
            self.running.insert(branch.clone(), Arc::clone(cancel));
        }
    }

    /// 終了時に走っている解析を全部止める。これが無いと、メインループが止まった
    /// あとも AI コマンドが孤児として走り続ける。
    pub fn abort(&mut self) {
        for (_, cancel) in self.running.drain() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// 解析 1 本の終わり。文言にブランチ名を入れるのは、複数の worktree で同時に
    /// 走らせていると終わったのがいま見ているものとは限らないため。
    pub fn finished(
        &mut self,
        branch: &str,
        outcome: AnalyzeOutcome,
        worktree: PathBuf,
        selected_branch: &str,
    ) -> Vec<Effect> {
        self.running.remove(branch);
        let (message, level) = match &outcome {
            AnalyzeOutcome::Done {
                coverage_complete: true,
            } => (
                format!("Review ready for '{branch}'."),
                StatusLevel::Success,
            ),
            AnalyzeOutcome::Done {
                coverage_complete: false,
            } => (
                format!("Review ready for '{branch}', but some changed lines are unexplained."),
                StatusLevel::Warning,
            ),
            AnalyzeOutcome::Failed(why) => (
                format!("revidere failed for '{branch}': {why}"),
                StatusLevel::Error,
            ),
        };
        let mut effects = vec![Effect::Status(level, message)];
        // フォーカスは動かさない。数分待つ仕事なので、終わった頃には端末で打鍵している。
        if matches!(outcome, AnalyzeOutcome::Failed(_)) || branch != selected_branch {
            return effects;
        }
        effects.push(self.reload(worktree));
        effects
    }

    pub fn sync_layout(&mut self, layout: &Layout) {
        self.order_area = layout.rect(Region::RevidereOrder).unwrap_or_default();
        self.diff_area = layout.rect(Region::RevidereDiff).unwrap_or_default();
    }

    /// 折り返しと組み立ては 1 フレームに収まる仕事ではないので、幅・テーマ・成果物の
    /// どれかが変わったときだけ組み直す。
    pub fn prepare(&mut self, theme: &Theme, config: &Config) {
        let key = render::Key {
            order_width: self.order_area.width,
            diff_width: self.diff_area.width,
            theme: theme.name,
            epoch: self.epoch,
        };
        if self.cache.as_ref().map(|c| c.key) == Some(key) {
            return;
        }
        self.cache = self
            .review
            .as_deref()
            .map(|review| render::build(key, review, theme, config.viewer.tab_width));
    }

    pub(crate) fn cache(&self) -> Option<&render::Rendered> {
        self.cache.as_ref()
    }

    pub fn diff_scroll(&self) -> usize {
        self.diff_scroll
    }

    pub fn overview_scroll(&self) -> usize {
        self.overview_scroll
    }

    fn sections(&self) -> usize {
        self.review.as_ref().map_or(0, |r| r.order.sections.len())
    }

    fn select(&mut self, index: usize) {
        if index >= self.sections() {
            return;
        }
        self.selected = index;
        if let Some(row) = self.cache.as_ref().and_then(|c| c.section_rows.get(index)) {
            self.diff_scroll = *row;
        }
    }

    fn step(&mut self, delta: isize) {
        let len = self.sections();
        if len == 0 {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, len as isize - 1);
        self.select(next as usize);
    }

    fn scroll_diff(&mut self, delta: isize) {
        let max = self
            .cache
            .as_ref()
            .map_or(0, |c| c.diff_lines.len().saturating_sub(1));
        self.diff_scroll = (self.diff_scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    fn scroll_overview(&mut self, delta: isize) {
        let max = self
            .cache
            .as_ref()
            .map_or(0, |c| c.overview_lines.len().saturating_sub(1));
        self.overview_scroll =
            (self.overview_scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    /// ホイールはフォーカスを動かさないので、どの列の上かは区画で決まる。
    pub fn scroll(&mut self, region: Region, delta: isize) {
        match (region, self.show_overview) {
            (_, true) => self.scroll_overview(delta),
            (Region::RevidereOrder, _) => self.step(delta.signum()),
            (_, _) => self.scroll_diff(delta),
        }
    }

    pub fn click(&mut self, region: Region, y: u16) {
        if region != Region::RevidereOrder || self.show_overview {
            return;
        }
        let inner = crate::list::inner(self.order_area);
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let scroll = cache.order_scroll(self.selected, inner.height as usize);
        let row = y.saturating_sub(inner.y) as usize + scroll;
        let Some(index) = cache.item_of_row.get(row).copied() else {
            return;
        };
        self.column = Column::Order;
        self.select(index);
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        // 画面の切り替えは行き先ごとにキーが分かれているので、どちらを出していても
        // 押した結果は同じ。先に受けてしまってよい。
        match action {
            Action::RevidereShowOverview => {
                self.show_overview = true;
                return Some(Vec::new());
            }
            Action::RevidereShowSections => {
                self.show_overview = false;
                return Some(Vec::new());
            }
            Action::ExitSubPanel => return Some(vec![Effect::Focus(Focus::Explorer)]),
            Action::RevidereToggleScope => {
                return Some(self.toggle_scope(ctx.root.to_path_buf()));
            }
            _ => {}
        }

        // 概要は 1 列で読むだけの画面なので、項目に関わるキーは効かない。
        if self.show_overview {
            return Some(match action {
                Action::NavigateDown => {
                    self.scroll_overview(1);
                    Vec::new()
                }
                Action::NavigateUp => {
                    self.scroll_overview(-1);
                    Vec::new()
                }
                Action::GoToTop => {
                    self.overview_scroll = 0;
                    Vec::new()
                }
                // 概要を読み終えたら次は項目。読む順の入口として enter も通す。
                Action::Select | Action::ExpandOrRight => {
                    self.show_overview = false;
                    Vec::new()
                }
                _ => return None,
            });
        }

        Some(match action {
            Action::NavigateDown if self.column == Column::Order => {
                self.step(1);
                Vec::new()
            }
            Action::NavigateUp if self.column == Column::Order => {
                self.step(-1);
                Vec::new()
            }
            Action::NavigateDown => {
                self.scroll_diff(1);
                Vec::new()
            }
            Action::NavigateUp => {
                self.scroll_diff(-1);
                Vec::new()
            }
            Action::RevidereNextSection => {
                self.step(1);
                Vec::new()
            }
            Action::ReviderePrevSection => {
                self.step(-1);
                Vec::new()
            }
            Action::GoToTop => {
                self.select(0);
                self.diff_scroll = 0;
                Vec::new()
            }
            Action::GoToBottom => {
                self.select(self.sections().saturating_sub(1));
                Vec::new()
            }
            Action::ExpandOrRight => {
                self.column = Column::Diff;
                Vec::new()
            }
            Action::CollapseOrLeft => {
                self.column = Column::Order;
                Vec::new()
            }
            // 左列では diff へ 1 段入り、右列ではその位置を Viewer で開く。
            // コメントを書けるのは Viewer なので、ここが唯一の橋になる。
            Action::Select if self.column == Column::Order => {
                self.column = Column::Diff;
                Vec::new()
            }
            Action::Select => self.open_in_viewer(),
            _ => return None,
        })
    }

    /// 選択中の項目が指す位置を通常の Viewer で開く。
    ///
    /// 着地先はその項目が最初に持っている変更行で、借りた文脈行は飛ばす — 項目の
    /// 話の中心は、借りた行ではなく持ち物の行にある。
    fn open_in_viewer(&self) -> Vec<Effect> {
        let Some(placed) = self
            .review
            .as_ref()
            .and_then(|review| review.order.sections.get(self.selected))
        else {
            return Vec::new();
        };
        let target = placed.blocks.iter().find_map(|block| {
            let line = block.lines.iter().find(|l| l.owned)?;
            Some((
                block.path.clone(),
                line.line.new_line.or(line.line.old_line),
            ))
        });
        let Some((path, line)) = target else {
            return vec![Effect::Status(
                StatusLevel::Warning,
                "This section has no line to open (file-level change only).".into(),
            )];
        };
        vec![Effect::OpenChangedFile {
            path,
            line: line.map(|n| n as usize),
        }]
    }

    /// 切り替えた結果は画面が名乗るので、ここではステータスに出さない。
    pub fn toggle_scope(&mut self, worktree: PathBuf) -> Vec<Effect> {
        self.scope = match self.scope {
            Scope::Base => Scope::SincePrevious,
            Scope::SincePrevious => Scope::Base,
        };
        // 区間ごとに読みかけの位置は別物。持ち越すと、行数の違う diff の途中に
        // いきなり着地する。
        self.replace(None);
        vec![self.reload(worktree)]
    }
}

#[cfg(test)]
mod tests;
