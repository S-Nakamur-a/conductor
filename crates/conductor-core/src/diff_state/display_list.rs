//! 変更ファイルをディレクトリツリーの行に平坦化し、行とファイルを相互に引く。

use std::collections::BTreeMap;

use super::{DiffListEntry, DiffState, FileDiff};

#[derive(Default)]
struct Dir {
    subdirs: BTreeMap<String, Dir>,
    file_indices: Vec<usize>,
}

impl Dir {
    fn insert(&mut self, path: &str, file_index: usize) {
        match path.split_once('/') {
            Some((head, rest)) => self
                .subdirs
                .entry(head.to_string())
                .or_default()
                .insert(rest, file_index),
            None => self.file_indices.push(file_index),
        }
    }

    fn emit(&self, depth: usize, out: &mut Vec<DiffListEntry>) {
        for (name, dir) in &self.subdirs {
            let mut name = name.clone();
            let mut dir = dir;
            // 子が 1 つしかない階層は 1 行に畳む。深い一本道が行と字下げを食い潰すため。
            while dir.file_indices.is_empty() && dir.subdirs.len() == 1 {
                let Some((child, only)) = dir.subdirs.iter().next() else {
                    break;
                };
                name = format!("{name}/{child}");
                dir = only;
            }
            out.push(DiffListEntry::Directory { name, depth });
            dir.emit(depth + 1, out);
        }
        for &file_index in &self.file_indices {
            out.push(DiffListEntry::File { file_index, depth });
        }
    }
}

impl DiffState {
    pub fn rebuild_display_list(&mut self) {
        self.display_list.clear();
        if self.has_summary {
            self.display_list.push(DiffListEntry::Summary);
        }
        let mut sorted: Vec<(usize, &str)> = self
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f.path.as_str()))
            .collect();
        sorted.sort_by_key(|&(_, path)| path);
        let mut root = Dir::default();
        for (index, path) in sorted {
            root.insert(path, index);
        }
        root.emit(0, &mut self.display_list);
    }

    /// ディレクトリ行、サマリー行、範囲外は None。
    pub fn resolve_file(&self, display_idx: usize) -> Option<&FileDiff> {
        match self.display_list.get(display_idx)? {
            DiffListEntry::File { file_index, .. } => self.files.get(*file_index),
            DiffListEntry::Directory { .. } | DiffListEntry::Summary => None,
        }
    }

    /// [Self::resolve_file] の逆引き。
    pub fn display_index_for_path(&self, path: &str) -> Option<usize> {
        (0..self.display_list.len())
            .find(|&idx| self.resolve_file(idx).is_some_and(|f| f.path == path))
    }

    /// 表示リストと無関係に、diff にある全パス。
    pub fn changed_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }

    /// 外から来たパス (コメントのアンカー) を diff 側の綴りに直す。
    ///
    /// 完全一致、正規化した綴り、git diff の a/ b/ 接頭辞を落とした形、セグメント境界での
    /// 末尾一致の順に試す。末尾一致は一意なときだけ採る。2 つに当たるなら推測で決めるより
    /// 見つからない方がよく、完全一致を先にするのは本当にトップレベルに b/ を持つ
    /// リポジトリを隠さないため。
    pub fn resolve_changed_path(&self, path: &str) -> Option<String> {
        let paths = self.changed_paths();
        let exact = |candidate: &str| {
            paths
                .iter()
                .find(|p| **p == candidate)
                .map(|p| p.to_string())
        };

        if let Some(hit) = exact(path) {
            return Some(hit);
        }
        let normalized = crate::repo_path::normalize(path);
        if normalized.is_empty() {
            return None;
        }
        if let Some(hit) = exact(&normalized) {
            return Some(hit);
        }
        if let Some((_, rest)) = normalized.split_once('/')
            && !rest.is_empty()
            && let Some(hit) = exact(rest)
        {
            return Some(hit);
        }
        let suffix = format!("/{normalized}");
        let mut matches = paths.iter().filter(|p| p.ends_with(&suffix));
        match (matches.next(), matches.next()) {
            (Some(only), None) => Some(only.to_string()),
            _ => None,
        }
    }
}
