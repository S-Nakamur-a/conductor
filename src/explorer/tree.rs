//! ファイルツリーの構築とナビゲーション — ファイルシステムを歩いてフラットな
//! Vec<FileTreeEntry> を作る、ディレクトリの子要素の遅延読み込み、展開/折りたたみ、
//! ツリー中のパスの reveal。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::git_engine::status_map::GitStatusMap;
use crate::icons::{dir_icon, file_icon};

use super::{Explorer, FileTreeState};
use crate::viewer::{FileTreeEntry, FilenameSearchState};

/// ツリーを読み直した結果、Viewer 側でやるべきこと。
///
/// Explorer は ViewerState を知らないので、後始末は呼び出し側 (App の
/// 配線層) が行う。
#[must_use]
pub struct TreeReload {
    /// 根が変わった。新しい根に無いファイルのタブを閉じること。
    pub root_changed: bool,
    /// 開いていたファイルの、新しいツリーでの相対パス。読み直すこと。
    pub reopen: Option<String>,
    /// 可視エントリの集合が変わった。`root_changed` とは別物で、同じ根への再走査でも
    /// ファイルの増減があれば真になる。3 秒ポーリングはこれを見て再描画をスキップする。
    pub entries_changed: bool,
}

impl FileTreeState {
    /// file_tree のインデックス idx にあるディレクトリの展開/折りたたみを切り替える。
    pub fn toggle_dir(&mut self, idx: usize) {
        if let Some(entry) = self.file_tree.get_mut(idx)
            && entry.is_dir
        {
            entry.is_expanded = !entry.is_expanded;
            self.invalidate_visible_cache();
        }
    }

    /// インデックス idx のディレクトリを展開する。ファイルエントリなら何もしない。
    pub fn expand_dir(&mut self, idx: usize) {
        if let Some(entry) = self.file_tree.get_mut(idx)
            && entry.is_dir
            && !entry.is_expanded
        {
            entry.is_expanded = true;
            self.invalidate_visible_cache();
        }
    }

    /// インデックス idx のディレクトリを折りたたむ。ファイルエントリなら何もしない。
    pub fn collapse_dir(&mut self, idx: usize) {
        if let Some(entry) = self.file_tree.get_mut(idx)
            && entry.is_dir
            && entry.is_expanded
        {
            entry.is_expanded = false;
            self.invalidate_visible_cache();
        }
    }

    /// キャッシュ済みの可視インデックスを無効化する。ツリー構造が変わるたび
    /// （展開/折りたたみ、子の読み込み、ツリーの再読み込み）に必ず呼ぶこと。
    pub fn invalidate_visible_cache(&mut self) {
        *self.cached_visible_indices.borrow_mut() = None;
    }

    /// 現在可視な file_tree のインデックス一覧。invalidate_visible_cache() まで Rc で
    /// キャッシュするので、同一フレーム内の繰り返し呼び出しは実質コストゼロ。
    ///
    /// 純粋なメモ化なので内部可変にしてある。`&mut` を要求すると描画が共有借用しか持てず、
    /// 「描く前に温めておく」契約が呼び出し側に生まれ、忘れると静かに空の一覧が描かれる。
    pub fn visible_indices(&self) -> Rc<Vec<usize>> {
        if let Some(cached) = self.cached_visible_indices.borrow().as_ref() {
            return Rc::clone(cached);
        }

        let mut result = Vec::with_capacity(self.file_tree.len());
        let mut skip_depth: Option<usize> = None;

        for (i, entry) in self.file_tree.iter().enumerate() {
            if let Some(sd) = skip_depth {
                if entry.depth > sd {
                    continue;
                } else {
                    skip_depth = None;
                }
            }

            result.push(i);

            if entry.is_dir && !entry.is_expanded {
                skip_depth = Some(entry.depth);
            }
        }

        let rc = Rc::new(result);
        *self.cached_visible_indices.borrow_mut() = Some(Rc::clone(&rc));
        rc
    }

    /// ディレクトリの直接の子要素を遅延読み込みする。ディレクトリでない、または読み込み
    /// 済みなら何もしない。
    pub fn ensure_children_loaded(&mut self, idx: usize) {
        let (rel_path, child_depth) = match self.file_tree.get(idx) {
            Some(e) if e.is_dir && !e.children_loaded => (e.path.clone(), e.depth + 1),
            _ => return,
        };

        let full_path = self.root.join(&rel_path);

        let mut children: Vec<FileTreeEntry> = Vec::new();
        Explorer::walk_dir(
            &self.root,
            &full_path,
            child_depth,
            &mut children,
            &self.git_status,
        );

        self.file_tree[idx].children_loaded = true;

        if children.is_empty() {
            return;
        }

        let insert_pos = idx + 1;
        self.file_tree.splice(insert_pos..insert_pos, children);
        self.invalidate_visible_cache();
    }
}

impl Explorer {
    /// 表示中のツリーを歩いた根。エントリの相対パスはここに繋いで絶対パスにする。
    pub fn root(&self) -> &Path {
        &self.tree.root
    }

    /// 根だけを差し替える。ツリーをまだ歩いていない場面 (リポジトリを開き直した
    /// 直後など、走査は遅延させてある) 用。
    ///
    /// 根が空のまま相対パスを繋ぐとカレントディレクトリ相対になり、意図しない
    /// ファイルを黙って開くので、ツリーを空にする側は必ずここも呼ぶ。
    pub fn set_root(&mut self, root: PathBuf) {
        self.tree.root = root;
    }

    /// 裏で歩き終えたツリーを丸ごと差し替える。根が変わったかどうかを返す。
    ///
    /// 根・エントリ・git status は同じ 1 回の走査から出たものなので 3 つ揃って入れ替える。
    /// 別々に書けると「根は新しいのにエントリは古い」状態が作れ、その瞬間のクリックが
    /// 別ブランチの同名ファイルを静かに開く (worktree 切り替えは走査を裏に回すので実在する)。
    pub fn replace_tree(
        &mut self,
        root: PathBuf,
        entries: Vec<FileTreeEntry>,
        git_status: GitStatusMap,
    ) -> bool {
        let root_changed = self.tree.root != root;
        self.tree.root = root;
        self.tree.file_tree = entries;
        self.tree.git_status = git_status;
        self.invalidate_visible_cache();
        root_changed
    }

    /// worktree_path 以下を歩いてファイルツリーを構築する。`.git` はスキップ。各階層で
    /// ディレクトリがファイルより前、各グループ内はアルファベット順。
    ///
    /// 開いているファイル・スクロール位置・展開状態を保持するので、ウォッチャーによる
    /// 再構築が見ている画面を崩さない。
    ///
    /// 根を受け取る唯一の入口。以降ファイルを開く・子を読む・検索候補を集めるのはすべて
    /// この根に対して行う。`prev_file` は Viewer 側の状態なので引数で受け取り、Viewer 側で
    /// やるべきことは [TreeReload] で返す (実行するのは呼び出し側)。
    pub fn load_file_tree(&mut self, worktree_path: &Path, prev_file: Option<&str>) -> TreeReload {
        let root_changed = self.tree.root != worktree_path;
        self.tree.root = worktree_path.to_path_buf();

        let expanded_dirs: Vec<String> = self
            .tree
            .file_tree
            .iter()
            .filter(|e| e.is_dir && e.is_expanded)
            .map(|e| e.path.clone())
            .collect();
        // カーソル位置の復元と、再構築後のツリーが実際に変化したかの検出に使う。カーソルは
        // 可視インデックスで数えるので、ファイル添字へ直してから引く。
        let prev_selected_path = self
            .tree
            .visible_indices()
            .get(self.tree_cursor.selected())
            .and_then(|&i| self.tree.file_tree.get(i))
            .map(|e| e.path.clone());
        let prev_paths: Vec<String> = self.tree.file_tree.iter().map(|e| e.path.clone()).collect();

        // git status のスナップショットは再構築ごとに 1 回だけ取得する。失敗しても空のマップに
        // 落とすが、これは無害なフォールバックではない — エントリが無いとツリー上は全て
        // Tracked に見え、UI が「未ステージの変更がある」の正反対を黙って主張する。git 管理外の
        // ディレクトリでは正当にこの経路を通るので、実在するリポジトリでの一時的な失敗
        // (index.lock の競合など) と見分けるにはログしかない。
        self.tree.git_status = match GitStatusMap::load(worktree_path) {
            Ok(map) => map,
            Err(e) => {
                log::warn!(
                    "git status unavailable for {} — file tree and Changed files will render as if everything is tracked and committed: {e}",
                    worktree_path.display()
                );
                GitStatusMap::default()
            }
        };

        self.tree.file_tree.clear();
        self.invalidate_visible_cache();
        Self::walk_dir(
            worktree_path,
            worktree_path,
            0,
            &mut self.tree.file_tree,
            &self.tree.git_status,
        );

        // 遅延読み込みのディレクトリは子要素も読み込み、リフレッシュ前と同じ見た目にする。
        let mut idx = 0;
        while idx < self.tree.file_tree.len() {
            if self.tree.file_tree[idx].is_dir
                && expanded_dirs.contains(&self.tree.file_tree[idx].path)
            {
                self.tree.file_tree[idx].is_expanded = true;
                if !self.tree.file_tree[idx].children_loaded {
                    self.ensure_children_loaded(idx);
                }
            }
            idx += 1;
        }

        // 以前開いていたファイルがまだ存在するなら reopen 対象にする。読み直すのは呼び出し側の
        // 仕事で、ウォッチャーや 3 秒ポーリングが読者を追い出さないようにするのが要点。
        let reopen = prev_file.and_then(|rel_path| {
            if !worktree_path.join(rel_path).is_file() {
                return None;
            }
            Some(rel_path.to_string())
        });

        // 再構築をまたいでツリーのカーソルを固定する。開いているファイルがあればそこ、無ければ
        // 以前選んでいたパスへ戻す。ウォッチャーや定期リフレッシュで先頭に巻き戻されないため。
        let anchor = reopen.as_deref().or(prev_selected_path.as_deref());
        if let Some(path) = anchor {
            let visible = self.tree.visible_indices();
            if let Some(at) = visible
                .iter()
                .position(|&i| self.tree.file_tree.get(i).is_some_and(|e| e.path == path))
            {
                self.tree_cursor.place(at, visible.len());
            }
        }

        let entries_changed = self
            .tree
            .file_tree
            .iter()
            .map(|e| &e.path)
            .ne(prev_paths.iter());

        TreeReload {
            root_changed,
            reopen,
            entries_changed,
        }
    }

    pub fn invalidate_visible_cache(&mut self) {
        self.tree.invalidate_visible_cache();
    }

    pub fn visible_indices(&mut self) -> Rc<Vec<usize>> {
        self.tree.visible_indices()
    }

    pub fn ensure_children_loaded(&mut self, idx: usize) {
        self.tree.ensure_children_loaded(idx);
    }

    // ツリーの reveal

    /// 相対パスを指定して、explorer ツリー内でファイルを reveal し選択する。
    ///
    /// パスのセグメントを順に辿りながら、途中の各親ディレクトリを展開
    /// （必要なら遅延読み込みも）していき、最後に対象エントリへ tree_selected を
    /// 合わせ、それが見えるようスクロールを調整する。
    pub fn reveal_file_in_tree(&mut self, relative_path: &str) {
        let segments: Vec<&str> = relative_path.split('/').collect();
        if segments.is_empty() {
            return;
        }

        let mut parent_path = String::new();

        for (seg_idx, segment) in segments.iter().enumerate() {
            let is_last = seg_idx == segments.len() - 1;
            let target_path = if parent_path.is_empty() {
                segment.to_string()
            } else {
                format!("{parent_path}/{segment}")
            };

            // パスが一致するエントリを探す。
            let Some(idx) = self
                .tree
                .file_tree
                .iter()
                .position(|e| e.path == target_path)
            else {
                return; // エントリが見つからない — reveal できない。
            };

            if is_last {
                // 窓には触れない。画面に入れるのは高さを知る側 (入力の入口) の
                // 仕事で、そこが次のティックで clamp する。
                let visible = self.visible_indices();
                if let Some(at) = visible.iter().position(|&vi| vi == idx) {
                    self.tree_cursor.place(at, visible.len());
                }
            } else {
                // 途中のディレクトリ — 子要素が読み込まれていることを確認し展開する。
                self.ensure_children_loaded(idx);
                if let Some(entry) = self.tree.file_tree.get_mut(idx)
                    && !entry.is_expanded
                {
                    entry.is_expanded = true;
                    self.invalidate_visible_cache();
                }
            }

            parent_path = target_path;
        }
    }

    // 内部ヘルパー

    /// ファイルツリー走査の最大再帰深度。
    const MAX_DEPTH: usize = 8;

    /// ファイルツリー走査時にスキップするディレクトリ。ファイル数が非常に多くなりがちで、
    /// 対話的に閲覧する価値がほとんどないもの。
    const SKIP_DIRS: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        "vendor",
        ".next",
        "dist",
        "build",
        "__pycache__",
        ".cache",
        "coverage",
        ".venv",
        "venv",
        "bower_components",
        ".tox",
        ".mypy_cache",
        ".pytest_cache",
        ".turbo",
        ".nuxt",
        ".output",
    ];

    /// ツリーの根以下のファイルシステム全体を歩き、ファイル名検索キャッシュを構築する。
    ///
    /// ファイル名検索は Viewer 側の状態なので、書き込み先を引数で受け取る
    /// (Explorer が Viewer をフィールドとして持つことはしない)。
    pub fn populate_filename_search_cache(&mut self, filename_search: &mut FilenameSearchState) {
        filename_search.filename_search_all_files.clear();
        Self::collect_all_file_paths(
            &self.tree.root,
            &self.tree.root,
            0,
            &mut filename_search.filename_search_all_files,
        );
    }

    /// スキップするディレクトリは walk_dir / SKIP_DIRS と同じ。root からの相対パスで、
    /// ディレクトリは含めない。
    fn collect_all_file_paths(root: &Path, dir: &Path, depth: usize, paths: &mut Vec<String>) {
        if depth > Self::MAX_DEPTH {
            return;
        }
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_path = entry.path();
            let is_dir = child_path.is_dir();
            if is_dir && Self::SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let rel_path = child_path
                .strip_prefix(root)
                .unwrap_or(&child_path)
                .to_string_lossy()
                .to_string();
            if is_dir {
                Self::collect_all_file_paths(root, &child_path, depth + 1, paths);
            } else {
                paths.push(rel_path);
            }
        }
    }

    /// dir の直接の子要素を読み、entries に追加する。各エントリには git_status から
    /// 求めた [TreeGitState] を刻む。
    ///
    /// 再帰はしない: 子ディレクトリは children_loaded: false の折りたたみ状態で
    /// 積まれ、ユーザーが展開したときに ensure_children_loaded がここに戻ってきて
    /// 埋める。初回の走査と遅延展開の両方をこの1つの関数でまかなうのは意図的な設計。
    /// もともとは同一ロジックの別々のコピーだったが、git_status パラメータを
    /// 両方に通す必要が出たとき、次の乖離が発生するのは時間の問題だった。
    pub fn walk_dir(
        root: &Path,
        dir: &Path,
        depth: usize,
        entries: &mut Vec<FileTreeEntry>,
        git_status: &GitStatusMap,
    ) {
        if depth > Self::MAX_DEPTH {
            return;
        }

        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };

        // 収集してソートする: ディレクトリを先に、ファイルを後に、それぞれアルファベット順。
        let mut children: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();

        children.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        for child in &children {
            let name = child.file_name().to_string_lossy().to_string();

            let child_path = child.path();
            let is_dir = child_path.is_dir();

            // 既知の重いディレクトリはスキップする。
            if is_dir && Self::SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }

            let rel_path = child_path
                .strip_prefix(root)
                .unwrap_or(&child_path)
                .to_string_lossy()
                .to_string();

            let icon = if is_dir {
                dir_icon(false)
            } else {
                file_icon(&name)
            };
            let git_state = git_status.classify(&rel_path);
            entries.push(FileTreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                is_expanded: false,
                children_loaded: false,
                icon,
                git_state,
            });
        }
    }
}

#[cfg(test)]
#[path = "tree/tests.rs"]
mod tests;
