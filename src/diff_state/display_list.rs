//! DiffState のフラット化された explorer 表示リストの構築とナビゲーション:
//! コミット済み/未コミットを統合したディレクトリツリー、セクションの折りたたみ/展開、
//! 表示リストのインデックスとファイル参照との相互解決を扱う。

use std::collections::{BTreeMap, HashSet};

use super::model::{DiffListEntry, DiffState, DiffViewMode, FileDiff};

impl DiffState {
    /// 空の DiffState を新規作成する。
    pub fn new(base_branch: &str, view_mode: DiffViewMode) -> Self {
        let mut state = Self {
            files: Vec::new(),
            display_list: Vec::new(),
            collapsed_dirs: HashSet::new(),
            scroll: 0,
            view_mode,
            base_branch: base_branch.to_string(),
            error: None,
            has_summary: false,
        };
        state.rebuild_display_list();
        state
    }

    /// フラット化した表示リストを再構築する。
    pub fn rebuild_display_list(&mut self) {
        self.display_list.clear();

        // 変更サマリーの疑似ファイル。存在すれば最上部に固定表示する。
        if self.has_summary {
            self.display_list.push(DiffListEntry::Summary {});
        }

        Self::build_tree_entries(&self.files, &self.collapsed_dirs, &mut self.display_list);
    }

    /// 変更ファイルからディレクトリツリーを構築する。
    fn build_tree_entries(
        files: &[FileDiff],
        collapsed_dirs: &HashSet<String>,
        display_list: &mut Vec<DiffListEntry>,
    ) {
        // ツリーの葉: 元の files のどのインデックスに解決されるかを保持する。
        struct Leaf {
            index: usize,
            path: String,
        }
        let mut leaves: Vec<Leaf> = files
            .iter()
            .enumerate()
            .map(|(index, f)| Leaf {
                index,
                path: f.path.clone(),
            })
            .collect();
        leaves.sort_by(|a, b| a.path.cmp(&b.path));

        // ディレクトリパスと、そこに直接属する葉のインデックスを集める。
        let mut dir_set: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut top_level: Vec<usize> = Vec::new();
        for (li, leaf) in leaves.iter().enumerate() {
            if let Some(slash) = leaf.path.rfind('/') {
                dir_set
                    .entry(leaf.path[..slash].to_string())
                    .or_default()
                    .push(li);
            } else {
                top_level.push(li);
            }
        }

        // すべての祖先ディレクトリがノードとして存在することを保証する。
        let all_dirs: Vec<String> = dir_set.keys().cloned().collect();
        for dir in &all_dirs {
            let mut current = dir.as_str();
            while let Some(slash) = current.rfind('/') {
                let parent = &current[..slash];
                dir_set.entry(parent.to_string()).or_default();
                current = parent;
            }
        }

        struct TreeNode {
            child_dirs: Vec<String>,
            leaves: Vec<usize>,
        }
        let mut nodes: BTreeMap<String, TreeNode> = BTreeMap::new();
        for dir_path in dir_set.keys() {
            nodes.entry(dir_path.clone()).or_insert_with(|| TreeNode {
                child_dirs: Vec::new(),
                leaves: Vec::new(),
            });
        }
        for (dir_path, leaf_indices) in &dir_set {
            if let Some(node) = nodes.get_mut(dir_path) {
                node.leaves = leaf_indices.clone();
            }
        }
        let dir_paths: Vec<String> = nodes.keys().cloned().collect();
        let mut root_dirs: Vec<String> = Vec::new();
        for dir_path in &dir_paths {
            if let Some(slash) = dir_path.rfind('/') {
                let parent = &dir_path[..slash];
                if let Some(parent_node) = nodes.get_mut(parent) {
                    parent_node.child_dirs.push(dir_path.clone());
                } else {
                    root_dirs.push(dir_path.clone());
                }
            } else {
                root_dirs.push(dir_path.clone());
            }
        }
        root_dirs.sort();
        for node in nodes.values_mut() {
            node.child_dirs.sort();
            // leaves は全体ソートの結果、既にパス/出自順になっている。
        }

        fn emit_dir(
            dir_path: &str,
            depth: usize,
            leaves: &[Leaf],
            nodes: &BTreeMap<String, TreeNode>,
            collapsed_dirs: &HashSet<String>,
            display_list: &mut Vec<DiffListEntry>,
        ) {
            let name = dir_path.rsplit('/').next().unwrap_or(dir_path).to_string();
            let is_collapsed = collapsed_dirs.contains(dir_path);
            display_list.push(DiffListEntry::Directory {
                path: dir_path.to_string(),
                name,
                depth,
                collapsed: is_collapsed,
            });
            if is_collapsed {
                return;
            }
            if let Some(node) = nodes.get(dir_path) {
                for child_dir in &node.child_dirs {
                    emit_dir(
                        child_dir,
                        depth + 1,
                        leaves,
                        nodes,
                        collapsed_dirs,
                        display_list,
                    );
                }
                for &li in &node.leaves {
                    display_list.push(DiffListEntry::File {
                        file_index: leaves[li].index,
                        depth: depth + 1,
                    });
                }
            }
        }

        for dir_path in &root_dirs {
            emit_dir(dir_path, 0, &leaves, &nodes, collapsed_dirs, display_list);
        }
        for &li in &top_level {
            display_list.push(DiffListEntry::File {
                file_index: leaves[li].index,
                depth: 0,
            });
        }
    }

    /// 表示リストのインデックスをファイル参照に解決する。
    ///
    /// ディレクトリ行、サマリー行、範囲外のインデックスの場合は None を返す。
    pub fn resolve_file(&self, display_idx: usize) -> Option<&FileDiff> {
        match self.display_list.get(display_idx)? {
            DiffListEntry::File { file_index, .. } => self.files.get(*file_index),
            DiffListEntry::Directory { .. } | DiffListEntry::Summary {} => None,
        }
    }

    /// リポジトリ相対パスから表示リストのインデックスを探す
    /// ([Self::resolve_file] の逆引き)。リストのインデックスではなくパスで
    /// ファイルを開いた際(例: 節が指す位置へジャンプする場合)に、
    /// diff リストのカーソルを同期させておくために使う。
    pub fn display_index_for_path(&self, path: &str) -> Option<usize> {
        (0..self.display_list.len())
            .find(|&idx| self.resolve_file(idx).is_some_and(|f| f.path == path))
    }

    /// diff 内の全ての変更パス。[Self::display_index_for_path] と違い表示リストを
    /// 無視するので、折りたたまれたディレクトリ内のファイルも存在するものとして数える。
    pub fn changed_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }

    /// 外部から渡されたパス(revidere の節が指すパス、コメントの
    /// アンカーなど)を diff と突き合わせ、diff 側の正式な表記を返す。
    ///
    /// まず完全一致を試みる。これにより実在するファイルが常に以下のどの推測よりも
    /// 優先される。次に、正しいファイルを違う書き方で指しているケースを扱う:
    ///
    /// 1. 正規化した表記(./src/a.rs, src//a.rs など)。
    /// 2. 先頭セグメントを除いたパス。これは git diff の a//b/ プレフィックスを
    ///    取り除く処理に相当する。完全一致の後にしか試さないため、本当にトップレベルに
    ///    b/ を持つリポジトリを誤って覆い隠すことはない。
    /// 3. セグメント境界での一意なサフィックス一致。サブディレクトリからの相対パス
    ///    (src/app/foo.rs に対する app/foo.rs など)を拾うための処理。曖昧な場合は
    ///    不採用にする。末尾が同じファイルが2つあれば、どちらを指しているか分からず、
    ///    推測で決めるとレビュアーを何の手がかりもなく誤ったファイルへ送ってしまう。
    pub fn resolve_changed_path(&self, path: &str) -> Option<String> {
        let paths = self.changed_paths();
        let exact = |candidate: &str| -> Option<String> {
            paths
                .iter()
                .find(|p| **p == candidate)
                .map(|p| (*p).to_string())
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
            (Some(only), None) => Some((*only).to_string()),
            _ => None,
        }
    }

    /// path に行が存在するよう、折りたたまれている祖先ディレクトリを展開し、
    /// その行の表示インデックスを返す。
    ///
    /// これがないと、レビュアーが折りたたんでいたディレクトリ内のファイルへの
    /// ジャンプは、そのファイルが diff に存在しない場合と見分けがつかなくなる。
    /// [Self::display_index_for_path] は表示リストに今ある行しか見えないためだ。
    pub fn reveal_path(&mut self, path: &str) -> Option<usize> {
        if let Some(idx) = self.display_index_for_path(path) {
            return Some(idx);
        }
        let mut changed = false;
        let mut prefix_end = 0;
        while let Some(slash) = path[prefix_end..].find('/') {
            prefix_end += slash;
            changed |= self.collapsed_dirs.remove(&path[..prefix_end]);
            prefix_end += 1;
        }
        if changed {
            self.rebuild_display_list();
        }
        self.display_index_for_path(path)
    }

    /// 指定した表示インデックスのディレクトリの折りたたみ状態を切り替える。
    /// ディレクトリを切り替えた場合は true を返す(呼び出し側がリストの変化を
    /// 検知できるように)。ディレクトリ以外の行(ファイル、サマリー)は何もしない。
    pub fn toggle_section(&mut self, display_idx: usize) -> bool {
        if let Some(DiffListEntry::Directory { path, .. }) = self.display_list.get(display_idx) {
            let key = path.clone();
            if self.collapsed_dirs.contains(&key) {
                self.collapsed_dirs.remove(&key);
            } else {
                self.collapsed_dirs.insert(key);
            }
            self.rebuild_display_list();
            true
        } else {
            false
        }
    }

    /// 指定した表示インデックスのディレクトリを折りたたむ(他の行では何もしない)。
    pub fn collapse_section(&mut self, display_idx: usize) {
        if let Some(DiffListEntry::Directory {
            path, collapsed, ..
        }) = self.display_list.get(display_idx)
            && !collapsed
        {
            let key = path.clone();
            self.collapsed_dirs.insert(key);
            self.rebuild_display_list();
        }
    }

    /// 指定した表示インデックスのディレクトリを展開する(他の行では何もしない)。
    pub fn expand_section(&mut self, display_idx: usize) {
        if let Some(DiffListEntry::Directory {
            path, collapsed, ..
        }) = self.display_list.get(display_idx)
            && *collapsed
        {
            let key = path.clone();
            self.collapsed_dirs.remove(&key);
            self.rebuild_display_list();
        }
    }

    // ヘルパー

    /// タブ文字をスペースに展開する。viewer のタブ展開と挙動を合わせる。
    pub fn expand_tabs(line: &str, tab_width: usize) -> String {
        if !line.contains('\t') {
            return line.to_string();
        }
        let mut result = String::with_capacity(line.len());
        let mut col = 0;
        for ch in line.chars() {
            if ch == '\t' {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    result.push(' ');
                }
                col += spaces;
            } else {
                result.push(ch);
                col += 1;
            }
        }
        result
    }
}
