//! sheaf-core の [`Store`] を conductor に橋渡しする層。
//!
//! # なぜ `.conductor/` を main worktree 側から読むか
//!
//! リンクされた worktree は自分の `.conductor/` を持たない。これは既存の前提で、
//! レビュー DB も同じ扱いをしている（[`crate::mcp_serve::resolve`]）。索引ファイルを
//! worktree ごとに置く運用にはしないので、`load` は `repo_root` から `commondir()`
//! を辿って main worktree のパスを解決する。`repo_root` にはどの worktree のパスを
//! 渡してもよい（呼び出し元がリンクされた worktree で起動していても構わない）。
//! 照合先のツリー（`tree_root`）は選択中の worktree のパスで、これは別物である。
//!
//! # なぜ出自の申告が要るか
//!
//! SCIP 索引はソース本文を持たない（rust-analyzer が `Document.text` を空にする）ため、
//! 索引だけを見ても「生成した時点でファイルがどんな内容だったか」が分からない。
//! `Store::load` が受け取る期待ハッシュの表は、その出自を外から渡すためのもの。
//! 出自はコミットではなく生成時にディスク上にあった内容のハッシュで記録する。
//! `make index` が作業ツリーを索引する一方でコミットを出自として申告すると、
//! 未追跡ファイルが索引に載っていても永久に鮮度の検査を通らなくなる。
//!
//! # 触るファイル
//!
//! 索引ルート 1 本につき `.conductor/` に `index.<言語>[.<ルート>].{scip,hashes,log}` の
//! 3 つ。`scip` が索引そのもの、`hashes` が生成した時点の内容ハッシュの表で、
//! 1 行 1 ファイルの `<sha1> <相対パス>`。名前の付け方は [`roots`] にある。

mod bridge;
mod roots;

pub use bridge::Bridge;
pub use sheaf_core::Outcome as Regenerated;

use roots::IndexRoot;
use sheaf_core::{IndexSource, Regenerator, Slot, Store};
use std::path::{Path, PathBuf};

/// 索引ルート 1 本と、その作り直し係。
struct Root {
    at: IndexRoot,
    regenerator: Regenerator,
}

impl Root {
    /// 作り直しを待っているか、走っているか。
    fn is_working(&self) -> bool {
        self.regenerator.is_pending() || self.regenerator.is_running()
    }
}

/// 索引 (sheaf-core の [`Store`]) の有無と、その作り直しを保持する。
///
/// `Store` は 1 つで、ツリーの中の索引ルートすべてを束ねたもの。作り直しはルートごとに
/// 独立して走る (道具も成果物も別なので、片方の失敗をもう片方に伝播させない)。
#[derive(Default)]
pub struct SemanticIndex {
    slot: Slot,
    /// いま列挙してある索引ルートと、その列挙元のツリー。ツリーが変われば引き直す。
    roots: Vec<Root>,
    tree: PathBuf,
    /// ツリーを引き直す前に来た「1 本作ってほしい」。
    requested: bool,
    /// main worktree の `.conductor/`。解決に git2 のリポジトリオープンが要るので
    /// 覚えておく。外側の `None` は「まだ引いていない」、内側は「git リポジトリでない」。
    conductor_dir: Option<Option<PathBuf>>,
}

impl SemanticIndex {
    /// `tree_root` に向いている索引。向いていなければ `None`。
    pub fn store(&self, tree_root: &Path) -> Option<&Store> {
        self.slot.get(tree_root)
    }

    /// 索引がまだ無いので 1 本作ってほしい、と伝える。
    ///
    /// 生成が起きるのは編集が収束したときだけなので、これが無いと索引の無い
    /// リポジトリでは何か 1 ファイル編集するまで全部が構文層に落ち続ける。
    /// その差は画面に出ないので、ユーザからは「ジャンプが甘い」としか見えない。
    ///
    /// 索引ルートの列挙にはツリーが要るが、ここには渡ってこない。覚えておいて
    /// [`Self::tick_regeneration`] で配る。
    pub fn request_build(&mut self) {
        self.requested = true;
    }

    /// ファイルが変わったことを伝える。変更を数えるのは、それを含む索引ルートだけ。
    pub fn note_change(&mut self, changed: &Path, tree_root: &Path) {
        self.sync_roots(tree_root);
        for root in &mut self.roots {
            let at = tree_root.join(&root.at.subroot);
            root.regenerator.note_change(changed, &at);
        }
    }

    /// 作り直しを 1 周進める。始めどきなら始め、終わっていれば結果を返す。
    ///
    /// 毎フレーム呼ばれる。待つものが無いうちにツリーを歩いたり置き場所を
    /// 組み立てたりしないのは、そこにファイルシステムと git2 の参照が要るため。
    pub fn tick_regeneration(&mut self, repo_root: &Path, tree_root: &Path) -> Option<Regenerated> {
        if !self.requested && !self.roots.iter().any(|r| r.is_working()) {
            return None;
        }
        self.sync_roots(tree_root);
        let dir = self.conductor_dir(repo_root)?.to_path_buf();
        if std::mem::take(&mut self.requested) {
            for root in &mut self.roots {
                root.regenerator.request();
            }
        }
        // 1 周で返すのは 1 本ぶん。生成はロックで直列化されているので、
        // 同じ周に 2 本が終わることはほとんど無い。
        self.roots.iter_mut().find_map(|root| {
            let target = root.at.target(&dir, tree_root);
            root.regenerator.tick(&target)
        })
    }

    /// 索引ルートの列挙を `tree_root` に合わせる。ツリーが変わっていなければ何もしない。
    fn sync_roots(&mut self, tree_root: &Path) {
        if self.tree == tree_root {
            return;
        }
        self.tree = tree_root.to_path_buf();
        // 走っている生成は前のツリーを索引している。Regenerator を捨てれば
        // Drop がプロセスグループごと止める。
        self.roots = roots::discover(tree_root)
            .into_iter()
            .map(|at| Root {
                regenerator: Regenerator::new(at.lang.producer()),
                at,
            })
            .collect();
    }

    fn conductor_dir(&mut self, repo_root: &Path) -> Option<&Path> {
        self.conductor_dir
            .get_or_insert_with(|| main_conductor_dir(repo_root))
            .as_deref()
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.roots.iter().any(|r| r.regenerator.is_pending())
    }

    /// 走っている生成を止める。worktree を切り替えたときに呼ぶ。
    pub fn abort_regeneration(&mut self) {
        self.requested = false;
        for root in &mut self.roots {
            root.regenerator.abort();
        }
    }

    /// 別のツリーを見に行くことになったなら、読み直しを待たずに捨てる。
    pub fn invalidate_if_retargeted(&mut self, tree_root: &Path) {
        self.slot.retarget(tree_root);
    }

    /// 背景ロードの結果を取り込む。取り込まなかったときは `false` を返すので、
    /// 呼び出し側はそれを見て読み直しを起こす。
    pub fn accept(&mut self, requested: &Path, current: &Path, store: Option<Store>) -> bool {
        self.slot.accept(requested, current, store)
    }
}

/// ツリーの索引ルートをすべて索引して置く。`conductor index` の実体。
///
/// 初回の 1 本を作るための口。以後は編集が収束するたびに conductor 自身が作り直す。
///
/// 1 本が失敗しても残りは作る。道具は言語ごとに別なので、scip-go が入っていない
/// ことを理由に Rust の索引まで諦めるのは筋が違う。
pub fn build_index(repo_root: &Path) -> anyhow::Result<()> {
    let dir = main_conductor_dir(repo_root)
        .ok_or_else(|| anyhow::anyhow!("{} が git リポジトリではない", repo_root.display()))?;
    let found = roots::discover(repo_root);
    if found.is_empty() {
        anyhow::bail!(
            "{} に索引の作り方が分からない (Cargo.toml / go.mod / tsconfig.json のどれも無い)",
            repo_root.display()
        );
    }

    let mut failures = Vec::new();
    for at in &found {
        let target = at.target(&dir, repo_root);
        let index = target.index.clone();
        match sheaf_core::generate_once(target, at.lang.producer()) {
            Regenerated::Ready { store, .. } => println!(
                "{} に索引を置いた ({} document、うち出自を言えないもの {})",
                index.display(),
                store.len(),
                store.missing_provenance()
            ),
            Regenerated::Failed(why) | Regenerated::Unavailable(why) => {
                failures.push(format!("{:?}: {why}", at.lang))
            }
            Regenerated::Busy => {
                failures.push(format!("{:?}: ほかのプロセスが索引を作っている", at.lang))
            }
        }
    }

    if failures.len() == found.len() {
        anyhow::bail!("{}", failures.join("\n"));
    }
    for why in &failures {
        eprintln!("索引を作れなかった {why}");
    }
    Ok(())
}

/// main worktree に置いてある索引を、`tree_root` のツリーに向けてロードする。
///
/// 索引ルートは `tree_root` から引き直す。索引の中の相対パスをツリーのどこへ
/// 接ぎ木するかは、ツリーの側にあるルートで決まるため。
///
/// 1 本も投入できなければ `None`。
pub fn load(repo_root: &Path, tree_root: &Path) -> Option<Store> {
    let conductor_dir = main_conductor_dir(repo_root)?;
    let sources: Vec<IndexSource> = roots::discover(tree_root)
        .iter()
        .filter_map(|at| at.source(&conductor_dir))
        .collect();
    if sources.is_empty() {
        return None;
    }
    Store::load(&sources, tree_root).ok()
}

/// `repo_root` の main worktree での `.conductor/` を返す。
///
/// `repo_root` がリンクされた worktree のパスでも、`commondir()` は常に main の
/// `.git` を指すので、その親を辿れば main のルートになる
/// （`mcp_serve::resolve::resolve_db_path_with` が同じ考え方でレビュー DB を探している）。
fn main_conductor_dir(repo_root: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::open(repo_root).ok()?;
    Some(repo.commondir().parent()?.join(".conductor"))
}

/// 種別を、ホバーの見出しに置く 1 語にする。
///
/// 綴りは Rust の宣言キーワードに寄せてある。ホバーの本文には索引が書いた宣言が
/// そのまま並ぶので、見出しだけ英語の分類名 (`Function`) にすると 2 つの語彙が
/// 混ざる。読めない種別は空にして、見出しごと出さない。
pub fn kind_label(kind: sheaf_core::SymbolKind) -> &'static str {
    use sheaf_core::SymbolKind::*;
    match kind {
        Function => "fn",
        Method => "method",
        Struct => "struct",
        Class => "class",
        Enum => "enum",
        EnumMember => "variant",
        Field => "field",
        Trait => "trait",
        Interface => "interface",
        Package => "package",
        TypeAlias => "type",
        AssociatedType => "assoc type",
        ImplBlock => "impl",
        Module => "mod",
        Constant => "const",
        Static => "static",
        Variable => "let",
        Parameter => "param",
        SelfParameter => "self",
        TypeParameter => "type param",
        Unknown => "",
    }
}

#[cfg(test)]
mod tests {
    use super::roots::Language;
    use super::*;

    /// 索引ルートの目印。これが無いツリーは索引の対象にならないので、
    /// 索引を置く検査ではソースと一緒にこれも置く。
    const CARGO_TOML: (&str, &str) = ("Cargo.toml", "[package]\nname = \"demo\"\n");

    /// Rust の索引を作る道具。出自の表の読み書きは道具ごとに照合されるので、
    /// 検査でも本番と同じものを渡す。
    fn producer() -> std::sync::Arc<dyn sheaf_core::Producer> {
        Language::Rust.producer()
    }

    /// Rust の索引ルート 1 本ぶんの成果物の名前。
    fn artifact(dir: &Path, ext: &str) -> PathBuf {
        dir.join(format!("index.rust.{ext}"))
    }

    /// commit_sha でコミットした状態のリポジトリを tempdir に作る。
    /// 戻り値は (repo_root, commit_sha)。
    fn init_repo_with_commit(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }

        let mut index = repo.index().unwrap();
        for (rel, _) in files {
            index.add_path(Path::new(rel)).unwrap();
        }
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        let commit_id = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        (dir, commit_id.to_string())
    }

    #[test]
    fn no_scip_and_no_hashes_file_is_none() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        assert!(load(dir.path(), dir.path()).is_none());
    }

    #[test]
    fn missing_scip_file_is_none() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        std::fs::write(artifact(&conductor_dir, "hashes"), "").unwrap();
        assert!(load(dir.path(), dir.path()).is_none());
    }

    /// 引数を無視してひたすら sleep するだけの producer。生成が走っている最中を
    /// 安定して作るために使う。
    struct SlowProducer(PathBuf);

    impl sheaf_core::Producer for SlowProducer {
        fn command(&self, _out: &Path) -> Vec<String> {
            vec![self.0.to_string_lossy().into_owned()]
        }
    }

    #[test]
    fn tick_regeneration_は生成が走っていても即座に返る() {
        // 「バックグラウンドでやっている」ことの回帰ガード。索引の読み込みや
        // 生成の待ち合わせがここに紛れ込むと、呼び出し元(イベントループ)を止める。
        use std::time::{Duration, Instant};

        // Rust のツリーとして認識されないと生成そのものが起きない (target を参照)。
        let (dir, _commit) = init_repo_with_commit(&[
            ("src/lib.rs", "fn f() {}\n"),
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ]);
        std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();

        let script = dir.path().join("slow-producer.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        // 索引ルートを直に組む。ツリーを既に引いてあることにしておかないと、
        // sync_roots が rust-analyzer を持つ Regenerator に差し替えてしまう。
        let mut semantic = SemanticIndex {
            tree: dir.path().to_path_buf(),
            roots: vec![Root {
                at: IndexRoot {
                    subroot: PathBuf::new(),
                    lang: Language::Rust,
                },
                regenerator: sheaf_core::Regenerator::new(std::sync::Arc::new(SlowProducer(
                    script,
                ))),
            }],
            ..Default::default()
        };
        semantic.note_change(&dir.path().join("src/lib.rs"), dir.path());

        // 静穏時間が経つのを待って、生成が実際に走っている状態を作る。
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let outcome = semantic.tick_regeneration(dir.path(), dir.path());
            assert!(
                outcome.is_none(),
                "30 秒 sleep する producer がもう終わっているはずがない"
            );
            if !semantic.is_pending() {
                break;
            }
            assert!(Instant::now() < deadline, "生成が始まらない");
            std::thread::sleep(Duration::from_millis(100));
        }

        let at = Instant::now();
        let outcome = semantic.tick_regeneration(dir.path(), dir.path());
        let elapsed = at.elapsed();

        assert!(outcome.is_none());
        assert!(
            elapsed < Duration::from_millis(200),
            "tick が生成を待ってしまっている: {elapsed:?}"
        );

        semantic.abort_regeneration();
    }

    #[test]
    fn 索引ルートの無いツリーでは生成を起こさない() {
        // どの目印も無いツリーに道具を向けても意味が無い。しかも認識できない対象に
        // 対して終了コード 0 で空の索引を書くことがあるので、起こさないこと自体が答え。
        let (dir, _commit) = init_repo_with_commit(&[("main.go", "package main\n")]);
        let mut semantic = SemanticIndex::default();
        semantic.note_change(&dir.path().join("main.go"), dir.path());

        assert!(
            semantic.tick_regeneration(dir.path(), dir.path()).is_none(),
            "目印が無いのに生成が始まった"
        );
    }

    #[test]
    fn go_のツリーには_scip_go_を向ける() {
        // ここが Rust 決め打ちだと、Go のリポジトリは索引が 1 本も無いまま
        // tree-sitter の名前一致に落ち続ける。画面には出ないので気づけない。
        let (dir, _commit) =
            init_repo_with_commit(&[("go.mod", "module demo\n"), ("main.go", "package main\n")]);

        let mut semantic = SemanticIndex::default();
        semantic.note_change(&dir.path().join("main.go"), dir.path());

        assert!(semantic.is_pending(), "go.mod があるのに生成を待っていない");
        let argv = semantic.roots[0]
            .at
            .lang
            .producer()
            .command(Path::new("/o"));
        assert_eq!(argv[0], "scip-go");
    }

    /// conductor が Go のツリーに scip-go を向け、その索引で `Exact` に答えるまでを
    /// 一続きで見る。読み取り側は sheaf のテストが見ているので、ここで見たいのは
    /// host 側の配線 (道具の選択・成果物の名前・投入) だけ。
    ///
    /// scip-go が無ければ飛ばさずに落とす。飛ばすと、配線が壊れていても緑になる。
    #[test]
    fn go_のツリーを索引して定義に飛べる() {
        let (dir, _commit) = init_repo_with_commit(&[
            ("go.mod", "module example.com/app\n\ngo 1.21\n"),
            (
                "pkg/greet/greet.go",
                "package greet\n\nfunc Greet() string {\n\treturn \"hi\"\n}\n",
            ),
            (
                "main.go",
                "package main\n\nimport \"example.com/app/pkg/greet\"\n\nfunc main() {\n\tprintln(greet.Greet())\n}\n",
            ),
        ]);
        let root = dir.path();
        build_index(root).expect("Go のツリーを索引できない");

        let store = load(root, root).expect("置いた索引を読めない");
        let rel = Path::new("main.go");
        let source = std::fs::read_to_string(root.join(rel)).unwrap();
        let (line, text) = source
            .lines()
            .enumerate()
            .find(|(_, t)| t.contains("greet.Greet()"))
            .unwrap();
        let col = text.find("Greet()").unwrap();

        let mask = crate::symbol_index::CodeMask::compute(&source, "main.go");
        let index = crate::symbol_index::SymbolIndex::new(root.to_path_buf());
        let bridge = Bridge {
            abs_path: &root.join(rel),
            source: &source,
            mask: &mask,
            index: &index,
        };
        assert_eq!(
            sheaf_core::definition_at(&store, &bridge, rel, line as u32, col as u32),
            sheaf_core::Definition::Exact(vec![sheaf_core::Location {
                path: PathBuf::from("pkg/greet/greet.go"),
                line: 2,
                col: 5,
            }])
        );
    }

    #[test]
    fn missing_hashes_file_is_none() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        // 本物の SCIP を置いても、出自の表が無ければ None を返すこと。
        // ここで壊れた索引を置くと、出自の表を見ずに投入が失敗しても
        // 結果として None になり、このテストが検査したいことを検査できなくなる。
        write_index(&artifact(&conductor_dir, "scip"));
        assert!(load(dir.path(), dir.path()).is_none());
    }

    const SOURCE: &str = "pub fn greet() {}\nfn caller() { greet(); }\n";
    const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";

    /// 渡した各ファイルが SOURCE を説明する索引を書き出す。greet の定義が 0 行目、
    /// 呼び出しが 1 行目。
    ///
    /// シンボルはファイルごとに変える。`Store` はシンボル文字列だけで定義を引く
    /// (ファイルをまたいで同じ文字列を使うと別ファイルの定義まで拾ってしまう)ので、
    /// 複数ファイルを 1 つの索引に入れるときは同じ SYMBOL を使い回せない。
    fn write_index_for(path: &Path, rels: &[&str]) {
        use protobuf::{EnumOrUnknown, Message, MessageField};
        use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};

        let documents = rels
            .iter()
            .map(|rel| {
                let symbol = format!("{SYMBOL} {rel}");
                let occurrence = |range: Vec<i32>, roles: i32| Occurrence {
                    range,
                    symbol: symbol.clone(),
                    symbol_roles: roles,
                    ..Default::default()
                };
                Document {
                    relative_path: rel.to_string(),
                    language: "rust".to_string(),
                    occurrences: vec![
                        occurrence(vec![0, 7, 12], 1),
                        occurrence(vec![1, 14, 19], 0),
                    ],
                    ..Default::default()
                }
            })
            .collect();
        let index = Index {
            metadata: MessageField::some(Metadata {
                text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
                ..Default::default()
            }),
            documents,
            ..Default::default()
        };
        std::fs::write(path, index.write_to_bytes().unwrap()).unwrap();
    }

    fn write_index(path: &Path) {
        write_index_for(path, &["src/lib.rs"]);
    }

    /// `fn caller() { greet(); }` の greet の位置を、索引に向けて引く。
    fn definition_of_greet_at(
        store: &Store,
        tree_root: &Path,
        rel: &str,
    ) -> sheaf_core::Definition {
        let abs = tree_root.join(rel);
        let source = std::fs::read_to_string(&abs).unwrap();
        let mask = crate::symbol_index::CodeMask::compute(&source, rel);
        let index = crate::symbol_index::SymbolIndex::new(tree_root.to_path_buf());
        let bridge = Bridge {
            abs_path: &abs,
            source: &source,
            mask: &mask,
            index: &index,
        };
        sheaf_core::definition_at(store, &bridge, Path::new(rel), 1, 14)
    }

    fn definition_of_greet(store: &Store, tree_root: &Path) -> sheaf_core::Definition {
        definition_of_greet_at(store, tree_root, "src/lib.rs")
    }

    /// Rust の索引と、`repo_root` に今ある `src/lib.rs` の内容から計算した出自の表を
    /// 置く。「生成時点でディスクにあった内容」を申告する体で、コミットされているか
    /// どうかは問わない。
    fn place_index(repo_root: &Path) {
        let conductor_dir = repo_root.join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        write_index(&artifact(&conductor_dir, "scip"));
        let content = std::fs::read(repo_root.join("src/lib.rs")).unwrap();
        write_hashes(
            &artifact(&conductor_dir, "hashes"),
            &[("src/lib.rs", sheaf_core::blob_hash(&content))],
        );
    }

    /// 出自の表を置く。書式は sheaf の持ち物なので `write_provenance` に書かせる。
    /// 見出しには producer の素性が入り、読む側がそれを照合する。手で綴ると、
    /// 読み書きのどちらかが変わったときに、表が黙って読まれなくなる。
    fn write_hashes(path: &Path, entries: &[(&str, String)]) {
        let expected = entries
            .iter()
            .map(|(rel, hash)| (PathBuf::from(rel), hash.clone()))
            .collect();
        sheaf_core::write_provenance(path, &*producer(), &expected).unwrap();
    }

    #[test]
    fn indexed_tree_answers_with_exact() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let store = load(dir.path(), dir.path()).expect("索引と出自の申告が揃っている");
        assert_eq!(
            definition_of_greet(&store, dir.path()),
            sheaf_core::Definition::Exact(vec![sheaf_core::Location {
                path: PathBuf::from("src/lib.rs"),
                line: 0,
                col: 7,
            }])
        );
    }

    #[test]
    fn linked_worktree_finds_the_index_at_the_main_worktree() {
        // リンクされた worktree の Repository::workdir() はリンク先自身を指すので、
        // repo_root にそれをそのまま渡すと、main 側にしか無い .conductor/ が
        // 見つからない。commondir() を辿って main を解決できているかを確かめる。
        let (dir, commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let wt_parent = tempfile::tempdir().unwrap();
        let wt_path = wt_parent.path().join("linked-wt");
        let status = std::process::Command::new("git")
            // ユーザのグローバル/システム git 設定から隔離し、テスト対象と
            // 無関係な理由で失敗しないようにする。
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-C",
                dir.path().to_str().unwrap(),
                "worktree",
                "add",
                "-b",
                "wt-branch",
                wt_path.to_str().unwrap(),
                &commit,
            ])
            .status()
            .unwrap();
        assert!(status.success(), "git worktree add failed");

        let store = load(&wt_path, &wt_path).expect("main 側の索引が見つかるはず");
        assert!(matches!(
            definition_of_greet(&store, &wt_path),
            sheaf_core::Definition::Exact(_)
        ));
    }

    #[test]
    fn other_tree_with_the_same_content_still_answers() {
        // worktree の形。内容が同じファイルは索引を使い回せる。これが成り立たないと
        // worktree ごとに索引を作ることになり、この設計の意味が無くなる。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join("src")).unwrap();
        std::fs::write(other.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
        std::fs::write(other.path().join("src/lib.rs"), SOURCE).unwrap();

        let store = load(dir.path(), other.path()).expect("索引と出自の申告が揃っている");
        assert!(matches!(
            definition_of_greet(&store, other.path()),
            sheaf_core::Definition::Exact(_)
        ));
    }

    #[test]
    fn other_tree_with_a_changed_file_does_not_answer() {
        // 同じ worktree の形だが、そのファイルが編集されている。索引の言う 0 行目は
        // もう greet の定義ではないので、確信度つきで答えてはいけない。
        //
        // 聞く行(1行目)は両方のツリーで同じにしてある。ここを動かすと、問い合わせ位置が
        // 別の語にずれて「語が無い」で落ちるだけになり、鮮度を検査しないまま緑になる。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join("src")).unwrap();
        std::fs::write(other.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
        std::fs::write(
            other.path().join("src/lib.rs"),
            "pub fn hello() {}\nfn caller() { greet(); }\n",
        )
        .unwrap();

        let store = load(dir.path(), other.path()).expect("索引と出自の申告が揃っている");
        assert!(!matches!(
            definition_of_greet(&store, other.path()),
            sheaf_core::Definition::Exact(_)
        ));
    }

    /// 実際に置かれている索引で、git2 のツリー走査と投入が通ることを見る。
    /// 合成した索引では、実索引の Document 数(345)やパスの綴りまでは検査できない。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_loads_from_the_repository_it_was_generated_for() {
        let repo_root = std::env::var("CONDUCTOR_TEST_REPO")
            .expect("CONDUCTOR_TEST_REPO に .conductor/ へ索引を置いたリポジトリのパスを渡すこと");
        let repo_root = Path::new(&repo_root);

        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");
        println!(
            "{} Document / ルート外 {} / 保持 {:.1}MB",
            store.len(),
            store.outside_root(),
            store.retained_bytes() as f64 / 1048576.0,
        );
        assert!(!store.is_empty());
        assert_eq!(store.outside_root(), 0, "ツリー外を指す Document がある");
    }

    /// 索引が説明を答える割合を、実リポジトリの実 Bridge (tree-sitter) 越しに測る。
    ///
    /// 合成した索引では、rust-analyzer が実際に何を書くかを検査できない。ここが
    /// 落ちるのは、宣言の綴りが変わって読めなくなったとき (`Signature` の
    /// フィールド番号がまさにそれ) と、種別の番号が変わったとき。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_describes_what_it_answers() {
        use crate::app::{code_identifiers_on_line, occurrence_span_in_source};

        let repo_root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
        let repo_root = Path::new(&repo_root);
        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");

        // 索引はこのワークスペースのシンボルしか説明を持たない (rust-analyzer は
        // SCIP の external_symbols を書かないので、std や ratatui の語は符号だけ)。
        // 全体の割合で見ると、その欠落と自前の欠落が混ざって回帰に気づけない。
        let own = |symbol: &str| {
            symbol.starts_with("local ")
                || matches!(
                    symbol.split(' ').nth(2),
                    Some("conductor" | "sheaf-core" | "revidere" | "revidere-fixtures")
                )
        };

        let (mut asked, mut described, mut own_described, mut own_signature) = (0, 0, 0, 0);
        let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
        for rel in [
            "src/repo_path.rs",
            "src/jump_history.rs",
            "src/hover_info.rs",
        ] {
            let abs = repo_root.join(rel);
            let source = std::fs::read_to_string(&abs).unwrap();
            let mask = crate::symbol_index::CodeMask::compute(&source, rel);
            let index = crate::symbol_index::SymbolIndex::new(repo_root.to_path_buf());
            let bridge = Bridge {
                abs_path: &abs,
                source: &source,
                mask: &mask,
                index: &index,
            };
            for (line, text) in source.lines().enumerate() {
                for (k, _, word) in code_identifiers_on_line(text, line + 1, &mask) {
                    let Some((start, end)) = occurrence_span_in_source(text, k) else {
                        continue;
                    };
                    if text.get(start..end) != Some(word.as_str()) {
                        continue;
                    }
                    asked += 1;
                    let answer = sheaf_core::describe_at(
                        &store,
                        &bridge,
                        Path::new(rel),
                        line as u32,
                        start as u32,
                    );
                    let Some(detail) = answer.first() else {
                        continue;
                    };
                    described += 1;
                    if !own(detail.symbol.as_str()) {
                        continue;
                    }
                    own_described += 1;
                    if detail.signature.is_some() {
                        own_signature += 1;
                    }
                    let label = kind_label(detail.kind);
                    if !label.is_empty() {
                        *kinds.entry(label).or_default() += 1;
                    }
                }
            }
        }

        println!(
            "聞いた {asked} / 符号が付いた {described} / うち自前 {own_described} / 宣言 {own_signature}"
        );
        println!("種別の内訳: {kinds:?}");
        assert!(
            own_described > 100,
            "索引がほとんど答えていない: {own_described}"
        );
        // 自前のシンボルには索引が必ず SymbolInformation を書く。ここが落ちるのは
        // 宣言の綴りか種別の番号が変わったとき。
        assert!(
            own_signature * 20 >= own_described * 19,
            "自前のシンボルの宣言が読めていない: {own_signature}/{own_described}"
        );
        let with_kind: usize = kinds.values().sum();
        assert!(
            with_kind * 20 >= own_described * 19,
            "自前のシンボルの種別が読めていない: {with_kind}/{own_described}"
        );
        // 分類が 1 種類に潰れていたら、番号の対応表が壊れている。
        assert!(kinds.len() >= 5, "種別が偏りすぎ: {kinds:?}");
    }

    /// 呼び出し口(`App::pick_line_identifier`)が選ばせうる位置を、リポジトリの
    /// 実ファイルで全部叩く。索引が実際にどれだけ答えるかと、飛び先が
    /// リポジトリ内の実在する位置であることを見る。
    ///
    /// 呼び出し口は viewer が持つタブ展開済みの行から出現インデックスを取るが、
    /// 対象は Rust なのでタブを含む行が無く、ここでは元ソースから取っている。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_answers_across_the_repository() {
        use crate::app::{code_identifiers_on_line, occurrence_span_in_source};

        let repo_root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
        let repo_root = Path::new(&repo_root);
        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");

        let dirty = std::process::Command::new("git")
            .args([
                "-C",
                repo_root.to_str().unwrap(),
                "diff",
                "--name-only",
                "HEAD",
            ])
            .output()
            .unwrap();
        let dirty: Vec<String> = String::from_utf8_lossy(&dirty.stdout)
            .lines()
            .map(String::from)
            .collect();

        let (mut asked, mut exact, mut examples) = (0usize, 0usize, Vec::new());
        let (mut containers, mut named) = (0usize, Vec::new());
        let mut slowest = std::time::Duration::ZERO;
        for rel in [
            "src/repo_path.rs",
            "src/jump_history.rs",
            "src/background.rs",
        ] {
            let abs = repo_root.join(rel);
            let source = std::fs::read_to_string(&abs).unwrap();
            let mask = crate::symbol_index::CodeMask::compute(&source, rel);
            let index = crate::symbol_index::SymbolIndex::new(repo_root.to_path_buf());
            let bridge = Bridge {
                abs_path: &abs,
                source: &source,
                mask: &mask,
                index: &index,
            };
            for (line, text) in source.lines().enumerate() {
                for (k, _, word) in code_identifiers_on_line(text, line + 1, &mask) {
                    let Some((start, end)) = occurrence_span_in_source(text, k) else {
                        continue;
                    };
                    if text.get(start..end) != Some(word.as_str()) {
                        continue;
                    }
                    asked += 1;
                    let at = std::time::Instant::now();
                    let answer = sheaf_core::definition_at(
                        &store,
                        &bridge,
                        Path::new(rel),
                        line as u32,
                        start as u32,
                    );
                    slowest = slowest.max(at.elapsed());
                    if let sheaf_core::Definition::Exact(locations) = answer {
                        exact += 1;
                        if let Some(path) = sheaf_core::describe_at(
                            &store,
                            &bridge,
                            Path::new(rel),
                            line as u32,
                            start as u32,
                        )
                        .iter()
                        .find_map(|d| d.container.clone())
                        {
                            containers += 1;
                            if named.len() < 5 {
                                named.push(format!("  {word} <- {path}"));
                            }
                        }
                        for loc in &locations {
                            // 飛び先は必ずリポジトリ内の実在する行でなければならない。
                            let target = repo_root.join(&loc.path);
                            let text = std::fs::read_to_string(&target).unwrap_or_else(|_| {
                                panic!("飛び先が存在しない: {}", target.display())
                            });
                            assert!(
                                (loc.line as usize) < text.lines().count(),
                                "飛び先の行がファイルの外: {}:{}",
                                loc.path.display(),
                                loc.line
                            );
                        }
                        if examples.len() < 5 {
                            examples.push(format!("  {rel}:{line} {word} -> {:?}", locations[0]));
                        }
                    }
                }
            }
        }

        println!("汚れているファイル {} 件", dirty.len());
        println!("問い合わせ {asked} 箇所 / Exact {exact} / 最遅 1 クエリ {slowest:?}");
        println!("所属の綴りが出た {containers} 箇所");
        for n in &named {
            println!("{n}");
        }
        for e in &examples {
            println!("{e}");
        }
        assert!(exact > 0, "索引が1件も答えていない");
        assert!(
            slowest < std::time::Duration::from_millis(100),
            "1 クエリが gd の予算(100ms)を超えた: {slowest:?}"
        );
    }

    // 「向き先が違うツリーには答えない」の検査は sheaf 側 (`Slot`) にある。
    // 判定そのものがあちらにあるので、こちらに写しを置くと片方だけが古くなる。

    #[test]
    fn 読めない種別は見出しごと出さない() {
        // 種別を読めなかったときにそれらしい名前を返すと、ホバーが自信を持って嘘を出す。
        assert_eq!(kind_label(sheaf_core::SymbolKind::Unknown), "");
        assert_eq!(kind_label(sheaf_core::SymbolKind::Function), "fn");
        assert_eq!(kind_label(sheaf_core::SymbolKind::Variable), "let");
    }

    #[test]
    fn nested_paths_are_spelled_the_way_scip_spells_them() {
        // 表の鍵は SCIP の relative_path と突き合わせられる。綴りがずれると
        // 一致するファイルが 1 つも無くなり、全部が構文層に落ちる。誤答にはならないので
        // 気づけず、テストも緑のままになる。深い階層で綴りを固定しておく。
        let dir = tempfile::tempdir().unwrap();
        let hashes_path = dir.path().join("index.hashes");
        write_hashes(
            &hashes_path,
            &[
                ("src/deep/nested/lib.rs", "0".repeat(40)),
                ("top.rs", "1".repeat(40)),
            ],
        );

        let hashes = sheaf_core::read_provenance(&hashes_path, &*producer()).unwrap();

        let mut keys: Vec<_> = hashes.keys().map(|p| p.to_string_lossy()).collect();
        keys.sort();
        assert_eq!(keys, ["src/deep/nested/lib.rs", "top.rs"]);
    }

    /// `index.hashes` に書かれたハッシュが、そのまま(取り直さずに)期待ハッシュの表に
    /// 入ることを確かめる。
    #[test]
    fn expected_hashes_uses_the_recorded_hash_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let hashes_path = dir.path().join("index.hashes");
        let hash = sheaf_core::blob_hash(b"fn f() {}\n");
        write_hashes(&hashes_path, &[("src/lib.rs", hash.clone())]);

        let hashes = sheaf_core::read_provenance(&hashes_path, &*producer()).unwrap();
        assert_eq!(hashes.get(Path::new("src/lib.rs")), Some(&hash));
    }

    /// `load` が実際に返す `Store` の鮮度判定を、4 通りのファイルで見る。生きたリポジトリの
    /// Exact 率は汚れ具合で変わるため assert できないので、固定の一時リポジトリに
    /// 1 ファイルずつ用意して個別に検査する。
    #[test]
    fn provenance_table_governs_freshness_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // (a) 生成後に触っていないファイル。
        let untouched = "src/untouched.rs";
        // (b) コミットされていない(未追跡の)ファイル。ここが今日永久に落ちている箇所。
        let untracked = "src/untracked.rs";
        // (c) 生成後に編集されたファイル。
        let edited = "src/edited.rs";
        // (d) 生成の前後でハッシュが食い違うファイル。index.hashes に載らない。
        let racy = "src/racy.rs";

        std::fs::write(dir.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
        for rel in [untouched, untracked, edited, racy] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, SOURCE).unwrap();
        }

        // untouched と edited はコミットしておく。untracked は git に足さないままにして、
        // 出自の申告が git のトラッキング状態と無関係に効くことを確かめる。
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(untouched)).unwrap();
        index.add_path(Path::new(edited)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        write_index_for(
            &artifact(&conductor_dir, "scip"),
            &[untouched, untracked, edited, racy],
        );

        // 生成手順を模す。racy は前後でハッシュが食い違った体にして書かない。
        let hash = sheaf_core::blob_hash(SOURCE.as_bytes());
        write_hashes(
            &artifact(&conductor_dir, "hashes"),
            &[
                (untouched, hash.clone()),
                (untracked, hash.clone()),
                (edited, hash),
            ],
        );

        // edited は生成が終わった後に編集される。呼び出し箇所(1行目)は変えていないので、
        // クエリの単語自体は引き続き greet を指す。
        std::fs::write(
            dir.path().join(edited),
            "pub fn hello() {}\nfn caller() { greet(); }\n",
        )
        .unwrap();

        let store = load(dir.path(), dir.path()).expect("索引と出自の申告が揃っている");

        assert!(matches!(
            definition_of_greet_at(&store, dir.path(), untouched),
            sheaf_core::Definition::Exact(_)
        ));
        assert!(
            matches!(
                definition_of_greet_at(&store, dir.path(), untracked),
                sheaf_core::Definition::Exact(_)
            ),
            "未追跡でも index.hashes に載っていれば Exact になるはず(今日の欠陥の修正対象)"
        );
        assert!(!matches!(
            definition_of_greet_at(&store, dir.path(), edited),
            sheaf_core::Definition::Exact(_)
        ));
        assert!(!matches!(
            definition_of_greet_at(&store, dir.path(), racy),
            sheaf_core::Definition::Exact(_)
        ));
    }
}
