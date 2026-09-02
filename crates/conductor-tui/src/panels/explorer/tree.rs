//! ファイルツリーの走査と、その中の位置。
//!
//! 走査は svc のワーカーで行い ([Snapshot])、展開したディレクトリの子だけを
//! UI スレッドで読む。根・エントリ・git status は同じ 1 回の走査から出たものなので
//! 3 つ揃って入れ替える。別々に書けると「根は新しいのにエントリは古い」状態が作れ、
//! その瞬間のクリックが別ブランチの同名ファイルを開く。

use std::path::{Path, PathBuf};

use conductor_core::git_engine::status_map::{GitStatusMap, TreeGitState};
use conductor_core::icons::{FileIcon, dir_icon, file_icon};

/// 走査の最大深度。
const MAX_DEPTH: usize = 8;

/// ファイル数が多くなりがちで、対話的に開く価値がほとんど無いディレクトリ。
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

/// 平坦化したツリーの 1 行。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 根からの相対パス。
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// ディレクトリの子をツリーに読み込み済みか。ファイルでは常に false。
    pub children_loaded: bool,
    pub icon: FileIcon,
    pub git: TreeGitState,
}

/// ワーカーが 1 回の走査で作るもの。
#[derive(Debug)]
pub struct Snapshot {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub status: GitStatusMap,
    /// ファイル名検索が引く、根以下の全ファイル。
    pub all_files: Vec<String>,
}

/// root を歩いて Snapshot を作る。`expanded` にあるディレクトリは子まで読む。
pub fn survey(root: &Path, expanded: &[String]) -> Snapshot {
    // 失敗しても空のマップに落とすが、これは無害なフォールバックではない。エントリが
    // 無いとツリー上は全て Tracked に見え、UI が「未ステージの変更がある」の正反対を
    // 黙って主張する。git 管理外のディレクトリでは正当にこの経路を通る。
    let status = GitStatusMap::load(root).unwrap_or_else(|e| {
        log::warn!(
            "git status unavailable for {} — the file tree will render as if everything is tracked: {e}",
            root.display()
        );
        GitStatusMap::default()
    });

    let mut entries = Vec::new();
    read_children(root, root, 0, &mut entries, &status);

    let mut idx = 0;
    while idx < entries.len() {
        if entries[idx].is_dir && expanded.contains(&entries[idx].path) {
            entries[idx].expanded = true;
            load_children(root, &mut entries, idx, &status);
        }
        idx += 1;
    }

    let mut all_files = Vec::new();
    collect_files(root, root, 0, &mut all_files);

    Snapshot {
        root: root.to_path_buf(),
        entries,
        status,
        all_files,
    }
}

/// 表示中のツリー。
#[derive(Debug, Default)]
pub struct FileTree {
    root: PathBuf,
    entries: Vec<Entry>,
    status: GitStatusMap,
    all_files: Vec<String>,
}

impl FileTree {
    /// エントリの相対パスをここに繋いで絶対パスにする。
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn status(&self) -> &GitStatusMap {
        &self.status
    }

    pub fn all_files(&self) -> &[String] {
        &self.all_files
    }

    /// 走査がまだ届いていない間も相対パスを絶対に戻せるよう、根だけ先に置く。
    /// 根が空のまま相対パスを繋ぐとカレントディレクトリ相対になり、意図しない
    /// ファイルを黙って開く。
    pub fn set_root(&mut self, root: PathBuf) {
        if self.root != root {
            self.entries.clear();
            self.all_files.clear();
            self.status = GitStatusMap::default();
        }
        self.root = root;
    }

    pub fn install(&mut self, snapshot: Snapshot) {
        self.root = snapshot.root;
        self.entries = snapshot.entries;
        self.status = snapshot.status;
        self.all_files = snapshot.all_files;
    }

    /// いま展開しているディレクトリ。走査をやり直しても同じ形に戻すために渡す。
    pub fn expanded_dirs(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.is_dir && e.expanded)
            .map(|e| e.path.clone())
            .collect()
    }

    /// 画面に出るエントリの、[Self::entries] への添字。
    pub fn visible(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.entries.len());
        let mut skip_below: Option<usize> = None;
        for (i, entry) in self.entries.iter().enumerate() {
            if let Some(depth) = skip_below {
                if entry.depth > depth {
                    continue;
                }
                skip_below = None;
            }
            out.push(i);
            if entry.is_dir && !entry.expanded {
                skip_below = Some(entry.depth);
            }
        }
        out
    }

    pub fn get(&self, idx: usize) -> Option<&Entry> {
        self.entries.get(idx)
    }

    pub fn toggle(&mut self, idx: usize) {
        match self.entries.get(idx) {
            Some(e) if e.is_dir && e.expanded => self.collapse(idx),
            Some(e) if e.is_dir => self.expand(idx),
            _ => {}
        }
    }

    pub fn expand(&mut self, idx: usize) {
        let Some(entry) = self.entries.get_mut(idx) else {
            return;
        };
        if !entry.is_dir || entry.expanded {
            return;
        }
        entry.expanded = true;
        load_children(&self.root, &mut self.entries, idx, &self.status);
    }

    pub fn collapse(&mut self, idx: usize) {
        if let Some(entry) = self.entries.get_mut(idx)
            && entry.is_dir
        {
            entry.expanded = false;
        }
    }

    /// 相対パスを指すエントリまで途中のディレクトリを開き、その可視添字を返す。
    pub fn reveal(&mut self, relative_path: &str) -> Option<usize> {
        let mut prefix = String::new();
        let segments: Vec<&str> = relative_path.split('/').collect();
        for (i, segment) in segments.iter().enumerate() {
            let path = if prefix.is_empty() {
                (*segment).to_string()
            } else {
                format!("{prefix}/{segment}")
            };
            let idx = self.entries.iter().position(|e| e.path == path)?;
            if i + 1 == segments.len() {
                return self.visible().iter().position(|&v| v == idx);
            }
            self.expand(idx);
            prefix = path;
        }
        None
    }
}

/// idx のディレクトリの直接の子を読み、その直後へ挿す。読み込み済みなら何もしない。
fn load_children(root: &Path, entries: &mut Vec<Entry>, idx: usize, status: &GitStatusMap) {
    let (path, depth) = match entries.get(idx) {
        Some(e) if e.is_dir && !e.children_loaded => (e.path.clone(), e.depth + 1),
        _ => return,
    };
    let mut children = Vec::new();
    read_children(root, &root.join(&path), depth, &mut children, status);
    entries[idx].children_loaded = true;
    if !children.is_empty() {
        entries.splice(idx + 1..idx + 1, children);
    }
}

/// dir の直接の子だけを読む。子ディレクトリは畳んだまま積み、展開されたときに
/// [load_children] がここへ戻ってきて埋める。
fn read_children(
    root: &Path,
    dir: &Path,
    depth: usize,
    entries: &mut Vec<Entry>,
    status: &GitStatusMap,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
    children.sort_by(|a, b| {
        let dir_of = |e: &std::fs::DirEntry| e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        dir_of(b)
            .cmp(&dir_of(a))
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    for child in &children {
        let name = child.file_name().to_string_lossy().to_string();
        let full = child.path();
        let is_dir = full.is_dir();
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = relative(root, &full);
        let git = status.classify(&path);
        entries.push(Entry {
            icon: if is_dir {
                dir_icon(false)
            } else {
                file_icon(&name)
            },
            path,
            name,
            depth,
            is_dir,
            expanded: false,
            children_loaded: false,
            git,
        });
    }
}

fn collect_files(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let full = entry.path();
        if full.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect_files(root, &full, depth + 1, out);
            }
        } else {
            out.push(relative(root, &full));
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// gitignore された ignored/、SKIP_DIRS の node_modules、どちらでもない src を持つ木。
    fn fixture() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        for sub in ["src", "src/deep", "node_modules", "ignored"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("src/deep/mod.rs"), "\n").unwrap();
        std::fs::write(root.join("node_modules/big.js"), "\n").unwrap();
        std::fs::write(root.join("ignored/out.txt"), "\n").unwrap();
        dir
    }

    fn paths(tree: &FileTree) -> Vec<&str> {
        tree.visible()
            .iter()
            .map(|&i| tree.entries()[i].path.as_str())
            .collect()
    }

    #[test]
    fn 走査はgitignoreを含み重いディレクトリだけを飛ばす() {
        let dir = fixture();
        let mut tree = FileTree::default();
        tree.install(survey(dir.path(), &[]));

        let visible = paths(&tree);
        assert!(visible.contains(&"ignored"), "{visible:?}");
        assert!(visible.contains(&"src"));
        assert!(!visible.contains(&"node_modules"), "{visible:?}");
        assert!(
            !tree
                .all_files()
                .iter()
                .any(|p| p.starts_with("node_modules"))
        );
        // 直下だけを読むので、畳んだディレクトリの中身はまだ出ない。
        assert!(!visible.contains(&"src/main.rs"));
    }

    #[test]
    fn ディレクトリは展開すると子が現れ畳むと隠れる() {
        let dir = fixture();
        let mut tree = FileTree::default();
        tree.install(survey(dir.path(), &[]));
        let src = tree.entries().iter().position(|e| e.path == "src").unwrap();

        tree.expand(src);
        assert!(paths(&tree).contains(&"src/main.rs"));
        tree.collapse(src);
        assert!(!paths(&tree).contains(&"src/main.rs"));
        // 一度読んだ子は残るので、開き直しても読み直さない。
        tree.expand(src);
        assert!(paths(&tree).contains(&"src/main.rs"));
    }

    /// ウォッチャーの再読み込みが見ている画面を崩さない。
    #[test]
    fn 展開したディレクトリは走査し直しても開いたまま() {
        let dir = fixture();
        let mut tree = FileTree::default();
        tree.install(survey(dir.path(), &[]));
        let src = tree.entries().iter().position(|e| e.path == "src").unwrap();
        tree.expand(src);

        let expanded = tree.expanded_dirs();
        assert_eq!(expanded, ["src"]);
        tree.install(survey(dir.path(), &expanded));
        assert!(paths(&tree).contains(&"src/main.rs"));
    }

    #[test]
    fn 根とエントリは一緒に入れ替わる() {
        let a = fixture();
        let b = TempDir::new().unwrap();
        std::fs::write(b.path().join("other.txt"), "\n").unwrap();

        let mut tree = FileTree::default();
        tree.install(survey(a.path(), &[]));
        tree.install(survey(b.path(), &[]));
        assert_eq!(tree.root(), b.path());
        assert_eq!(paths(&tree), ["other.txt"]);
    }

    #[test]
    fn 根だけ差し替えるとエントリは捨てる() {
        let a = fixture();
        let b = TempDir::new().unwrap();
        let mut tree = FileTree::default();
        tree.install(survey(a.path(), &[]));
        tree.set_root(b.path().to_path_buf());
        assert!(tree.entries().is_empty());
        assert_eq!(tree.root(), b.path());
    }

    #[test]
    fn revealは途中のディレクトリを開いて可視添字を返す() {
        let dir = fixture();
        let mut tree = FileTree::default();
        tree.install(survey(dir.path(), &[]));

        let at = tree.reveal("src/deep/mod.rs").expect("見つかる");
        assert_eq!(paths(&tree)[at], "src/deep/mod.rs");
        assert_eq!(tree.reveal("src/nope.rs"), None);
    }
}
