//! ファイルツリーの構築とナビゲーション — ファイルシステムを歩いてフラットな
//! Vec<FileTreeEntry> を作る、ディレクトリの子要素の遅延読み込み、展開/折りたたみ、
//! ツリー中のパスの reveal。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::git_engine::status_map::GitStatusMap;

use super::file_tree::{FileTreeEntry, file_icon};
use super::state::ViewerState;

impl ViewerState {
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

    /// 裏で歩き終えたツリーを丸ごと差し替える。
    ///
    /// 根・エントリ・git status は同じ 1 回の走査から出たものなので 3 つ揃って
    /// 入れ替える。別々に書けるようにしておくと「根は新しいのにエントリは古い」
    /// 状態が作れてしまい、その瞬間のクリックは別ブランチの同名ファイルを静かに
    /// 開く (worktree 切り替えは走査を裏に回すので、この隙間は実在する)。
    pub fn replace_tree(
        &mut self,
        root: PathBuf,
        entries: Vec<FileTreeEntry>,
        git_status: GitStatusMap,
        tab_width: usize,
    ) {
        let root_changed = self.tree.root != root;
        self.tree.root = root;
        self.tree.file_tree = entries;
        self.tree.git_status = git_status;
        self.invalidate_visible_cache();
        // 相対パスの指す先が変わるので、新しい根に無いファイルのタブは閉じる。
        // 同じ根への再走査では触らない — 一時的に消えたファイルのタブまで
        // 勝手に閉じてしまう。
        if root_changed {
            self.prune_tabs_to_root(tab_width);
        }
    }

    /// worktree_path 以下のファイルシステムを歩いてファイルツリーを構築する。
    ///
    /// .git という名前のディレクトリはスキップする。ツリーは各階層でディレクトリが
    /// ファイルより前に来るようソートされ、各グループ内はアルファベット順になる。
    ///
    /// 現在開いているファイル・スクロール位置・ディレクトリの展開状態を保持するので、
    /// ファイルウォッチャーによる再構築がユーザーの見ている画面を崩さない。
    /// 以前開いていたファイルが削除されていた場合は、自然に「ファイル未選択」に戻る。
    ///
    /// 可視エントリの集合が変化した場合は true を返す。呼び出し側は、定期リフレッシュで
    /// 変化が無かった場合の再描画をスキップできる。
    ///
    /// 根を受け取る唯一の入口。ここで [ViewerState::root] を確定させ、以降
    /// ファイルを開く・子を読む・検索候補を集めるのはすべてこの根に対して行う。
    pub fn load_file_tree(&mut self, worktree_path: &Path, tab_width: usize) -> bool {
        let root_changed = self.tree.root != worktree_path;
        self.tree.root = worktree_path.to_path_buf();
        if root_changed {
            self.prune_tabs_to_root(tab_width);
        }

        // クリアする前に状態を退避しておく。
        let prev_file = self.content.current_file.clone();
        let expanded_dirs: Vec<String> = self
            .tree
            .file_tree
            .iter()
            .filter(|e| e.is_dir && e.is_expanded)
            .map(|e| e.path.clone())
            .collect();
        // カーソルが指すエントリと全パス集合を覚えておく。カーソル位置を復元し、
        // 再構築後のツリーが実際に変化したかを検出するために使う。
        let prev_selected_path = self
            .tree
            .file_tree
            .get(self.tree.tree_selected)
            .map(|e| e.path.clone());
        let prev_paths: Vec<String> = self.tree.file_tree.iter().map(|e| e.path.clone()).collect();

        // git status のスナップショットは再構築ごとに1回だけ取得する（エントリごとではない）。
        // 取得に失敗しても、減光表示の細部のためにツリー再構築全体を失敗させるのではなく
        // 空のマップにフォールバックする。ただしログには残す。空のマップは無害な
        // フォールバックではない — エントリが無いと、ツリー上は全て Tracked、
        // Changed files 上は全て Committed（緑）に見えてしまい、UI が
        // 「未ステージの変更がある」の正反対を黙って主張してしまう。
        // git 管理外のディレクトリを開いた場合はこの経路を正当に通る（発見すべき
        // リポジトリが無いのだから）。一方、実在するリポジトリ内での一時的な失敗
        // （並行して走る git コマンドが index.lock を握っている、など）は画面上
        // 見分けがつかないので、ログだけが両者を区別する手段になる。
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

        // ディスクからツリーを再構築する。
        self.tree.file_tree.clear();
        self.invalidate_visible_cache();
        Self::walk_dir(
            worktree_path,
            worktree_path,
            0,
            &mut self.tree.file_tree,
            &self.tree.git_status,
        );

        // ディレクトリの展開状態を復元する。遅延読み込みのディレクトリについては
        // 子要素も読み込み、リフレッシュ前と同じ見た目のツリーにする。
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

        // 以前開いていたファイルがまだ存在するなら、読んでいた位置と表示モード
        // （unified diff / SUMMARY / markdown）を保ったまま読み直す。ウォッチャー
        // や3秒ポーリングが読者を追い出さないようにするのが要点。
        // ファイルが削除されていた場合は、自然に「ファイル未選択」のまま留まる。
        if let Some(ref rel_path) = prev_file
            && worktree_path.join(rel_path).is_file()
        {
            self.reload_active_file(rel_path, tab_width);
            // tree_selected をファイルエントリに合わせて復元しようとする。
            if let Some(idx) = self.tree.file_tree.iter().position(|e| e.path == *rel_path) {
                self.tree.tree_selected = idx;
            }
        }

        // 再構築をまたいでツリーのカーソルを固定する。ファイルが開いている場合は
        // 上のブロックで既にカーソルをそこに合わせている。そうでなければ以前選択していた
        // エントリを復元し、ウォッチャーや定期リフレッシュでカーソルが先頭に
        // 巻き戻されないようにする。
        let anchored_to_open_file = prev_file.as_ref().is_some_and(|f| {
            self.tree
                .file_tree
                .get(self.tree.tree_selected)
                .map(|e| &e.path)
                == Some(f)
        });
        if !anchored_to_open_file
            && let Some(path) = prev_selected_path
            && let Some(idx) = self.tree.file_tree.iter().position(|e| e.path == path)
        {
            self.tree.tree_selected = idx;
        }
        if self.tree.tree_selected >= self.tree.file_tree.len() {
            self.tree.tree_selected = self.tree.file_tree.len().saturating_sub(1);
        }

        self.tree
            .file_tree
            .iter()
            .map(|e| &e.path)
            .ne(prev_paths.iter())
    }

    /// file_tree のインデックス idx にあるディレクトリの展開/折りたたみを切り替える。
    pub fn toggle_dir(&mut self, idx: usize) {
        if let Some(entry) = self.tree.file_tree.get_mut(idx)
            && entry.is_dir
        {
            entry.is_expanded = !entry.is_expanded;
            self.invalidate_visible_cache();
        }
    }

    /// インデックス idx のディレクトリを展開する（既に展開済み、もしくはファイル
    /// エントリなら何もしない）。
    pub fn expand_dir(&mut self, idx: usize) {
        if let Some(entry) = self.tree.file_tree.get_mut(idx)
            && entry.is_dir
            && !entry.is_expanded
        {
            entry.is_expanded = true;
            self.invalidate_visible_cache();
        }
    }

    /// インデックス idx のディレクトリを折りたたむ（既に折りたたみ済み、もしくは
    /// ファイルエントリなら何もしない）。
    pub fn collapse_dir(&mut self, idx: usize) {
        if let Some(entry) = self.tree.file_tree.get_mut(idx)
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
        self.tree.cached_visible_indices = None;
    }

    /// 折りたたまれたディレクトリを考慮した上で、現在可視な file_tree の
    /// インデックス一覧を返す。結果は Rc としてキャッシュされ、
    /// invalidate_visible_cache() が呼ばれるまで保持されるので、同一フレーム内での
    /// 繰り返し呼び出しは実質コストゼロになる。
    pub fn visible_indices(&mut self) -> Rc<Vec<usize>> {
        if let Some(ref cached) = self.tree.cached_visible_indices {
            return Rc::clone(cached);
        }

        let mut result = Vec::with_capacity(self.tree.file_tree.len());
        let mut skip_depth: Option<usize> = None;

        for (i, entry) in self.tree.file_tree.iter().enumerate() {
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
        self.tree.cached_visible_indices = Some(Rc::clone(&rc));
        rc
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
                // 対象のファイル/ディレクトリを選択する。
                self.tree.tree_selected = idx;
                // その項目が見えるようスクロールを調整する。
                let visible = self.visible_indices();
                if let Some(vis_pos) = visible.iter().position(|&vi| vi == idx) {
                    let height = self.explorer.explorer_tree_height;
                    if vis_pos < self.tree.tree_scroll || vis_pos >= self.tree.tree_scroll + height
                    {
                        self.tree.tree_scroll = vis_pos.saturating_sub(height / 3);
                    }
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

    /// file_tree のインデックス idx にあるディレクトリの直接の子要素を遅延読み込みする。
    /// エントリがディレクトリでない、または既に子要素が読み込み済みの場合は何もしない。
    pub fn ensure_children_loaded(&mut self, idx: usize) {
        let (rel_path, child_depth) = match self.tree.file_tree.get(idx) {
            Some(e) if e.is_dir && !e.children_loaded => (e.path.clone(), e.depth + 1),
            _ => return,
        };

        let full_path = self.tree.root.join(&rel_path);

        let mut children: Vec<FileTreeEntry> = Vec::new();
        Self::walk_dir(
            &self.tree.root,
            &full_path,
            child_depth,
            &mut children,
            &self.tree.git_status,
        );

        self.tree.file_tree[idx].children_loaded = true;

        if children.is_empty() {
            return;
        }

        let insert_pos = idx + 1;
        let count = children.len();

        // 挿入位置以降にある場合は tree_selected を調整する。
        if self.tree.tree_selected >= insert_pos {
            self.tree.tree_selected += count;
        }

        self.tree.file_tree.splice(insert_pos..insert_pos, children);
        self.invalidate_visible_cache();
    }

    /// ツリーの根以下のファイルシステム全体を歩き、ファイル名検索キャッシュを構築する。
    pub fn populate_filename_search_cache(&mut self) {
        self.filename_search.filename_search_all_files.clear();
        Self::collect_all_file_paths(
            &self.tree.root,
            &self.tree.root,
            0,
            &mut self.filename_search.filename_search_all_files,
        );
    }

    /// dir 以下の全てのファイルパスを再帰的に収集する。スキップするディレクトリは
    /// walk_dir / SKIP_DIRS と同じ。ファイルパスのみ（ディレクトリは含まない）を
    /// paths に追加し、root からの相対パスとして格納する。
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
                "\u{1f4c1}"
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
