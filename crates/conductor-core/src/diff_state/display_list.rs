//! 変更ファイルを explorer に出すディレクトリツリーに平坦化し、行とファイルを相互に引く。

use std::collections::{BTreeMap, HashSet};

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

    fn emit(
        &self,
        prefix: &str,
        depth: usize,
        collapsed_dirs: &HashSet<String>,
        out: &mut Vec<DiffListEntry>,
    ) {
        for (name, dir) in &self.subdirs {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let collapsed = collapsed_dirs.contains(&path);
            out.push(DiffListEntry::Directory {
                name: name.clone(),
                path: path.clone(),
                depth,
                collapsed,
            });
            if !collapsed {
                dir.emit(&path, depth + 1, collapsed_dirs, out);
            }
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
        root.emit("", 0, &self.collapsed_dirs, &mut self.display_list);
    }

    /// ディレクトリ行、サマリー行、範囲外は None。
    pub fn resolve_file(&self, display_idx: usize) -> Option<&FileDiff> {
        match self.display_list.get(display_idx)? {
            DiffListEntry::File { file_index, .. } => self.files.get(*file_index),
            DiffListEntry::Directory { .. } | DiffListEntry::Summary => None,
        }
    }

    /// [Self::resolve_file] の逆引き。折りたたまれたディレクトリの中のファイルは見えない。
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

    /// 外から来たパス (revidere の節、コメントのアンカー) を diff 側の綴りに直す。
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

    /// path の行が見えるよう折りたたまれた祖先を展開し、その行の位置を返す。
    /// diff に無いファイルなら None。
    pub fn reveal_path(&mut self, path: &str) -> Option<usize> {
        if let Some(idx) = self.display_index_for_path(path) {
            return Some(idx);
        }
        let ancestors: Vec<&str> = path
            .match_indices('/')
            .map(|(slash, _)| &path[..slash])
            .collect();
        let mut changed = false;
        for ancestor in ancestors {
            changed |= self.collapsed_dirs.remove(ancestor);
        }
        if changed {
            self.rebuild_display_list();
        }
        self.display_index_for_path(path)
    }

    /// ディレクトリ行なら折りたたみを反転して true。他の行では何もしない。
    pub fn toggle_section(&mut self, display_idx: usize) -> bool {
        match self.display_list.get(display_idx) {
            Some(DiffListEntry::Directory { collapsed, .. }) => {
                let collapse = !*collapsed;
                self.set_collapsed(display_idx, collapse)
            }
            _ => false,
        }
    }

    pub fn collapse_section(&mut self, display_idx: usize) {
        self.set_collapsed(display_idx, true);
    }

    pub fn expand_section(&mut self, display_idx: usize) {
        self.set_collapsed(display_idx, false);
    }

    fn set_collapsed(&mut self, display_idx: usize, collapse: bool) -> bool {
        let Some(DiffListEntry::Directory {
            path, collapsed, ..
        }) = self.display_list.get(display_idx)
        else {
            return false;
        };
        if *collapsed == collapse {
            return false;
        }
        let path = path.clone();
        if collapse {
            self.collapsed_dirs.insert(path);
        } else {
            self.collapsed_dirs.remove(&path);
        }
        self.rebuild_display_list();
        true
    }
}
