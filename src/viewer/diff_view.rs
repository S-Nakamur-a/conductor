//! unified diff 表示 — FileDiff からエントリ一覧を構築する処理、隠れた
//! コンテキスト領域の展開、diff モード/summary 表示のトグル。

use crate::diff_state::{DiffHunk, DiffLineTag, FileDiff};

use super::file_view::UnifiedDiffEntry;
use super::state::ViewerState;

impl ViewerState {
    /// viewer パネルの行番号領域が使う gutter の総幅（列数）を返す。行番号の
    /// 桁数に [crate::viewer::GUTTER_FIXED_W] を足したもの。
    pub fn gutter_total_width(&self) -> u16 {
        let digit_w = if self.diff_view.diff_mode {
            // renderer の gutter 幅と厳密に一致させないとマウスの当たり判定が 1 列ずれる。renderer が
            // 使う diff_view_max_line_no は折りたたまれた領域の new_line_end も数えるので、ここで
            // Line エントリだけから再計算すると桁数が足りない。
            digit_count(self.diff_view.diff_view_max_line_no)
        } else {
            digit_count(self.content.file_content.len())
        };
        (digit_w + crate::viewer::GUTTER_FIXED_W) as u16
    }

    /// FileDiff から unified diff 表示のエントリ一覧を構築する。
    ///
    /// hunk の間に ExpandableContext エントリを挿入し、必要に応じて展開できる
    /// 隠れたコンテキスト行を表す。
    pub fn build_unified_diff_view(&mut self, file_diff: &FileDiff) {
        self.diff_view.diff_view_lines.clear();

        let total_new_lines = self.content.file_content.len();

        let hunk_max_new_line = |hunk: &DiffHunk| -> usize {
            hunk.lines
                .iter()
                .filter_map(|l| l.new_line_no)
                .max()
                .unwrap_or(0)
        };
        let hunk_min_new_line = |hunk: &DiffHunk| -> usize {
            hunk.lines
                .iter()
                .filter_map(|l| l.new_line_no)
                .min()
                .unwrap_or(0)
        };

        for (hunk_idx, hunk) in file_diff.hunks.iter().enumerate() {
            if hunk_idx == 0 {
                let first_new = hunk_min_new_line(hunk);
                if first_new > 1 {
                    let hidden_start = 1;
                    let hidden_end = first_new - 1;
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::ExpandableContext {
                            hidden_count: hidden_end - hidden_start + 1,
                            new_line_start: hidden_start,
                            new_line_end: hidden_end,
                            func_header: hunk.func_header.clone(),
                        });
                }
            } else {
                let prev_hunk = &file_diff.hunks[hunk_idx - 1];
                let prev_end = hunk_max_new_line(prev_hunk);
                let curr_start = hunk_min_new_line(hunk);
                let hidden_start = prev_end + 1;
                let hidden_end = curr_start.saturating_sub(1);
                if hidden_start <= hidden_end {
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::ExpandableContext {
                            hidden_count: hidden_end - hidden_start + 1,
                            new_line_start: hidden_start,
                            new_line_end: hidden_end,
                            func_header: hunk.func_header.clone(),
                        });
                } else {
                    // 隠れた行が無い — 見た目上の区切りを残す。
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::HunkSeparator {
                            func_header: hunk.func_header.clone(),
                        });
                }
            }

            for line in &hunk.lines {
                self.diff_view.diff_view_lines.push(UnifiedDiffEntry::Line {
                    tag: line.tag,
                    new_line_no: line.new_line_no,
                    content: line.content.clone(),
                    inline_segments: line.inline_segments.clone(),
                });
            }
        }

        if let Some(last_hunk) = file_diff.hunks.last() {
            let last_new = hunk_max_new_line(last_hunk);
            if last_new < total_new_lines {
                let hidden_start = last_new + 1;
                let hidden_end = total_new_lines;
                self.diff_view
                    .diff_view_lines
                    .push(UnifiedDiffEntry::ExpandableContext {
                        hidden_count: hidden_end - hidden_start + 1,
                        new_line_start: hidden_start,
                        new_line_end: hidden_end,
                        func_header: None,
                    });
            }
        }

        self.recalc_diff_max_line_no();

        if !self.diff_view.diff_view_lines.is_empty() {
            self.diff_view.diff_mode = true;
            self.diff_view.diff_view_scroll = 0;
        }
    }

    fn recalc_diff_max_line_no(&mut self) {
        self.diff_view.diff_view_max_line_no = self
            .diff_view
            .diff_view_lines
            .iter()
            .filter_map(|e| match e {
                UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
                UnifiedDiffEntry::ExpandableContext { new_line_end, .. } => Some(*new_line_end),
                _ => None,
            })
            .max()
            .unwrap_or(0);
    }

    /// 現在の内容における最大行幅（文字数）を返す。
    ///
    /// diff モードでは diff_view_lines を走査し、それ以外では file_content を
    /// 走査する。表示すべきものが無い場合は 0 を返す。
    pub fn max_content_width(&self) -> usize {
        if self.diff_view.diff_mode {
            self.diff_view
                .diff_view_lines
                .iter()
                .map(|entry| match entry {
                    UnifiedDiffEntry::Line { content, .. } => content.chars().count(),
                    UnifiedDiffEntry::HunkSeparator { func_header }
                    | UnifiedDiffEntry::ExpandableContext { func_header, .. } => {
                        func_header.as_ref().map_or(0, |h| h.chars().count())
                    }
                })
                .max()
                .unwrap_or(0)
        } else {
            self.content
                .file_content
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
        }
    }

    /// h_scroll を delta だけ増やす。最も長い行を超えてスクロールしないようクランプする。
    pub fn scroll_right(&mut self, delta: usize) {
        let max_w = self.max_content_width();
        // 数文字だけ見える状態になるまでスクロールを許可する。
        let limit = max_w.saturating_sub(4);
        self.content.h_scroll = (self.content.h_scroll + delta).min(limit);
    }

    /// unified diff モードを終了する。summary 疑似ファイル表示からも抜ける — ファイルを開く
    /// あらゆる経路がここを通るので、show_summary と diff_mode が同時に立たないことを保証する。
    pub fn exit_diff_mode(&mut self) {
        self.diff_view.diff_mode = false;
        self.diff_view.diff_view_lines.clear();
        self.diff_view.diff_view_scroll = 0;
        self.diff_view.diff_view_max_line_no = 0;
        self.show_summary = false;
        self.summary_scroll = 0;
    }

    /// diff から素のファイル表示へ戻る。見ていた行はそのまま引き継ぐ。
    pub fn leave_diff_view(&mut self) {
        self.sync_file_scroll_to_diff_scroll();
        self.exit_diff_mode();
    }

    pub fn is_summary(&self) -> bool {
        self.show_summary
    }

    /// summary 疑似ファイル表示に入り、diff/ファイル内容から離れる。
    /// exit_diff_mode 経由で diff モードとは排他に保たれる。
    pub fn enter_summary_view(&mut self) {
        self.exit_diff_mode();
        self.show_summary = true;
        self.summary_scroll = 0;
    }

    /// diff_view_lines の指定インデックスにある、隠れたコンテキスト行を展開する。
    ///
    /// expand_all なら全て、そうでなければ先頭から 5 行 + 末尾から 5 行 (GitHub 方式の
    /// 双方向展開)。展開が起きたら true。
    pub fn expand_context_at(&mut self, idx: usize, expand_all: bool) -> bool {
        let entry = match self.diff_view.diff_view_lines.get(idx) {
            Some(UnifiedDiffEntry::ExpandableContext { .. }) => {
                self.diff_view.diff_view_lines[idx].clone()
            }
            _ => return false,
        };

        let (hidden_count, new_line_start, new_line_end, func_header) = match entry {
            UnifiedDiffEntry::ExpandableContext {
                hidden_count,
                new_line_start,
                new_line_end,
                func_header,
            } => (hidden_count, new_line_start, new_line_end, func_header),
            _ => unreachable!(),
        };

        if expand_all || hidden_count <= 10 {
            let mut new_entries: Vec<UnifiedDiffEntry> = Vec::with_capacity(hidden_count);
            for line_no in new_line_start..=new_line_end {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            let added = new_entries.len();
            self.diff_view
                .diff_view_lines
                .splice(idx..=idx, new_entries);

            if idx < self.diff_view.diff_view_scroll {
                let delta = added.saturating_sub(1);
                self.diff_view.diff_view_scroll += delta;
            }
        } else {
            let top_count = 5usize;
            let bottom_count = 5usize;

            let mut new_entries: Vec<UnifiedDiffEntry> =
                Vec::with_capacity(top_count + bottom_count + 1);

            for line_no in new_line_start..new_line_start + top_count {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            // 中間の残りぶんに対する、より小さい ExpandableContext。
            let remaining_start = new_line_start + top_count;
            let remaining_end = new_line_end - bottom_count;
            new_entries.push(UnifiedDiffEntry::ExpandableContext {
                hidden_count: remaining_end - remaining_start + 1,
                new_line_start: remaining_start,
                new_line_end: remaining_end,
                func_header,
            });

            for line_no in (new_line_end - bottom_count + 1)..=new_line_end {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            let added = new_entries.len();
            self.diff_view
                .diff_view_lines
                .splice(idx..=idx, new_entries);

            if idx < self.diff_view.diff_view_scroll {
                let delta = added.saturating_sub(1);
                self.diff_view.diff_view_scroll += delta;
            }
        }

        self.recalc_diff_max_line_no();
        true
    }

    pub fn find_visible_expandable(&self, viewport_height: usize) -> Option<usize> {
        let start = self.diff_view.diff_view_scroll;
        let end = (start + viewport_height).min(self.diff_view.diff_view_lines.len());
        for i in start..end {
            if matches!(
                self.diff_view.diff_view_lines.get(i),
                Some(UnifiedDiffEntry::ExpandableContext { .. })
            ) {
                return Some(i);
            }
        }
        None
    }
}

/// n の10進数の桁数を数える（最小1）。
fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    /// diff で見ていた行がそのまま素の表示の先頭に来る。先頭へ巻き戻ると、
    /// 変更箇所を探し直すことになる。
    #[test]
    fn diffを抜けても見ていた行はそのまま出す() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![UnifiedDiffEntry::Line {
            tag: DiffLineTag::Equal,
            new_line_no: Some(20),
            content: String::new(),
            inline_segments: Vec::new(),
        }];
        vs.diff_view.diff_view_scroll = 0;

        vs.leave_diff_view();

        assert!(!vs.diff_view.diff_mode);
        assert_eq!(vs.content.file_scroll, 19);
    }
}
