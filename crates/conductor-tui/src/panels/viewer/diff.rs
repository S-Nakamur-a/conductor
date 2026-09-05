//! unified diff 表示 — [conductor_core::diff_state::FileDiff] からエントリ列を組み立て、
//! ハンクの間に隠れた行を「展開できる塊」として挟む。左右 2 列表示はこの同じ列を
//! 並べ替えて作るので、行の出どころは 1 つしかない。

use conductor_core::diff_state::{DiffHunk, DiffLineTag, FileDiff, InlineSegment};

/// diff 表示の 1 エントリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// 隠れている行が無いハンクの境目。
    HunkSeparator { func_header: Option<String> },
    /// ハンクの間で隠れている行。
    ExpandableContext {
        hidden_count: usize,
        /// 新ファイル側で最初に隠れている行番号 (1 始まり)。
        new_line_start: usize,
        /// 新ファイル側で最後に隠れている行番号 (1 始まり、両端含む)。
        new_line_end: usize,
        func_header: Option<String>,
    },
    Line {
        tag: DiffLineTag,
        /// 追加行は旧ファイル側の行を持たない。左右 2 列の左側が読む。
        old_line_no: Option<usize>,
        /// 削除行は新ファイル側の行を持たない。
        new_line_no: Option<usize>,
        content: String,
        inline_segments: Vec<InlineSegment>,
    },
}

impl Entry {
    pub fn new_line_no(&self) -> Option<usize> {
        match self {
            Entry::Line { new_line_no, .. } => *new_line_no,
            Entry::ExpandableContext { new_line_start, .. } => Some(*new_line_start),
            Entry::HunkSeparator { .. } => None,
        }
    }

    fn tag(&self) -> Option<DiffLineTag> {
        match self {
            Entry::Line { tag, .. } => Some(*tag),
            _ => None,
        }
    }
}

/// diff 表示の状態。
#[derive(Debug, Default)]
pub struct DiffPane {
    pub active: bool,
    pub entries: Vec<Entry>,
    /// 左右 2 列で出すか。設定の `[diff] default_view` が初期値。
    pub side_by_side: bool,
    /// ガター幅の計算に使う。展開できる塊の末尾行も数えるので、Line だけからは求まらない。
    pub max_line_no: usize,
}

impl DiffPane {
    /// file_diff と、いま開いているファイルの行数からエントリ列を組む。
    pub fn build(&mut self, file_diff: &FileDiff, total_lines: usize) {
        self.entries.clear();
        for (i, hunk) in file_diff.hunks.iter().enumerate() {
            match i {
                0 => {
                    let first = min_new_line(hunk);
                    if first > 1 {
                        self.entries
                            .push(gap(1, first - 1, hunk.func_header.clone()));
                    }
                }
                _ => {
                    let start = max_new_line(&file_diff.hunks[i - 1]) + 1;
                    let end = min_new_line(hunk).saturating_sub(1);
                    self.entries.push(if start <= end {
                        gap(start, end, hunk.func_header.clone())
                    } else {
                        Entry::HunkSeparator {
                            func_header: hunk.func_header.clone(),
                        }
                    });
                }
            }
            self.entries
                .extend(hunk.lines.iter().map(|line| Entry::Line {
                    tag: line.tag,
                    old_line_no: line.old_line_no,
                    new_line_no: line.new_line_no,
                    content: line.content.clone(),
                    inline_segments: line.inline_segments.clone(),
                }));
        }

        if let Some(last) = file_diff.hunks.last() {
            let end = max_new_line(last);
            if end < total_lines {
                self.entries.push(gap(end + 1, total_lines, None));
            }
        }

        self.recalc_max_line_no();
        self.active = !self.entries.is_empty();
    }

    pub fn clear(&mut self) {
        self.active = false;
        self.entries.clear();
        self.max_line_no = 0;
    }

    /// idx の隠れた塊を開く。expand_all でなければ先頭 5 行 + 末尾 5 行だけ出し、
    /// 残りをより小さい塊として残す。開いたら true。
    pub fn expand(&mut self, idx: usize, expand_all: bool, source: &[String]) -> bool {
        let Some(Entry::ExpandableContext {
            hidden_count,
            new_line_start,
            new_line_end,
            func_header,
        }) = self.entries.get(idx).cloned()
        else {
            return false;
        };

        let line_entry = |no: usize| Entry::Line {
            tag: DiffLineTag::Equal,
            old_line_no: None,
            new_line_no: Some(no),
            content: source.get(no - 1).cloned().unwrap_or_default(),
            inline_segments: Vec::new(),
        };

        let replacement: Vec<Entry> = if expand_all || hidden_count <= 10 {
            (new_line_start..=new_line_end).map(line_entry).collect()
        } else {
            let head = new_line_start..new_line_start + 5;
            let tail = new_line_end - 4..=new_line_end;
            head.map(line_entry)
                .chain(std::iter::once(gap(
                    new_line_start + 5,
                    new_line_end - 5,
                    func_header,
                )))
                .chain(tail.map(line_entry))
                .collect()
        };

        self.entries.splice(idx..=idx, replacement);
        self.recalc_max_line_no();
        true
    }

    /// 窓の中にある最初の展開できる塊。
    pub fn visible_expandable(&self, scroll: usize, height: usize) -> Option<usize> {
        let end = (scroll + height).min(self.entries.len());
        (scroll..end).find(|i| matches!(self.entries[*i], Entry::ExpandableContext { .. }))
    }

    /// line_1 を先頭行に持つ展開できる塊の添字。
    pub fn expandable_at(&self, line_1: usize) -> Option<usize> {
        self.entries.iter().position(|e| {
            matches!(e, Entry::ExpandableContext { new_line_start, .. } if *new_line_start == line_1)
        })
    }

    fn recalc_max_line_no(&mut self) {
        self.max_line_no = self
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Line { new_line_no, .. } => *new_line_no,
                Entry::ExpandableContext { new_line_end, .. } => Some(*new_line_end),
                Entry::HunkSeparator { .. } => None,
            })
            .max()
            .unwrap_or(0);
    }
}

fn gap(start: usize, end: usize, func_header: Option<String>) -> Entry {
    Entry::ExpandableContext {
        hidden_count: end - start + 1,
        new_line_start: start,
        new_line_end: end,
        func_header,
    }
}

fn min_new_line(hunk: &DiffHunk) -> usize {
    hunk.lines
        .iter()
        .filter_map(|l| l.new_line_no)
        .min()
        .unwrap_or(0)
}

fn max_new_line(hunk: &DiffHunk) -> usize {
    hunk.lines
        .iter()
        .filter_map(|l| l.new_line_no)
        .max()
        .unwrap_or(0)
}

/// 左右 2 列の 1 行。
#[derive(Debug, PartialEq, Eq)]
pub enum SideRow<'a> {
    /// 行番号を持たない区切り。左右いっぱいに描く。
    Span(&'a Entry),
    Split {
        left: Option<&'a Entry>,
        right: Option<&'a Entry>,
    },
}

/// unified のエントリ列を左右 2 列に並べ替える。
///
/// 連続する削除と、その直後の連続する挿入を同じ高さで向かい合わせる。片方が短ければ
/// そちら側だけが空く。文脈行は両側に同じものを出す。
pub fn side_by_side(entries: &[Entry]) -> Vec<SideRow<'_>> {
    let mut rows = Vec::with_capacity(entries.len());
    let mut i = 0;
    while i < entries.len() {
        match entries[i].tag() {
            None => {
                rows.push(SideRow::Span(&entries[i]));
                i += 1;
            }
            Some(DiffLineTag::Equal) => {
                rows.push(SideRow::Split {
                    left: Some(&entries[i]),
                    right: Some(&entries[i]),
                });
                i += 1;
            }
            Some(_) => {
                let deletes = run(entries, i, DiffLineTag::Delete);
                let inserts = run(entries, deletes.end, DiffLineTag::Insert);
                for n in 0..(deletes.len()).max(inserts.len()) {
                    rows.push(SideRow::Split {
                        left: entries.get(deletes.start + n).filter(|_| n < deletes.len()),
                        right: entries.get(inserts.start + n).filter(|_| n < inserts.len()),
                    });
                }
                i = inserts.end;
            }
        }
    }
    rows
}

fn run(entries: &[Entry], from: usize, tag: DiffLineTag) -> std::ops::Range<usize> {
    let mut end = from;
    while entries.get(end).and_then(Entry::tag) == Some(tag) {
        end += 1;
    }
    from..end
}

/// 変更のかたまりの先頭か。
fn is_block_start(entries: &[Entry], idx: usize) -> bool {
    let changed = |i: usize| {
        matches!(
            entries.get(i).and_then(Entry::tag),
            Some(DiffLineTag::Insert | DiffLineTag::Delete)
        )
    };
    changed(idx) && (idx == 0 || !changed(idx - 1))
}

pub fn next_block(entries: &[Entry], from: usize) -> Option<usize> {
    (from + 1..entries.len()).find(|i| is_block_start(entries, *i))
}

pub fn prev_block(entries: &[Entry], from: usize) -> Option<usize> {
    (0..from).rev().find(|i| is_block_start(entries, *i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::diff_state::DiffLine;

    fn line(tag: DiffLineTag, new_line_no: Option<usize>, content: &str) -> DiffLine {
        DiffLine {
            tag,
            old_line_no: None,
            new_line_no,
            inline_segments: Vec::new(),
            content: content.into(),
        }
    }

    fn entry(tag: DiffLineTag, new_line_no: Option<usize>) -> Entry {
        Entry::Line {
            tag,
            old_line_no: None,
            new_line_no,
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }

    /// 20..=22 行目だけを触る diff。前後に隠れた行が残る。
    fn one_hunk() -> FileDiff {
        FileDiff {
            path: "a.rs".into(),
            added_lines: 1,
            deleted_lines: 1,
            hunks: vec![DiffHunk {
                lines: vec![
                    line(DiffLineTag::Equal, Some(20), "ctx"),
                    line(DiffLineTag::Delete, None, "old"),
                    line(DiffLineTag::Insert, Some(21), "new"),
                    line(DiffLineTag::Equal, Some(22), "ctx"),
                ],
                func_header: Some("fn f()".into()),
            }],
        }
    }

    #[test]
    fn ハンクの前後の隠れた行が塊になる() {
        let mut pane = DiffPane::default();
        pane.build(&one_hunk(), 40);
        assert!(pane.active);
        assert_eq!(
            pane.entries.first(),
            Some(&gap(1, 19, Some("fn f()".into())))
        );
        assert_eq!(pane.entries.last(), Some(&gap(23, 40, None)));
        assert_eq!(pane.max_line_no, 40);
    }

    #[test]
    fn 短い塊は全部開き長い塊は上下だけ開く() {
        let source: Vec<String> = (1..=40).map(|i| format!("line{i}")).collect();

        let mut pane = DiffPane::default();
        pane.build(&one_hunk(), 40);
        assert!(pane.expand(0, false, &source), "先頭の 19 行の塊");
        // 上 5 行 + 残りの塊 + 下 5 行。
        assert_eq!(pane.entries[0].new_line_no(), Some(1));
        assert_eq!(pane.entries[5], gap(6, 14, Some("fn f()".into())));
        assert_eq!(pane.entries[6].new_line_no(), Some(15));
        assert_eq!(
            pane.entries[0],
            Entry::Line {
                tag: DiffLineTag::Equal,
                old_line_no: None,
                new_line_no: Some(1),
                content: "line1".into(),
                inline_segments: Vec::new(),
            },
            "本文はいま開いているファイルから取る"
        );

        let mut pane = DiffPane::default();
        pane.build(&one_hunk(), 40);
        pane.expand(0, true, &source);
        assert_eq!(pane.entries[18].new_line_no(), Some(19));
        assert_eq!(pane.entries[19].new_line_no(), Some(20));
    }

    #[test]
    fn 塊が10行以下なら常に全部開く() {
        let mut pane = DiffPane {
            entries: vec![gap(1, 3, None)],
            ..DiffPane::default()
        };
        let source: Vec<String> = (1..=3).map(|i| format!("l{i}")).collect();
        pane.expand(0, false, &source);
        assert_eq!(pane.entries.len(), 3);
        assert!(
            !pane
                .entries
                .iter()
                .any(|e| matches!(e, Entry::ExpandableContext { .. }))
        );
    }

    #[test]
    fn 左右2列は削除と挿入を向かい合わせる() {
        let entries = vec![
            entry(DiffLineTag::Equal, Some(1)),
            entry(DiffLineTag::Delete, None),
            entry(DiffLineTag::Delete, None),
            entry(DiffLineTag::Insert, Some(2)),
            Entry::HunkSeparator { func_header: None },
        ];
        let rows = side_by_side(&entries);
        assert_eq!(rows.len(), 4);
        assert!(matches!(
            rows[0],
            SideRow::Split {
                left: Some(_),
                right: Some(_)
            }
        ));
        assert!(matches!(
            rows[1],
            SideRow::Split {
                left: Some(_),
                right: Some(_)
            }
        ));
        assert!(
            matches!(
                rows[2],
                SideRow::Split {
                    left: Some(_),
                    right: None
                }
            ),
            "削除の方が多ければ右が空く"
        );
        assert!(matches!(rows[3], SideRow::Span(_)));
    }

    #[test]
    fn 変更ブロックの移動は先頭だけに止まる() {
        let entries = vec![
            entry(DiffLineTag::Equal, Some(1)),
            entry(DiffLineTag::Insert, Some(2)),
            entry(DiffLineTag::Insert, Some(3)),
            entry(DiffLineTag::Equal, Some(4)),
            entry(DiffLineTag::Delete, None),
        ];
        assert_eq!(next_block(&entries, 0), Some(1));
        assert_eq!(next_block(&entries, 1), Some(4));
        assert_eq!(next_block(&entries, 4), None);
        assert_eq!(prev_block(&entries, 4), Some(1));
        assert_eq!(prev_block(&entries, 1), None);
    }
}
