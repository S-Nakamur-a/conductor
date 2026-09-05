//! ツリーのどこを、どの道具で索引するか。
//!
//! 成果物の名前は索引ルートごとに分ける。分けないと 2 本目の生成が 1 本目を上書きする。
//! ロックはリポジトリに 1 つ。producer 1 本のピークが 2.3GiB なので、ルートごとに
//! 分けると同時に立つ本数の上限が黙って消える。

pub use crate::symbol_index::Language;
use sheaf_core::{IndexSource, Producer, RustAnalyzer, ScipGo, ScipTypescript, Target};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ALL: [Language; 3] = [Language::Rust, Language::Go, Language::TypeScript];

impl Language {
    fn marker(self) -> &'static str {
        match self {
            Language::Rust => "Cargo.toml",
            Language::Go => "go.mod",
            Language::TypeScript => "tsconfig.json",
        }
    }

    /// 索引を作る道具。生成・出自の読み書き・投入の 3 箇所で同じものを指す必要がある。
    /// sheaf は別の道具が書いた出自の表を読まないので、ずれると全部が構文層に落ちる。
    pub fn producer(self) -> Arc<dyn Producer> {
        match self {
            Language::Rust => Arc::new(RustAnalyzer),
            Language::Go => Arc::new(ScipGo),
            Language::TypeScript => Arc::new(ScipTypescript),
        }
    }

    /// Rust の入れ子は workspace member なのでルートの rust-analyzer がまとめて見る。
    /// go.mod と tsconfig.json はモジュールの境界で、外側の索引に内側は入らない。
    fn nests(self) -> bool {
        match self {
            Language::Rust => false,
            Language::Go | Language::TypeScript => true,
        }
    }

    fn for_marker(name: &str) -> Option<Language> {
        ALL.into_iter().find(|lang| lang.marker() == name)
    }

    /// このファイルを索引に載せられる言語。目印そのものも数える。依存を足しただけの
    /// 編集で索引が据え置かれないため。
    pub fn of_file(path: &Path) -> Option<Language> {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(Language::for_marker)
            .or_else(|| Language::of_path(path))
    }

    /// 成果物の名前と記録に入る綴り。
    pub fn tag(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::TypeScript => "ts",
        }
    }
}

/// ツリーの中の索引ルート 1 本。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IndexRoot {
    /// ツリーのルートから見た相対パス。ツリーのルート自身なら空。
    pub subroot: PathBuf,
    pub lang: Language,
}

/// 依存を抱え込むディレクトリ。歩くと目印が数百単位で見つかる。
const VENDORED: [&str; 2] = ["node_modules", "vendor"];

fn is_vendored(name: &std::ffi::OsStr) -> bool {
    VENDORED.iter().any(|skip| name == *skip)
}

/// gitignore を尊重して `root` の下のファイルを歩く。ビルド成果物の下の目印を
/// 索引ルートにしないため。
fn walk_files(root: &Path) -> impl Iterator<Item = ignore::DirEntry> {
    ignore::WalkBuilder::new(root)
        .require_git(false)
        .filter_entry(|entry| !is_vendored(entry.file_name()))
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
}

/// `tree_root` の索引ルートを列挙する。ツリーのルートからの相対パスの浅い順。
pub fn discover(tree_root: &Path) -> Vec<IndexRoot> {
    if tree_root.as_os_str().is_empty() {
        return Vec::new();
    }
    let mut found: Vec<IndexRoot> = walk_files(tree_root)
        .filter_map(|entry| {
            let lang = entry.file_name().to_str().and_then(Language::for_marker)?;
            let subroot = entry.path().parent()?.strip_prefix(tree_root).ok()?;
            Some(IndexRoot {
                subroot: subroot.to_path_buf(),
                lang,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        a.subroot
            .cmp(&b.subroot)
            .then(a.lang.tag().cmp(b.lang.tag()))
    });
    let all = found.clone();
    found.retain(|at| at.lang.nests() || !covered_by_ancestor(at, &all));
    found
}

fn covered_by_ancestor(at: &IndexRoot, all: &[IndexRoot]) -> bool {
    all.iter().any(|other| {
        other.lang == at.lang
            && other.subroot != at.subroot
            && at.subroot.starts_with(&other.subroot)
    })
}

/// `rel` を索引に載せるルートの位置。同じ言語のルートが入れ子なら深いほうが持つ
/// ([`sheaf_core::Store`] の衝突の解き方に合わせる)。
///
/// 毎フレーム通る経路なので、束をそのまま借りられるよう反復子で受ける。
pub fn owning_index<'a>(all: impl Iterator<Item = &'a IndexRoot>, rel: &Path) -> Option<usize> {
    let lang = Language::of_file(rel)?;
    all.enumerate()
        .filter(|(_, at)| at.lang == lang && rel.starts_with(&at.subroot))
        .max_by_key(|(_, at)| at.subroot.components().count())
        .map(|(i, _)| i)
}

/// `at` の中にある、同じ言語のより深い索引ルート (`at` から見た相対パス)。
fn deeper_than(all: &[IndexRoot], at: &IndexRoot) -> Vec<PathBuf> {
    all.iter()
        .filter(|other| other.lang == at.lang && other.subroot != at.subroot)
        .filter_map(|other| other.subroot.strip_prefix(&at.subroot).ok())
        .map(Path::to_path_buf)
        .collect()
}

/// 内容の鍵の 16 進の桁数。48 ビット。1 本のルートが同時に持つ世代は数個なので
/// 衝突は起こらず、これ以上長いと置き場所を `ls` したときに読めない。
const KEY_LEN: usize = 12;

/// 索引ルート 1 本が置き場所に残せる世代の数。worktree を行き来したときに作り直さない
/// ためのもので、1 本 14.5MB。持つ worktree の数を超えて残しても一致しない。
const GENERATIONS: usize = 4;

impl IndexRoot {
    /// このルートの producer が読むファイルの内容から決まる鍵。成果物の名前に入れることで、
    /// 内容の違うツリーの索引を並べて持てる。
    ///
    /// ツリーを歩くので UI スレッドでは呼ばない (109 ルートのモノレポで最も重いルートが 110ms)。
    pub fn content_key(&self, tree_root: &Path, all: &[IndexRoot]) -> String {
        let at = tree_root.join(&self.subroot);
        let table = walk_files(&at).filter_map(|entry| {
            let rel = entry.path().strip_prefix(&at).ok()?.to_path_buf();
            let content = std::fs::read(entry.path()).ok()?;
            Some((rel, sheaf_core::blob_hash(&content)))
        });
        self.fold(table, &deeper_than(all, self))
    }

    /// (相対パス, 内容ハッシュ) の表を鍵に畳む。
    ///
    /// 畳むのはこの言語のファイルだけで、`deeper` (この中の同じ言語のより深いルート) は
    /// 除く。全部を畳むと画像の差し替えや内側の編集で鍵が動き、同じ索引を作り直す。
    /// 入口が 2 つある (ツリーを歩いたものと producer が書いた出自の表) ので、
    /// 絞り込みはここ 1 箇所に置く。ずれると生成した名前と次に探す名前が食い違う。
    pub(super) fn fold(
        &self,
        table: impl Iterator<Item = (PathBuf, String)>,
        deeper: &[PathBuf],
    ) -> String {
        let mut mine: Vec<(PathBuf, String)> = table
            .filter(|(rel, _)| Language::of_file(rel) == Some(self.lang))
            .filter(|(rel, _)| !deeper.iter().any(|inner| rel.starts_with(inner)))
            .filter(|(rel, _)| !rel.components().any(|c| is_vendored(c.as_os_str())))
            .collect();
        mine.sort();

        let mut folded = Vec::new();
        for (rel, hash) in mine {
            folded.extend_from_slice(rel.to_string_lossy().as_bytes());
            folded.push(0);
            folded.extend_from_slice(hash.as_bytes());
            folded.push(b'\n');
        }
        let mut key = sheaf_core::blob_hash(&folded);
        key.truncate(KEY_LEN);
        key
    }

    /// 置き場所に残っている、このルートの世代を新しい順に。
    fn generations(&self, dir: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
        let prefix = format!("{}.", self.stem());
        let mut found: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                let key = name.strip_prefix(&prefix)?.strip_suffix(".scip")?;
                // 幹は入れ子になりうる (index.go と index.go.api)。鍵の形で切り分けないと
                // 深いルートの世代を浅いルートが数える。
                if key.len() != KEY_LEN || !key.chars().all(|c| c.is_ascii_hexdigit()) {
                    return None;
                }
                let at = entry.metadata().ok()?.modified().ok()?;
                Some((at, entry.path()))
            })
            .collect();
        found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        found
    }

    pub fn has_any_generation(&self, dir: &Path) -> bool {
        !self.generations(dir).is_empty()
    }

    /// 新しいほうから [`GENERATIONS`] 本だけ残す。名前に鍵が入る前に置かれた索引も落とす。
    /// 消せなくても何もしない。残るのはディスクの無駄だけで、答えは変わらない。
    pub fn prune(&self, dir: &Path) {
        let stale = self
            .generations(dir)
            .into_iter()
            .skip(GENERATIONS)
            .map(|(_, index)| index)
            .chain(std::iter::once(dir.join(format!("{}.scip", self.stem()))));
        for index in stale {
            let _ = std::fs::remove_file(&index);
            let _ = std::fs::remove_file(index.with_extension("hashes"));
            let _ = std::fs::remove_file(index.with_extension("log"));
        }
    }

    /// 区切りを `_` に潰すだけだと `a/b` と `a_b` が同名になる。元の `_` を重ねて逃がす。
    fn stem(&self) -> String {
        let mut stem = format!("index.{}", self.lang.tag());
        let subroot = self.subroot.to_string_lossy();
        if !subroot.is_empty() {
            stem.push('.');
            for ch in subroot.chars() {
                match ch {
                    '_' => stem.push_str("__"),
                    '/' | '\\' => stem.push('_'),
                    _ => stem.push(ch),
                }
            }
        }
        stem
    }

    fn stem_for(&self, key: &str) -> String {
        format!("{}.{key}", self.stem())
    }

    /// このルートを索引する対象と、`key` の内容に対する成果物の置き場所。
    pub fn target(&self, dir: &Path, tree_root: &Path, key: &str) -> Target {
        let stem = self.stem_for(key);
        Target {
            root: tree_root.join(&self.subroot),
            index: dir.join(format!("{stem}.scip")),
            hashes: dir.join(format!("{stem}.hashes")),
            log: dir.join(format!("{stem}.log")),
            lock: dir.join("generate.lock"),
        }
    }

    /// `key` の内容の索引があるか。作り直しの判定に使う ([`source`](Self::source) は
    /// 一致しなければ最新の世代に落ちるので、そちらで判断すると永久に作り直さない)。
    pub fn has_generation(&self, dir: &Path, key: &str) -> bool {
        dir.join(format!("{}.scip", self.stem_for(key))).is_file()
    }

    /// 置いてある索引を投入元にする。索引か出自の表のどちらかが無ければ `None`。
    ///
    /// 鍵が一致する世代が無ければ最新の世代に落ちる。落とさないと 1 ファイル編集した
    /// 瞬間に索引全体が見えなくなる。説明できるかは出自の表がファイル単位で決めるので、
    /// 内容の違う世代を読んでも誤答にはならない。
    pub fn source(&self, dir: &Path, key: &str) -> Option<IndexSource> {
        let exact = dir.join(format!("{}.scip", self.stem_for(key)));
        let index = if exact.is_file() {
            exact
        } else {
            self.generations(dir).into_iter().next()?.1
        };
        let expected = self.provenance_at(&index.with_extension("hashes"))?;
        Some(IndexSource {
            index,
            subroot: self.subroot.clone(),
            expected,
        })
    }

    /// `key` の世代の出自の表。索引ルート相対のパス -> 内容ハッシュ。
    pub fn provenance(&self, dir: &Path, key: &str) -> Option<HashMap<PathBuf, String>> {
        self.provenance_at(&dir.join(format!("{}.hashes", self.stem_for(key))))
    }

    /// 最新の世代の出自の表。作り直しの前後でソースがどれだけ動いたかを比べる基準。
    pub fn newest_provenance(&self, dir: &Path) -> Option<HashMap<PathBuf, String>> {
        let (_, index) = self.generations(dir).into_iter().next()?;
        self.provenance_at(&index.with_extension("hashes"))
    }

    fn provenance_at(&self, path: &Path) -> Option<HashMap<PathBuf, String>> {
        sheaf_core::read_provenance(path, &*self.lang.producer())
    }
}
