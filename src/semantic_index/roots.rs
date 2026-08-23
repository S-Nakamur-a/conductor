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

    /// 入れ子になった目印を別の索引ルートとして扱うか。
    ///
    /// Rust の入れ子は workspace の member で、ルートに向けた rust-analyzer が
    /// まとめて見る。別に立てると同じソースを 2 度索引したうえ、ピーク 2.3GiB の
    /// producer が本数だけ増える。go.mod と tsconfig.json は逆で、そこがモジュール /
    /// プロジェクトの境界になるため、外側の索引には入らない。
    fn nests(self) -> bool {
        match self {
            Language::Rust => false,
            Language::Go | Language::TypeScript => true,
        }
    }

    fn for_marker(name: &str) -> Option<Language> {
        ALL.into_iter().find(|lang| lang.marker() == name)
    }

    /// このファイルを索引に載せられる言語。載せられないなら `None`。
    ///
    /// 目印そのものも数える。依存を足しただけで .go を触っていないときに、
    /// 索引が古いまま据え置かれるのを避けるため。
    pub fn of_file(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?;
        if let Some(lang) = Language::for_marker(name) {
            return Some(lang);
        }
        match path.extension()?.to_str()? {
            "rs" => Some(Language::Rust),
            "go" => Some(Language::Go),
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => {
                Some(Language::TypeScript)
            }
            _ => None,
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

/// 依存を抱え込むディレクトリ。ここを歩くと、目印が数百単位で見つかる。
/// 大半は gitignore されているが、コミットしているリポジトリは実在する。
const VENDORED: [&str; 2] = ["node_modules", "vendor"];

/// `tree_root` の索引ルートを列挙する。ツリーのルートからの相対パスの浅い順。
///
/// gitignore を尊重するのは、ビルド成果物の下の目印を索引ルートにしないため。
pub fn discover(tree_root: &Path) -> Vec<IndexRoot> {
    if tree_root.as_os_str().is_empty() {
        return Vec::new();
    }
    let mut found: Vec<IndexRoot> = ignore::WalkBuilder::new(tree_root)
        .require_git(false)
        .filter_entry(|entry| !VENDORED.iter().any(|skip| entry.file_name() == *skip))
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
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

/// 同じ言語の索引ルートが、より外側に既にあるか。
fn covered_by_ancestor(at: &IndexRoot, all: &[IndexRoot]) -> bool {
    all.iter().any(|other| {
        other.lang == at.lang
            && other.subroot != at.subroot
            && at.subroot.starts_with(&other.subroot)
    })
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

    /// `files` のパスをすべて空ファイルとして作る。
    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        dir
    }

    #[test]
    fn 入れ子の_go_mod_は別の索引ルートになる() {
        // go.mod はモジュールの境界で、外側の索引には入らない。1 本にまとめると
        // 内側のパッケージが丸ごと索引から落ちる。
        let dir = tree(&[
            ("go.mod", "module demo\n"),
            ("services/api/go.mod", "module demo/api\n"),
        ]);

        assert_eq!(
            discover(dir.path()),
            vec![root("", Language::Go), root("services/api", Language::Go)]
        );
    }

    #[test]
    fn 入れ子の_cargo_toml_は索引ルートにしない() {
        // workspace の member はルートに向けた rust-analyzer がまとめて見る。
        // 別に立てると同じソースを 2 度索引し、2.3GiB の producer が本数だけ増える。
        let dir = tree(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]\n"),
            ("crates/a/Cargo.toml", "[package]\nname = \"a\"\n"),
        ]);

        assert_eq!(discover(dir.path()), vec![root("", Language::Rust)]);
    }

    #[test]
    fn 依存を抱えたディレクトリの下は歩かない() {
        // node_modules の下には tsconfig.json が数百ある。索引ルートにすると
        // その数だけ producer が立つ。
        let dir = tree(&[
            ("tsconfig.json", "{}\n"),
            ("node_modules/pkg/tsconfig.json", "{}\n"),
        ]);

        assert_eq!(discover(dir.path()), vec![root("", Language::TypeScript)]);
    }

    #[test]
    fn gitignore_された目印は索引ルートにしない() {
        // ビルド成果物の下の目印を拾うと、生成物を索引することになる。
        let dir = tree(&[
            (".gitignore", "build/\n"),
            ("go.mod", "module demo\n"),
            ("build/gen/go.mod", "module demo/gen\n"),
        ]);

        assert_eq!(discover(dir.path()), vec![root("", Language::Go)]);
    }

    #[test]
    fn 言語ごとに違う道具を起動する() {
        // ここが 1 つに潰れると、Go や TypeScript のツリーに rust-analyzer が向く。
        // 認識できない対象には終了コード 0 で空の索引を書くので、失敗に見えない。
        let program = |lang: Language| lang.producer().command(Path::new("/o"))[0].clone();

        assert_eq!(program(Language::Rust), "rust-analyzer");
        assert_eq!(program(Language::Go), "scip-go");
        // scip-typescript は npx 越しに版を固定して起動する。
        assert_eq!(program(Language::TypeScript), "npx");
        assert!(
            Language::TypeScript
                .producer()
                .command(Path::new("/o"))
                .iter()
                .any(|a| a.starts_with("@sourcegraph/scip-typescript@"))
        );
    }

    #[test]
    fn 拡張子と目印から言語を引く() {
        assert_eq!(Language::of_file(Path::new("a/b.go")), Some(Language::Go));
        assert_eq!(
            Language::of_file(Path::new("a/tsconfig.json")),
            Some(Language::TypeScript)
        );
        // 索引に載らないファイルの変更で producer を起こさない。
        assert_eq!(Language::of_file(Path::new("README.md")), None);
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

#[cfg(test)]
mod bench {
    use super::*;

    /// 索引ルートの列挙はイベントループの中で走る。ツリーを歩くコストが
    /// フレームの予算 (16ms) を大きく超えるなら、置き場所を変える必要がある。
    #[test]
    #[ignore = "実リポジトリが要る"]
    fn 実リポジトリでの列挙にかかる時間() {
        let root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
        let at = std::time::Instant::now();
        let found = discover(Path::new(&root));
        println!("{} ルート / {:?}", found.len(), at.elapsed());
        for r in &found {
            println!("  {:?} {}", r.lang, r.subroot.display());
        }
    }
}
