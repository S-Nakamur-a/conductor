//! grep の結果を ディレクトリ → ファイル → 一致 の木にする。
//!
//! 行は畳んだ状態に関わらず 1 度だけ組み立て、畳んだ枝は表示のときに読み飛ばす。
//! 行そのものが自分のキーを持つので、畳む対象を画面上の並びから逆算する必要がない。

use std::collections::{BTreeMap, HashSet};

use conductor_core::grep_search::GrepMatch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Dir {
        /// 木の中でのフルパス。畳んだ状態のキーでもある。
        path: String,
        name: String,
        depth: usize,
        matches: usize,
    },
    File {
        path: String,
        name: String,
        depth: usize,
        matches: usize,
    },
    Hit {
        depth: usize,
        index: usize,
    },
}

impl Row {
    pub fn depth(&self) -> usize {
        match self {
            Row::Dir { depth, .. } | Row::File { depth, .. } | Row::Hit { depth, .. } => *depth,
        }
    }

    /// 畳める行のキー。一致の行は畳めない。
    fn key(&self) -> Option<&str> {
        match self {
            Row::Dir { path, .. } | Row::File { path, .. } => Some(path),
            Row::Hit { .. } => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct SearchTree {
    matches: Vec<GrepMatch>,
    rows: Vec<Row>,
    collapsed: HashSet<String>,
}

impl SearchTree {
    pub fn build(matches: Vec<GrepMatch>) -> Self {
        let mut by_file: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, m) in matches.iter().enumerate() {
            by_file.entry(m.file_path.as_str()).or_default().push(i);
        }

        let mut rows = Vec::new();
        let mut open: Vec<&str> = Vec::new();
        for (path, hits) in &by_file {
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", *path));
            let segments: Vec<&str> = if dir.is_empty() {
                Vec::new()
            } else {
                dir.split('/').collect()
            };
            // 共通の親はそのまま、分かれたところから新しい見出しを足す。
            let shared = segments
                .iter()
                .zip(&open)
                .take_while(|(a, b)| a == b)
                .count();
            open.truncate(shared);
            for segment in &segments[shared..] {
                open.push(segment);
                rows.push(Row::Dir {
                    path: open.join("/"),
                    name: (*segment).to_string(),
                    depth: open.len() - 1,
                    matches: 0,
                });
            }
            rows.push(Row::File {
                path: (*path).to_string(),
                name: name.to_string(),
                depth: segments.len(),
                matches: hits.len(),
            });
            rows.extend(hits.iter().map(|&index| Row::Hit {
                depth: segments.len() + 1,
                index,
            }));
        }
        count_dirs(&mut rows);

        Self {
            matches,
            rows,
            collapsed: HashSet::new(),
        }
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 畳んだ枝を落とした行。選択の添字はこの並びを指す。
    pub fn visible(&self) -> Vec<&Row> {
        let mut out = Vec::new();
        let mut hidden_below: Option<usize> = None;
        for row in &self.rows {
            if hidden_below.is_some_and(|depth| row.depth() > depth) {
                continue;
            }
            hidden_below = None;
            out.push(row);
            if row.key().is_some_and(|k| self.collapsed.contains(k)) {
                hidden_below = Some(row.depth());
            }
        }
        out
    }

    pub fn row(&self, visible_index: usize) -> Option<&Row> {
        self.visible().get(visible_index).copied()
    }

    /// その行が指す一致。見出しの行なら None。
    pub fn hit(&self, visible_index: usize) -> Option<&GrepMatch> {
        match self.row(visible_index)? {
            Row::Hit { index, .. } => self.matches.get(*index),
            _ => None,
        }
    }

    /// 行のキーで畳んだ状態を引く。描画は行を持っているので添字に戻さない。
    pub fn collapsed_key(&self, key: &str) -> bool {
        self.collapsed.contains(key)
    }

    pub fn match_at(&self, index: usize) -> Option<&GrepMatch> {
        self.matches.get(index)
    }

    pub fn is_collapsed(&self, visible_index: usize) -> bool {
        self.row(visible_index)
            .and_then(Row::key)
            .is_some_and(|k| self.collapsed.contains(k))
    }

    pub fn set_collapsed(&mut self, visible_index: usize, collapsed: bool) {
        let Some(key) = self
            .row(visible_index)
            .and_then(Row::key)
            .map(str::to_string)
        else {
            return;
        };
        if collapsed {
            self.collapsed.insert(key);
        } else {
            self.collapsed.remove(&key);
        }
    }

    pub fn toggle(&mut self, visible_index: usize) {
        self.set_collapsed(visible_index, !self.is_collapsed(visible_index));
    }

    /// 自分より浅いか同じ深さの次の行。畳んだ部分木を飛び越えるのに使う。
    pub fn next_sibling(&self, visible_index: usize) -> Option<usize> {
        let rows = self.visible();
        let depth = rows.get(visible_index)?.depth();
        rows.iter()
            .enumerate()
            .skip(visible_index + 1)
            .find(|(_, row)| row.depth() <= depth)
            .map(|(i, _)| i)
    }

    /// その行を含む見出しの行。既に見出しなら 1 つ上の階層を返す。
    pub fn parent(&self, visible_index: usize) -> Option<usize> {
        let rows = self.visible();
        let depth = rows.get(visible_index)?.depth();
        (0..visible_index)
            .rev()
            .find(|&i| rows[i].depth() < depth && rows[i].key().is_some())
    }
}

/// 見出しの一致数は配下のファイルの合計。行は親が先に並んでいるので後ろから畳む。
fn count_dirs(rows: &mut [Row]) {
    for i in (0..rows.len()).rev() {
        let Row::Dir { depth, .. } = rows[i] else {
            continue;
        };
        let total: usize = rows[i + 1..]
            .iter()
            .take_while(|row| row.depth() > depth)
            .filter_map(|row| match row {
                Row::File { matches, .. } => Some(*matches),
                _ => None,
            })
            .sum();
        if let Row::Dir { matches, .. } = &mut rows[i] {
            *matches = total;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, line: usize) -> GrepMatch {
        GrepMatch {
            file_path: path.to_string(),
            line_number: line,
            line_content: format!("line {line}"),
            match_start: 0,
            match_end: 1,
        }
    }

    fn tree(paths: &[(&str, usize)]) -> SearchTree {
        SearchTree::build(paths.iter().map(|(p, l)| hit(p, *l)).collect())
    }

    /// 行を「深さ:名前」で見る。座標ではなく形を固定する。
    fn shape(tree: &SearchTree) -> Vec<String> {
        tree.visible()
            .iter()
            .map(|row| match row {
                Row::Dir { name, depth, .. } => format!("{depth}:{name}/"),
                Row::File { name, depth, .. } => format!("{depth}:{name}"),
                Row::Hit { depth, index } => format!("{depth}:#{index}"),
            })
            .collect()
    }

    #[test]
    fn 一致はディレクトリとファイルの下にまとまる() {
        let t = tree(&[("src/a.rs", 1), ("src/a.rs", 5), ("src/ui/b.rs", 2)]);
        assert_eq!(
            shape(&t),
            [
                "0:src/", "1:a.rs", "2:#0", "2:#1", "1:ui/", "2:b.rs", "3:#2"
            ]
        );
    }

    #[test]
    fn 根直下のファイルにディレクトリ行は要らない() {
        let t = tree(&[("README.md", 3)]);
        assert_eq!(shape(&t), ["0:README.md", "1:#0"]);
    }

    #[test]
    fn サブディレクトリしか持たないディレクトリも畳める() {
        let mut t = tree(&[("a/b/c.rs", 1)]);
        assert_eq!(shape(&t), ["0:a/", "1:b/", "2:c.rs", "3:#0"]);
        t.toggle(0);
        assert_eq!(shape(&t), ["0:a/"]);
    }

    #[test]
    fn 畳むと配下が隠れる() {
        let mut t = tree(&[("src/a.rs", 1), ("src/ui/b.rs", 2)]);
        t.toggle(1);
        assert_eq!(shape(&t), ["0:src/", "1:a.rs", "1:ui/", "2:b.rs", "3:#1"]);
        t.toggle(0);
        assert_eq!(shape(&t), ["0:src/"]);
    }

    #[test]
    fn 畳んでも選択はその見出しに残る() {
        let mut t = tree(&[("src/a.rs", 1), ("src/a.rs", 2)]);
        t.toggle(1);
        assert!(t.is_collapsed(1));
        assert!(matches!(t.row(1), Some(Row::File { .. })));
    }

    #[test]
    fn 各行は配下の一致数を数える() {
        let t = tree(&[("src/a.rs", 1), ("src/a.rs", 2), ("src/ui/b.rs", 3)]);
        let counts: Vec<usize> = t
            .visible()
            .iter()
            .filter_map(|row| match row {
                Row::Dir { matches, .. } | Row::File { matches, .. } => Some(*matches),
                Row::Hit { .. } => None,
            })
            .collect();
        assert_eq!(counts, [3, 2, 1, 1]);
    }

    #[test]
    fn 次の兄弟へは畳んだ部分木を飛び越える() {
        let mut t = tree(&[("src/a.rs", 1), ("src/a.rs", 2), ("src/b.rs", 3)]);
        assert_eq!(t.next_sibling(1), Some(4), "a.rs の一致 2 件を飛ばす");
        t.toggle(1);
        assert_eq!(t.next_sibling(1), Some(2), "畳めば隣に b.rs が来る");
        assert_eq!(t.next_sibling(2), None);
    }

    #[test]
    fn 親は自分を含む見出しの行() {
        let t = tree(&[("src/ui/b.rs", 2)]);
        assert_eq!(t.parent(3), Some(2), "一致 → ファイル");
        assert_eq!(t.parent(2), Some(1), "ファイル → ディレクトリ");
        assert_eq!(t.parent(0), None);
    }

    #[test]
    fn 一致が無ければ木は空になる() {
        let t = tree(&[]);
        assert!(t.is_empty());
        assert!(t.visible().is_empty());
        assert_eq!(t.hit(0), None);
    }
}
