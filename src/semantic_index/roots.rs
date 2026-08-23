//! ツリーのどこを、どの道具で索引するか。
//!
//! 成果物の名前を索引ルートごとに分けるのもここの仕事で、分けないと 2 本目の生成が
//! 1 本目の索引を上書きする。ロックだけは分けない (リポジトリに 1 つ)。producer 1 本の
//! ピークが 2.3GiB なので、ルートごとに分けると同時に立つ本数の上限が黙って消える。

use sheaf_core::{IndexSource, Producer, RustAnalyzer, ScipGo, ScipTypescript, Target};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 索引を吐く道具の選択肢。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    Rust,
    Go,
    TypeScript,
}

/// 探す順。同じディレクトリが複数の言語の索引ルートになることはある。
const ALL: [Language; 3] = [Language::Rust, Language::Go, Language::TypeScript];

impl Language {
    /// そのディレクトリがこの言語の索引ルートであることを示すファイル。
    fn marker(self) -> &'static str {
        match self {
            Language::Rust => "Cargo.toml",
            Language::Go => "go.mod",
            Language::TypeScript => "tsconfig.json",
        }
    }

    /// 索引を作る道具。生成・出自の読み書き・投入の 3 箇所で同じものを指す必要がある。
    /// 道具が変われば同じソースから別の索引が出るので、sheaf は前の道具が書いた
    /// 出自の表を読まない。ここが食い違うと、読む側と書く側がずれて全部が構文層に落ちる。
    pub fn producer(self) -> Arc<dyn Producer> {
        match self {
            Language::Rust => Arc::new(RustAnalyzer),
            Language::Go => Arc::new(ScipGo),
            Language::TypeScript => Arc::new(ScipTypescript),
        }
    }

    fn tag(self) -> &'static str {
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

/// `tree_root` の索引ルートを列挙する。
pub fn discover(tree_root: &Path) -> Vec<IndexRoot> {
    if tree_root.as_os_str().is_empty() {
        return Vec::new();
    }
    ALL.iter()
        .filter(|lang| tree_root.join(lang.marker()).is_file())
        .map(|&lang| IndexRoot {
            subroot: PathBuf::new(),
            lang,
        })
        .collect()
}

impl IndexRoot {
    /// 成果物のファイル名の幹。
    ///
    /// 区切りを `_` に潰すだけだと `a/b` と `a_b` が同じ名前になり、2 本の索引が
    /// 同じファイルを取り合う。元の `_` を重ねて逃がすことで 1 対 1 に保つ。
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

    /// このルートを索引する対象と、成果物の置き場所。
    pub fn target(&self, dir: &Path, tree_root: &Path) -> Target {
        let stem = self.stem();
        Target {
            root: tree_root.join(&self.subroot),
            index: dir.join(format!("{stem}.scip")),
            hashes: dir.join(format!("{stem}.hashes")),
            log: dir.join(format!("{stem}.log")),
            lock: dir.join("generate.lock"),
        }
    }

    /// 置いてある索引を投入元にする。索引か出自の表のどちらかが無ければ `None`。
    ///
    /// 出自を言えない索引を読んでも、`Store` は結局すべてのファイルを「変更あり」と
    /// 扱って構文層に落とすだけなので、その場合は最初からロードしない。
    pub fn source(&self, dir: &Path) -> Option<IndexSource> {
        let stem = self.stem();
        let index = dir.join(format!("{stem}.scip"));
        if !index.is_file() {
            return None;
        }
        let expected = sheaf_core::read_provenance(
            &dir.join(format!("{stem}.hashes")),
            &*self.lang.producer(),
        )?;
        Some(IndexSource {
            index,
            subroot: self.subroot.clone(),
            expected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(subroot: &str, lang: Language) -> IndexRoot {
        IndexRoot {
            subroot: PathBuf::from(subroot),
            lang,
        }
    }

    #[test]
    fn 目印のある言語だけを索引ルートにする() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module demo\n").unwrap();

        assert_eq!(discover(dir.path()), vec![root("", Language::Go)]);
    }

    #[test]
    fn 目印が無いツリーには索引ルートが無い() {
        // ここが空でないと、Go だけのリポジトリで rust-analyzer が起動する。
        // 認識できない対象に対して終了コード 0 で空の索引を書くことがあるので、
        // 起こさないこと自体が答えになる。
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.go"), "package main\n").unwrap();

        assert!(discover(dir.path()).is_empty());
    }

    #[test]
    fn 索引ルートごとに成果物の名前が変わる() {
        let dir = Path::new("/artifacts");
        let tree = Path::new("/tree");
        let names = |r: &IndexRoot| r.target(dir, tree).index;

        assert_eq!(
            names(&root("", Language::Rust)),
            Path::new("/artifacts/index.rust.scip")
        );
        assert_eq!(
            names(&root("services/api", Language::Go)),
            Path::new("/artifacts/index.go.services_api.scip")
        );
    }

    #[test]
    fn 区切りを潰しても別のルートは別の名前になる() {
        // `a/b` と `a_b` が同じ名前に落ちると、2 本の索引が同じファイルを取り合う。
        let dir = Path::new("/artifacts");
        let tree = Path::new("/tree");
        assert_ne!(
            root("a/b", Language::Go).target(dir, tree).index,
            root("a_b", Language::Go).target(dir, tree).index
        );
    }

    #[test]
    fn ロックはルートをまたいで同じものを指す() {
        // 生成 1 本のピークが 2.3GiB なので、上限はリポジトリ単位で効かせる。
        let dir = Path::new("/artifacts");
        let tree = Path::new("/tree");
        assert_eq!(
            root("", Language::Rust).target(dir, tree).lock,
            root("services/api", Language::Go).target(dir, tree).lock
        );
    }
}
