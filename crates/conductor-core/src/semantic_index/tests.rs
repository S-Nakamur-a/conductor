//! semantic_index の検査。
//!
//! 実リポジトリの索引が要るものは `#[ignore]` にしてある (`CONDUCTOR_TEST_REPO`)。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sheaf_core::{Definition, Location, Producer, Store};

use super::history::{self, Outcome, SourceDelta, Sources, Trigger};
use super::roots::{self, IndexRoot, Language};
use super::survey::load;
use super::*;
use crate::symbol_index::{CodeMask, SymbolIndex};

// ---------------------------------------------------------------- 素材

/// 索引ルートの目印。これが無いツリーは索引の対象にならないので、索引を置く検査では
/// ソースと一緒にこれも置く。
const CARGO_TOML: (&str, &str) = ("Cargo.toml", "[package]\nname = \"demo\"\n");
const SOURCE: &str = "pub fn greet() {}\nfn caller() { greet(); }\n";
const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";

fn producer() -> Arc<dyn Producer> {
    Language::Rust.producer()
}

fn at(subroot: &str, lang: Language) -> IndexRoot {
    IndexRoot {
        subroot: PathBuf::from(subroot),
        lang,
    }
}

/// `files` を書いたツリー。git は初期化しない。
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
    dir
}

/// `files` をコミットしたリポジトリと、そのコミット。
fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
    let dir = tree(files);
    let repo = git2::Repository::init(dir.path()).unwrap();
    let mut index = repo.index().unwrap();
    for (rel, _) in files {
        index.add_path(Path::new(rel)).unwrap();
    }
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let git_tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("test", "test@example.com").unwrap();
    let commit = repo
        .commit(Some("HEAD"), &sig, &sig, "init", &git_tree, &[])
        .unwrap();
    (dir, commit.to_string())
}

/// 索引ルートが 2 本ある Go のツリー。`.conductor/` も掘っておく。
fn nested_go_tree() -> tempfile::TempDir {
    let (dir, _) = repo_with(&[
        ("go.mod", "module demo\n"),
        ("main.go", "package main\n"),
        ("services/api/go.mod", "module demo/api\n"),
        ("services/api/api.go", "package api\n"),
    ]);
    std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();
    dir
}

/// このツリーに対するそのルートの鍵。[`survey`] と同じ出し方をしないと、置いた索引が
/// 見つからない。
fn key_in(tree_root: &Path, root: &IndexRoot) -> String {
    root.content_key(tree_root, &roots::discover(tree_root))
}

/// 鍵は置き場所の親のツリーから出す。実際の内容の鍵で置かないと、読む側が探す名前と食い違う。
fn artifact(dir: &Path, ext: &str) -> PathBuf {
    let tree_root = dir.parent().expect(".conductor の親がツリー");
    let key = key_in(tree_root, &at("", Language::Rust));
    dir.join(format!("index.rust.{key}.{ext}"))
}

/// `Store` はシンボル文字列だけで定義を引くので、複数ファイルを 1 つの索引に入れるときは
/// 同じ SYMBOL を使い回せない (別ファイルの定義まで拾う)。
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

/// 書式は sheaf の持ち物なので `write_provenance` に書かせる。手で綴ると、読み書きの
/// どちらかが変わったときに表が黙って読まれなくなる。
fn write_hashes(path: &Path, entries: &[(&str, String)]) {
    let expected = entries
        .iter()
        .map(|(rel, hash)| (PathBuf::from(rel), hash.clone()))
        .collect();
    sheaf_core::write_provenance(path, &*producer(), &expected).unwrap();
}

/// 「生成時点でディスクにあった内容」を申告する体で、コミット済みかは問わない。
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

/// 背景の調査を済ませた `SemanticIndex`。note_open はこれが無いと `Loading` のまま
/// 何もしない。
fn surveyed(tree_root: &Path, reading: Option<&str>) -> SemanticIndex {
    let mut semantic = SemanticIndex::default();
    let conductor = tree_root.join(".conductor");
    semantic.install(
        survey(tree_root, Some(&conductor), reading.map(Path::new), &[]),
        tree_root,
    );
    semantic
}

/// 索引を投入済みの `SemanticIndex`。
fn loaded(repo_root: &Path) -> SemanticIndex {
    let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");
    let mut semantic = surveyed(repo_root, None);
    assert!(semantic.accept(repo_root, repo_root, Some(store)));
    semantic
}

/// 生成を待っている索引ルートの位置。
fn pending_roots(semantic: &SemanticIndex) -> Vec<PathBuf> {
    semantic
        .roots
        .iter()
        .filter(|r| r.regenerator.is_pending())
        .map(|r| r.at.subroot.clone())
        .collect()
}

/// 置き場所に残っている Rust の索引の本数。
fn generation_count(conductor_dir: &Path) -> usize {
    std::fs::read_dir(conductor_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("index.rust.") && name.ends_with(".scip")
        })
        .count()
}

/// そのファイルを読んでいる体の Bridge を組んで、`rel` の `line`:`col` の定義を引く。
fn definition_at(store: &Store, tree_root: &Path, rel: &str, line: u32, col: u32) -> Definition {
    let abs = tree_root.join(rel);
    let source = std::fs::read_to_string(&abs).unwrap();
    let mask = CodeMask::compute(&source, rel);
    let index = SymbolIndex::new(tree_root.to_path_buf());
    let bridge = Bridge {
        abs_path: &abs,
        source: &source,
        mask: &mask,
        index: &index,
    };
    sheaf_core::definition_at(store, &bridge, Path::new(rel), line, col)
}

/// `fn caller() { greet(); }` の greet の位置を、索引に向けて引く。
fn definition_of_greet_at(store: &Store, tree_root: &Path, rel: &str) -> Definition {
    definition_at(store, tree_root, rel, 1, 14)
}

fn definition_of_greet(store: &Store, tree_root: &Path) -> Definition {
    definition_of_greet_at(store, tree_root, "src/lib.rs")
}

/// `needle` を含む行と、その行の中での `needle` の位置。
fn site(source: &str, needle: &str) -> (u32, u32) {
    let (line, text) = source
        .lines()
        .enumerate()
        .find(|(_, t)| t.contains(needle))
        .unwrap_or_else(|| panic!("{needle} を含む行が無い"));
    (line as u32, text.find(needle).unwrap() as u32)
}

// ---------------------------------------------------------------- 索引ルートの列挙

#[test]
fn 目印のあるディレクトリだけを索引ルートにする() {
    // 目印の無いツリーが空でないと、Go だけのリポジトリで rust-analyzer が起動する。
    // 認識できない対象には終了コード 0 で空の索引を書くので、起こさないこと自体が答え。
    struct Case {
        why: &'static str,
        files: &'static [(&'static str, &'static str)],
        expected: Vec<IndexRoot>,
    }
    let case = |why, files, expected| Case {
        why,
        files,
        expected,
    };

    let cases = [
        case(
            "目印のある言語だけ",
            &[("go.mod", "module demo\n")],
            vec![at("", Language::Go)],
        ),
        case(
            "目印が無ければ対象外",
            &[("main.go", "package main\n")],
            vec![],
        ),
        case(
            "go.mod はモジュールの境界なので入れ子も索引ルート",
            &[
                ("go.mod", "module demo\n"),
                ("services/api/go.mod", "module demo/api\n"),
            ],
            vec![at("", Language::Go), at("services/api", Language::Go)],
        ),
        case(
            "入れ子の Cargo.toml は workspace member なので索引ルートにしない",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]\n"),
                ("crates/a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ],
            vec![at("", Language::Rust)],
        ),
        case(
            "依存を抱えたディレクトリの下は歩かない",
            &[
                ("tsconfig.json", "{}\n"),
                ("node_modules/pkg/tsconfig.json", "{}\n"),
            ],
            vec![at("", Language::TypeScript)],
        ),
        case(
            "gitignore された目印は索引ルートにしない",
            &[
                (".gitignore", "build/\n"),
                ("go.mod", "module demo\n"),
                ("build/gen/go.mod", "module demo/gen\n"),
            ],
            vec![at("", Language::Go)],
        ),
    ];

    for Case {
        why,
        files,
        expected,
    } in cases
    {
        let dir = tree(files);
        assert_eq!(roots::discover(dir.path()), expected, "{why}");
    }
}

#[test]
fn 拡張子と目印から言語を引く() {
    let cases = [
        ("a/b.go", Some(Language::Go)),
        ("a/lib.rs", Some(Language::Rust)),
        ("a/page.tsx", Some(Language::TypeScript)),
        ("a/tsconfig.json", Some(Language::TypeScript)),
        ("Cargo.toml", Some(Language::Rust)),
        ("go.mod", Some(Language::Go)),
        // 索引に載らないファイルの変更で producer を起こさない。
        ("README.md", None),
    ];
    for (path, expected) in cases {
        assert_eq!(Language::of_file(Path::new(path)), expected, "{path}");
    }
}

#[test]
fn 言語ごとに違う道具を起動する() {
    // ここが 1 つに潰れると、Go や TypeScript のツリーに rust-analyzer が向く。認識できない
    // 対象には終了コード 0 で空の索引を書くので、失敗に見えない。
    let argv = |lang: Language| lang.producer().command(Path::new("/o"));

    assert_eq!(argv(Language::Rust)[0], "rust-analyzer");
    assert_eq!(argv(Language::Go)[0], "scip-go");
    // scip-typescript は npx 越しに版を固定して起動する。
    assert_eq!(argv(Language::TypeScript)[0], "npx");
    assert!(
        argv(Language::TypeScript)
            .iter()
            .any(|a| a.starts_with("@sourcegraph/scip-typescript@"))
    );
}

#[test]
fn 成果物の名前は索引ルートごとに分かれる() {
    const KEY: &str = "0123456789ab";
    let (dir, tree_root) = (Path::new("/artifacts"), Path::new("/tree"));
    let index = |root: IndexRoot| root.target(dir, tree_root, KEY).index;

    assert_eq!(
        index(at("", Language::Rust)),
        Path::new("/artifacts/index.rust.0123456789ab.scip")
    );
    assert_eq!(
        index(at("services/api", Language::Go)),
        Path::new("/artifacts/index.go.services_api.0123456789ab.scip")
    );
    // `a/b` と `a_b` が同じ名前に落ちると、2 本の索引が同じファイルを取り合う。
    assert_ne!(
        index(at("a/b", Language::Go)),
        index(at("a_b", Language::Go))
    );
    // 生成 1 本のピークが 2.3GiB なので、上限はリポジトリ単位で効かせる。
    assert_eq!(
        at("", Language::Rust).target(dir, tree_root, KEY).lock,
        at("services/api", Language::Go)
            .target(dir, tree_root, KEY)
            .lock
    );
}

#[test]
fn 鍵はそのルートのその言語のファイルだけで決まる() {
    // 全部を畳むと、画像を差し替えただけで名前が変わり、中身の同じ索引を作り直す。内側の
    // ルートを数えるのも同じで、そこの編集で外側の 2.3GiB を払うことになる。
    let files = [
        ("go.mod", "module demo\n"),
        ("main.go", "package main\n"),
        ("services/api/go.mod", "module demo/api\n"),
        ("services/api/api.go", "package api\n"),
    ];
    let key_of = |dir: &tempfile::TempDir| key_in(dir.path(), &at("", Language::Go));
    let base = key_of(&tree(&files));

    let mut with_asset = files.to_vec();
    with_asset.push(("docs/logo.svg", "<svg/>\n"));
    assert_eq!(
        key_of(&tree(&with_asset)),
        base,
        "索引に載らないファイルで動いた"
    );

    let mut inner_edited = files.to_vec();
    inner_edited[3] = ("services/api/api.go", "package api\n\nfunc F() {}\n");
    assert_eq!(
        key_of(&tree(&inner_edited)),
        base,
        "内側のルートの編集で動いた"
    );

    let mut own_edited = files.to_vec();
    own_edited[1] = ("main.go", "package main\n\nfunc main() {}\n");
    assert_ne!(
        key_of(&tree(&own_edited)),
        base,
        "自分のソースが動いたのに同じ"
    );
}

#[test]
#[ignore = "実リポジトリが要る"]
fn 実リポジトリでの列挙にかかる時間() {
    // 列挙はイベントループの中で走る。ツリーを歩くコストがフレームの予算 (16ms) を大きく
    // 超えるなら、置き場所を変える必要がある。
    let root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
    let started = Instant::now();
    let found = roots::discover(Path::new(&root));
    println!("{} ルート / {:?}", found.len(), started.elapsed());
    for r in &found {
        println!("  {:?} {}", r.lang, r.subroot.display());
    }
}

// ---------------------------------------------------------------- 索引の読み込み

#[test]
fn 索引と出自の表が揃わなければ読まない() {
    // 壊れた索引を置くと、出自を見ずに失敗しても None になり、検査したいことを検査できない。
    // 本物の SCIP を置いて、欠けているのが出自の表だけの状態を作る。
    let cases: [(&str, &[&str]); 3] = [
        ("どちらも無い", &[]),
        ("索引が無い", &["hashes"]),
        ("出自の表が無い", &["scip"]),
    ];
    for (why, present) in cases {
        let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        for ext in present {
            match *ext {
                "scip" => write_index(&artifact(&conductor_dir, "scip")),
                _ => std::fs::write(artifact(&conductor_dir, "hashes"), "").unwrap(),
            }
        }
        assert!(load(dir.path(), dir.path()).is_none(), "{why}");
    }
}

#[test]
fn 索引ルートが複数あればすべて畳んで読む() {
    // 1 世代が作るのは 1 ルートぶん。それをそのまま投入すると、他のルートの索引が黙って
    // 落ちて、そこは以後ずっと構文層で答えることになる。
    let dir = nested_go_tree();
    let conductor_dir = dir.path().join(".conductor");
    for (subroot, docs) in [("", ["main.go"]), ("services/api", ["api.go"])] {
        let root = at(subroot, Language::Go);
        let target = root.target(&conductor_dir, dir.path(), &key_in(dir.path(), &root));
        write_index_for(&target.index, &docs);
        sheaf_core::write_provenance(&target.hashes, &*root.lang.producer(), &Default::default())
            .unwrap();
    }

    let store = load(dir.path(), dir.path()).expect("置いた索引を読めない");
    assert_eq!(store.len(), 2, "索引ルートのどちらかが落ちた");
}

#[test]
fn リンクされたworktreeはmain側の索引を見つける() {
    // リンクされた worktree の workdir() はリンク先自身を指すので、repo_root にそれを
    // そのまま渡すと main 側にしか無い .conductor/ が見つからない。
    let (dir, commit) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    place_index(dir.path());

    let repo = git2::Repository::open(dir.path()).unwrap();
    let head = repo
        .find_commit(git2::Oid::from_str(&commit).unwrap())
        .unwrap();
    repo.branch("wt-branch", &head, false).unwrap();
    let reference = repo.find_reference("refs/heads/wt-branch").unwrap();
    let parent = tempfile::tempdir().unwrap();
    let wt_path = parent.path().join("linked-wt");
    repo.worktree(
        "linked-wt",
        &wt_path,
        Some(git2::WorktreeAddOptions::new().reference(Some(&reference))),
    )
    .unwrap();

    let store = load(&wt_path, &wt_path).expect("main 側の索引が見つかるはず");
    assert!(matches!(
        definition_of_greet(&store, &wt_path),
        Definition::Exact(_)
    ));
}

#[test]
fn 別ツリーは内容が一致したときだけ答える() {
    // worktree の形。内容が同じファイルは索引を使い回せる。これが成り立たないと worktree
    // ごとに索引を作ることになり、この設計の意味が無くなる。逆に編集されていれば、索引の
    // 言う 0 行目はもう greet の定義ではないので、確信度つきで答えてはいけない。
    //
    // 聞く行 (1 行目) は両方で同じにしてある。ここを動かすと問い合わせ位置が別の語にずれて
    // 「語が無い」で落ちるだけになり、鮮度を検査しないまま緑になる。
    for (why, content, exact) in [
        ("内容が同じ", SOURCE, true),
        (
            "編集されている",
            "pub fn hello() {}\nfn caller() { greet(); }\n",
            false,
        ),
    ] {
        let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());
        let other = tree(&[CARGO_TOML, ("src/lib.rs", content)]);

        let store = load(dir.path(), other.path()).expect("索引と出自の申告が揃っている");
        assert_eq!(
            matches!(
                definition_of_greet(&store, other.path()),
                Definition::Exact(_)
            ),
            exact,
            "{why}"
        );
    }
}

#[test]
fn 出自の表が鮮度をファイル単位で決める() {
    // 生きたリポジトリの Exact 率は汚れ具合で変わって assert できないので、1 ファイルずつ
    // 用意して個別に見る。
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let untouched = "src/untouched.rs";
    let untracked = "src/untracked.rs";
    let edited = "src/edited.rs";
    // 生成の前後でハッシュが食い違ったファイル。index.hashes に載らない。
    let racy = "src/racy.rs";

    std::fs::write(dir.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
    for rel in [untouched, untracked, edited, racy] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, SOURCE).unwrap();
    }

    // untracked は git に足さないままにして、出自の申告が git のトラッキング状態と無関係に
    // 効くことを確かめる。
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(untouched)).unwrap();
    index.add_path(Path::new(edited)).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let git_tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &git_tree, &[])
        .unwrap();

    let conductor_dir = dir.path().join(".conductor");
    std::fs::create_dir_all(&conductor_dir).unwrap();
    write_index_for(
        &artifact(&conductor_dir, "scip"),
        &[untouched, untracked, edited, racy],
    );
    let hash = sheaf_core::blob_hash(SOURCE.as_bytes());
    write_hashes(
        &artifact(&conductor_dir, "hashes"),
        &[
            (untouched, hash.clone()),
            (untracked, hash.clone()),
            (edited, hash),
        ],
    );

    // edited は生成が終わった後に編集される。呼び出し箇所 (1 行目) は変えていないので、
    // クエリの単語自体は引き続き greet を指す。
    std::fs::write(
        dir.path().join(edited),
        "pub fn hello() {}\nfn caller() { greet(); }\n",
    )
    .unwrap();

    let store = load(dir.path(), dir.path()).expect("索引と出自の申告が揃っている");
    for (rel, exact, why) in [
        (untouched, true, "生成後に触っていない"),
        (untracked, true, "未追跡でも表に載っていれば答える"),
        (edited, false, "生成後に編集された"),
        (racy, false, "表に載っていない"),
    ] {
        assert_eq!(
            matches!(
                definition_of_greet_at(&store, dir.path(), rel),
                Definition::Exact(_)
            ),
            exact,
            "{why}"
        );
    }
}

#[test]
fn 出自の表は綴りも値もそのまま往復する() {
    // 表の鍵は SCIP の relative_path と突き合わせられる。綴りがずれると一致するファイルが
    // 1 つも無くなり、全部が構文層に落ちる。誤答にはならないので気づけず、テストも緑のまま。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.hashes");
    let hash = sheaf_core::blob_hash(b"fn f() {}\n");
    write_hashes(
        &path,
        &[
            ("src/deep/nested/lib.rs", hash.clone()),
            ("top.rs", "1".repeat(40)),
        ],
    );

    let table = sheaf_core::read_provenance(&path, &*producer()).unwrap();

    let mut keys: Vec<_> = table.keys().map(|p| p.to_string_lossy()).collect();
    keys.sort();
    assert_eq!(keys, ["src/deep/nested/lib.rs", "top.rs"]);
    assert_eq!(table.get(Path::new("src/deep/nested/lib.rs")), Some(&hash));
}

// ---------------------------------------------------------------- 生成の起こし方

/// 引数を無視してひたすら sleep するだけの producer。生成が走っている最中を安定して作る。
struct SlowProducer(PathBuf);

impl Producer for SlowProducer {
    fn command(&self, _out: &Path) -> Vec<String> {
        vec![self.0.to_string_lossy().into_owned()]
    }
}

#[test]
fn tick_regenerationは生成が走っていても即座に返る() {
    // 「バックグラウンドでやっている」ことの回帰ガード。索引の読み込みや生成の待ち合わせが
    // ここに紛れ込むと、呼び出し元 (イベントループ) を止める。
    const KEY: &str = "0123456789ab";
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
    std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();

    let script = dir.path().join("slow-producer.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    // ツリーを既に引いてあることにしないと、調査の取り込みが rust-analyzer の Regenerator に
    // 差し替えてしまう。
    let mut semantic = SemanticIndex::with_root(
        dir.path(),
        at("", Language::Rust),
        sheaf_core::Regenerator::new(Arc::new(SlowProducer(script))),
        KEY,
    );
    semantic.note_change(&dir.path().join("src/lib.rs"), dir.path());
    // 編集で鍵が落ちる。鍵の無いルートは生成を始めないので、調査が届いた状態にしておく。
    semantic.roots[0].key = Some(KEY.to_string());

    // 静穏時間が経つのを待って、生成が実際に走っている状態を作る。
    let deadline = Instant::now() + Duration::from_secs(10);
    while semantic.is_pending() {
        assert!(
            semantic.tick_regeneration(dir.path(), dir.path()).is_none(),
            "30 秒 sleep する producer がもう終わっているはずがない"
        );
        assert!(Instant::now() < deadline, "生成が始まらない");
        std::thread::sleep(Duration::from_millis(100));
    }

    let started = Instant::now();
    let outcome = semantic.tick_regeneration(dir.path(), dir.path());
    let elapsed = started.elapsed();

    assert!(outcome.is_none());
    assert!(
        elapsed < Duration::from_millis(200),
        "tick が生成を待ってしまっている: {elapsed:?}"
    );

    semantic.abort_regeneration(dir.path());
}

#[test]
fn 索引ルートの無いツリーでは生成を起こさない() {
    // どの目印も無いツリーに道具を向けても意味が無い。認識できない対象に対して終了コード 0 で
    // 空の索引を書くことがあるので、起こさないこと自体が答え。
    let (dir, _) = repo_with(&[("main.go", "package main\n")]);
    let mut semantic = SemanticIndex::default();
    semantic.note_change(&dir.path().join("main.go"), dir.path());

    assert!(
        semantic.tick_regeneration(dir.path(), dir.path()).is_none(),
        "目印が無いのに生成が始まった"
    );
}

#[test]
fn goのツリーにはscip_goを向ける() {
    // ここが Rust 決め打ちだと、Go のリポジトリは索引が 1 本も無いまま tree-sitter の名前
    // 一致に落ち続ける。画面には出ないので気づけない。
    let (dir, _) = repo_with(&[("go.mod", "module demo\n"), ("main.go", "package main\n")]);

    let mut semantic = surveyed(dir.path(), Some("main.go"));
    semantic.note_change(&dir.path().join("main.go"), dir.path());

    assert!(semantic.is_pending(), "go.mod があるのに生成を待っていない");
    assert_eq!(
        semantic.roots[0]
            .at
            .lang
            .producer()
            .command(Path::new("/o"))[0],
        "scip-go"
    );
}

#[test]
fn 読んでいるファイルの索引ルートだけに索引を作らせる() {
    // 実在するリポジトリで索引ルートは 109 本になる。まとめて作ると数十分。
    let dir = nested_go_tree();
    let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));

    semantic.note_open(Path::new("services/api/api.go"), dir.path(), dir.path());

    assert_eq!(
        pending_roots(&semantic),
        vec![PathBuf::from("services/api")]
    );
}

#[test]
fn 入れ子のルートの編集は外側の索引を起こさない() {
    // go.mod はモジュールの境界なので、外側の索引に内側のパッケージは入らない。起こすと、
    // 変わっていない索引を作り直すだけになる。
    let dir = nested_go_tree();
    let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));

    semantic.note_change(&dir.path().join("services/api/api.go"), dir.path());

    assert_eq!(
        pending_roots(&semantic),
        vec![PathBuf::from("services/api")]
    );
}

#[test]
fn 索引に載らないファイルの変更では作り直さない() {
    let dir = nested_go_tree();
    let mut semantic = surveyed(dir.path(), None);

    semantic.note_change(&dir.path().join("README.md"), dir.path());

    assert!(pending_roots(&semantic).is_empty());
}

#[test]
fn gitignoreされた変更は作り直しを起こさない() {
    // 索引ルートの中にあるので owning_root は当たるが、producer は読まない。
    // 通すと target/ の書き込みが静穏タイマーを永久に押し戻す。
    let dir = nested_go_tree();
    std::fs::write(dir.path().join(".gitignore"), "/build\n").unwrap();
    std::fs::create_dir_all(dir.path().join("build")).unwrap();
    std::fs::write(dir.path().join("build/gen.go"), "package build\n").unwrap();
    let mut semantic = surveyed(dir.path(), Some("main.go"));

    semantic.note_change(&dir.path().join("build/gen.go"), dir.path());
    assert!(pending_roots(&semantic).is_empty());

    semantic.note_change(&dir.path().join("main.go"), dir.path());
    assert_eq!(pending_roots(&semantic), [PathBuf::from("")]);
}

#[test]
fn 索引が既にあるルートには作り直しを頼まない() {
    // 開くたびに頼むと、大きなリポジトリでは producer が止まらなくなる。
    let dir = nested_go_tree();
    let root = at("services/api", Language::Go);
    let conductor_dir = dir.path().join(".conductor");
    let target = root.target(&conductor_dir, dir.path(), &key_in(dir.path(), &root));
    write_index_for(&target.index, &["api.go"]);
    sheaf_core::write_provenance(&target.hashes, &*root.lang.producer(), &Default::default())
        .unwrap();

    let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));
    semantic.note_open(Path::new("services/api/api.go"), dir.path(), dir.path());

    assert!(
        pending_roots(&semantic).is_empty(),
        "索引があるのに作り直しを頼んだ"
    );
}

#[test]
fn 手で頼まれたら読んでいるルートを作り直す() {
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    place_index(dir.path());
    let mut semantic = loaded(dir.path());
    semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path());

    assert!(semantic.rebuild_reading());
    assert!(!pending_roots(&semantic).is_empty());
}

#[test]
fn 読んでいるファイルが無ければ手の作り直しは断る() {
    assert!(!SemanticIndex::default().rebuild_reading());
}

#[test]
fn 世代は上限まで残して古いものから落とす() {
    // 残す意味があるのは行き来する worktree のぶんだけ。際限なく残すと、二度と一致しない
    // 索引がディスクを食う。
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let conductor_dir = dir.path().join(".conductor");
    let lib = dir.path().join("src/lib.rs");
    for n in 0..6 {
        std::fs::write(&lib, format!("pub fn greet() {{}}\n// {n}\n")).unwrap();
        place_index(dir.path());
    }
    assert_eq!(generation_count(&conductor_dir), 6);

    at("", Language::Rust).prune(&conductor_dir);
    assert_eq!(generation_count(&conductor_dir), 4);
}

#[test]
fn 一度作った内容の索引は戻ってきても作り直さない() {
    // 索引を 1 本しか持たないと、内容の違う worktree を行き来するたびに上書きし合い、戻る
    // たびに 14 秒 / 2.3GiB を払うことになる。
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let conductor_dir = dir.path().join(".conductor");
    let lib = dir.path().join("src/lib.rs");

    place_index(dir.path());
    std::fs::write(&lib, "pub fn greet() {}\nfn other() { greet(); }\n").unwrap();
    place_index(dir.path());
    assert_eq!(
        generation_count(&conductor_dir),
        2,
        "内容が違うのに同じ名前で上書きしている"
    );

    std::fs::write(&lib, SOURCE).unwrap();
    let mut semantic = surveyed(dir.path(), Some("src/lib.rs"));
    let store = load(dir.path(), dir.path()).expect("戻った内容の索引を読めない");
    assert!(semantic.accept(dir.path(), dir.path(), Some(store)));

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Indexed
    );
    assert!(
        !semantic.roots.iter().any(|r| r.is_working()),
        "前に作った内容に戻っただけなのに作り直した"
    );
}

// ---------------------------------------------------------------- 読んでいるファイルの答え

#[test]
fn 索引が今の内容を説明できているときは何も言わない() {
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    place_index(dir.path());
    let mut semantic = loaded(dir.path());

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Indexed
    );
}

#[test]
fn 内容の変わったツリーを読んだら作りに行く() {
    // 索引は内容ごとに名前が分かれるので、内容が動けばその内容の索引はまだ無い。待つのでは
    // なく作りに行かないと、worktree を移るたびに構文層のまま据え置かれる。
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    place_index(dir.path());
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
    let mut semantic = loaded(dir.path());
    semantic.install(
        survey(
            dir.path(),
            Some(&dir.path().join(".conductor")),
            Some(Path::new("src/lib.rs")),
            &[],
        ),
        dir.path(),
    );

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Building
    );
    assert!(
        semantic.roots.iter().any(|r| r.is_working()),
        "いまの内容の索引が無いのに作りに行っていない"
    );
}

#[test]
fn 読んでいるファイルが説明できているなら内容が動いても作りに行かない() {
    // 起動のたびに producer を起こさないための門。出自はファイル単位なので、ほかのファイルが
    // 動いて鍵がずれても、読んでいるファイルは前の世代のまま Exact に答えられる。鍵だけを
    // 見て作りに行くと、git がツリーを動かすたびに 14 秒を払うことになる。
    let (dir, _) = repo_with(&[
        CARGO_TOML,
        ("src/lib.rs", SOURCE),
        ("src/other.rs", "pub fn other() {}\n"),
    ]);
    place_index(dir.path());
    let mut semantic = loaded(dir.path());
    std::fs::write(dir.path().join("src/other.rs"), "pub fn moved() {}\n").unwrap();
    semantic.install(
        survey(
            dir.path(),
            Some(&dir.path().join(".conductor")),
            Some(Path::new("src/lib.rs")),
            &[],
        ),
        dir.path(),
    );

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Indexed
    );
    assert!(
        !semantic.roots.iter().any(|r| r.is_working()),
        "読めているファイルのために producer を起こしている"
    );
}

#[test]
fn 索引が説明できないファイルを読んでいることを伝える() {
    // 索引はいまの内容のものなのに、このファイルだけ載っていない。作り直しても同じものが
    // 出るので、黙って構文層に落ちる。言わないと「ジャンプが甘い」としか見えない。
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let conductor_dir = dir.path().join(".conductor");
    std::fs::create_dir_all(&conductor_dir).unwrap();
    write_index(&artifact(&conductor_dir, "scip"));
    // 出自を 1 件も申告しない索引。鍵はツリーから出るので一致したままになる。
    write_hashes(&artifact(&conductor_dir, "hashes"), &[]);
    let mut semantic = loaded(dir.path());

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Stale
    );
    assert!(!semantic.roots.iter().any(|r| r.is_working()));
}

#[test]
fn 索引をまだ読み込めていないうちは答えを確定させない() {
    // 索引の読み込みは別スレッドで、worktree 切替の直後は間に合っていない。ここで確定させると、
    // 古いことを言うべき唯一の場面で必ず黙る。
    let (dir, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let conductor_dir = dir.path().join(".conductor");
    std::fs::create_dir_all(&conductor_dir).unwrap();
    write_index(&artifact(&conductor_dir, "scip"));
    write_hashes(&artifact(&conductor_dir, "hashes"), &[]);
    let mut semantic = surveyed(dir.path(), None);

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Loading
    );

    let store = load(dir.path(), dir.path()).unwrap();
    assert!(semantic.accept(dir.path(), dir.path(), Some(store)));
    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
        Reading::Stale,
        "読み込み待ちの周で答えを確定させてしまっている"
    );
}

#[test]
fn 調査に載っていないルートのファイルを読んだら調査をやり直す() {
    // 調査が鍵を出すのは成果物の置いてあるルートだけなので、まだ一度も索引されていない
    // ルートは列挙に載らない。やり直させないと、そのルートは鍵を持てないまま生成も始まらず、
    // ホバーが永久に構文層に落ちる。
    let dir = nested_go_tree();
    let mut semantic = surveyed(dir.path(), None);
    let rel = Path::new("services/api/api.go");

    assert_eq!(
        semantic.note_open(rel, dir.path(), dir.path()),
        Reading::NotIndexed
    );
    assert!(
        semantic.needs_survey(dir.path()).is_some(),
        "索引ルートの分からないファイルを読んだまま調査を頼んでいない"
    );

    semantic.install(
        survey(
            dir.path(),
            Some(&dir.path().join(".conductor")),
            Some(rel),
            &[],
        ),
        dir.path(),
    );
    assert_eq!(
        semantic.note_open(rel, dir.path(), dir.path()),
        Reading::Building
    );
}

#[test]
fn 鍵を失ったルートは名指しで調べ直される() {
    // 調査は鍵を出す相手を自分で選ぶ (109 本ぶんの鍵は 0.6 秒かかる)。選から漏れたルートは
    // 「調査が要る」と言い続け、背景の調査が毎フレーム走ることになる。
    let dir = nested_go_tree();
    let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));
    semantic.note_change(&dir.path().join("services/api/api.go"), dir.path());

    let wanted = semantic
        .needs_survey(dir.path())
        .expect("鍵が無いのに調べ直しを求めていない");
    assert!(!wanted.is_empty(), "鍵の要るルートを名指ししていない");

    // 読んでいるファイルを渡さなくても、名指しなら鍵が付く。
    semantic.install(survey(dir.path(), None, None, &wanted), dir.path());
    assert!(
        semantic.needs_survey(dir.path()).is_none(),
        "調べ直したのに鍵が付いていない"
    );
}

#[test]
fn 同じパスのまま別のツリーへ移ったら調査が届くまで答えない() {
    // 「前回と同じファイル」の早期リターンがツリーの照合より前にあると、worktree を移った
    // あとも前のツリーの索引ルートで答え続ける。誤りに見えない誤答になる。
    let (rust, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let (plain, _) = repo_with(&[("src/lib.rs", SOURCE)]);
    let mut semantic = surveyed(rust.path(), Some("src/lib.rs"));

    semantic.note_open(Path::new("src/lib.rs"), rust.path(), rust.path());
    assert_eq!(semantic.roots.len(), 1);

    assert_eq!(
        semantic.note_open(Path::new("src/lib.rs"), plain.path(), plain.path()),
        Reading::Loading,
        "別のツリーなのに前のツリーの索引ルートで答えた"
    );
    assert_eq!(
        semantic.needs_survey(plain.path()),
        Some(Vec::new()),
        "ツリーが変わったのに調べ直しを求めていない"
    );

    semantic.install(
        survey(plain.path(), None, Some(Path::new("src/lib.rs")), &[]),
        plain.path(),
    );
    assert!(
        semantic.roots.is_empty(),
        "目印の無いツリーに索引ルートがある"
    );
}

#[test]
fn リポジトリを切り替えたら成果物の置き場所も引き直す() {
    // 成果物の名前はリポジトリをまたいで同じ (index.rust.scip) なので、置き場所を引き直さ
    // ないと切り替え先の索引を切り替え元へ書き込み、相手の索引を上書きする。
    let (first, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let (second, _) = repo_with(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
    let mut semantic = SemanticIndex::default();

    // macOS の /var は /private/var への symlink で、git2 は解決した側を返す。
    let expected = |repo: &Path| repo.canonicalize().unwrap().join(".conductor");

    semantic.note_open(Path::new("src/lib.rs"), first.path(), first.path());
    assert_eq!(
        semantic.conductor_dir(first.path()),
        Some(expected(first.path()).as_path())
    );

    semantic.note_open(Path::new("src/lib.rs"), second.path(), second.path());
    assert_eq!(
        semantic.conductor_dir(second.path()),
        Some(expected(second.path()).as_path()),
        "切り替え元の .conductor を指したまま"
    );
}

// ---------------------------------------------------------------- 実際に索引を作る

/// conductor が Go のツリーに scip-go を向け、その索引で `Exact` に答えるまでを一続きで
/// 見る。読み取り側は sheaf のテストが見ているので、ここで見たいのは host 側の配線だけ。
/// scip-go が無ければ飛ばさずに落とす — 飛ばすと配線が壊れていても緑になる。
#[test]
fn goのツリーを索引して定義に飛べる() {
    let (dir, _) = repo_with(&[
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

    // 生成 1 件につき 1 行。あとから「いつ・どこを・どれだけかけて」を追える。
    let log = std::fs::read_to_string(root.join(".conductor/index-history.log")).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("trigger=cli"), "{log}");
    assert!(log.contains("result=ok documents=2"), "{log}");

    let store = load(root, root).expect("置いた索引を読めない");
    let source = std::fs::read_to_string(root.join("main.go")).unwrap();
    let (line, col) = site(&source, "greet.Greet()");

    assert_eq!(
        definition_at(&store, root, "main.go", line, col + "greet.".len() as u32),
        Definition::Exact(vec![Location {
            path: PathBuf::from("pkg/greet/greet.go"),
            line: 2,
            col: 5,
        }])
    );
}

/// 入れ子の索引ルートで作った索引が、ツリーのルートから見た正しいパスに飛ぶ。索引の中の
/// 綴りは索引ルート相対 (`handler/handler.go`) なので、接ぎ木を誤ると存在しないパスへ飛ぶ。
#[test]
fn 入れ子の索引ルートの中で定義に飛べる() {
    let (dir, _) = repo_with(&[
        ("go.mod", "module example.com/app\n\ngo 1.21\n"),
        ("main.go", "package main\n\nfunc main() {}\n"),
        ("services/api/go.mod", "module example.com/api\n\ngo 1.21\n"),
        (
            "services/api/handler/handler.go",
            "package handler\n\nfunc Handle() string {\n\treturn \"ok\"\n}\n",
        ),
        (
            "services/api/main.go",
            "package main\n\nimport \"example.com/api/handler\"\n\nfunc main() {\n\tprintln(handler.Handle())\n}\n",
        ),
    ]);
    let root = dir.path();
    build_index(root).expect("2 本の索引ルートを索引できない");

    let store = load(root, root).expect("置いた索引を読めない");
    let rel = "services/api/main.go";
    let source = std::fs::read_to_string(root.join(rel)).unwrap();
    let (line, col) = site(&source, "handler.Handle()");

    assert_eq!(
        definition_at(&store, root, rel, line, col + "handler.".len() as u32),
        Definition::Exact(vec![Location {
            path: PathBuf::from("services/api/handler/handler.go"),
            line: 2,
            col: 5,
        }])
    );
}

// ---------------------------------------------------------------- 記録

fn log_entry<'a>(outcome: Outcome<'a>, sources: Sources) -> history::Entry<'a> {
    history::Entry {
        root: Path::new("services/api"),
        lang: "go",
        trigger: Trigger::Change,
        cause: Some(Path::new("services/api/handler/handler.go")),
        waited: Duration::from_millis(3_100),
        took: Duration::from_millis(1_800),
        outcome,
        sources,
        changed_during: 0,
    }
}

fn written(dir: &tempfile::TempDir) -> String {
    std::fs::read_to_string(dir.path().join("index-history.log")).unwrap()
}

#[test]
fn 生成の顛末を1行に書く() {
    /// 既定の Entry を書き換えて、書かれた行に何が出る / 出ないかを言う。
    struct Case {
        why: &'static str,
        mutate: Box<dyn Fn(&mut history::Entry<'_>)>,
        must: &'static [&'static str],
        must_not: &'static [&'static str],
    }
    let case = |why, mutate, must, must_not| Case {
        why,
        mutate,
        must,
        must_not,
    };

    let unchanged = Sources::Delta(SourceDelta::default());
    let cases = [
        case(
            "何をきっかけにどこを作ったか",
            Box::new(|_| {}),
            &[
                "lang=go root=services/api",
                "trigger=change",
                "cause=services/api/handler/handler.go",
                "waited=3.1s took=1.8s",
                "result=ok documents=42",
                "sources=+0~1-0",
            ],
            &["waste="],
        ),
        case(
            "ソースが動いていない生成は無駄として残す",
            Box::new(move |e| e.sources = unchanged),
            &["sources=none", "waste=no-source-change"],
            &[],
        ),
        case(
            "初回は比べる相手がいないので無駄と言わない",
            Box::new(|e| e.sources = Sources::First),
            &["sources=first"],
            &["waste="],
        ),
        case(
            "無駄の理由は並べて書く",
            Box::new(move |e| {
                e.sources = unchanged;
                e.changed_during = 2;
            }),
            &["waste=no-source-change,stale-on-arrival(2 files)"],
            &[],
        ),
        case(
            // worktree を切り替えると、そこまでの producer の時間は捨てられる。
            "捨てた生成も無駄として残す",
            Box::new(|e| {
                e.outcome = Outcome::Aborted;
                e.sources = Sources::Unknown;
            }),
            &["result=aborted", "waste=discarded"],
            &["sources="],
        ),
        case(
            // 1 件 1 行という読み方も、key=value という読み方も崩さない。
            "失敗の理由は 1 行に畳んで引用する",
            Box::new(|e| {
                e.outcome = Outcome::Failed("落ちた\n詳細は log");
                e.sources = Sources::Unknown;
            }),
            &[r#"result=failed reason="落ちた 詳細は log""#],
            &[],
        ),
    ];

    for Case {
        why,
        mutate,
        must,
        must_not,
    } in cases
    {
        let dir = tempfile::tempdir().unwrap();
        let mut entry = log_entry(
            Outcome::Ready { documents: 42 },
            Sources::Delta(SourceDelta {
                modified: 1,
                ..Default::default()
            }),
        );
        mutate(&mut entry);
        history::append(dir.path(), &entry);

        let log = written(&dir);
        assert_eq!(log.lines().count(), 1, "{why}: {log}");
        for needle in must {
            assert!(log.contains(needle), "{why}: {needle} が無い: {log}");
        }
        for needle in must_not {
            assert!(!log.contains(needle), "{why}: {needle} を書いた: {log}");
        }
    }
}

#[test]
fn ツリーのルート自身は空ではなくドットで書く() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = log_entry(Outcome::Busy, Sources::Unknown);
    entry.root = Path::new("");
    entry.cause = None;
    history::append(dir.path(), &entry);

    let log = written(&dir);
    assert!(log.contains("root=. trigger=change waited="), "{log}");
}

#[test]
fn 追記されるので前の行が残る() {
    let dir = tempfile::tempdir().unwrap();
    history::append(dir.path(), &log_entry(Outcome::Busy, Sources::Unknown));
    history::append(dir.path(), &log_entry(Outcome::Aborted, Sources::Unknown));

    assert_eq!(written(&dir).lines().count(), 2);
}

#[test]
fn 時刻はutcのiso8601() {
    let at = history::stamp();
    assert_eq!(at.len(), 20, "{at}");
    assert!(at.ends_with('Z'), "{at}");
    // 桁位置がずれると、あとで並べ替えたときに壊れる。
    assert_eq!(&at[4..5], "-");
    assert_eq!(&at[10..11], "T");
}

#[test]
fn 暦の変換が既知の日付と合う() {
    assert_eq!(history::civil_from_days(0), (1970, 1, 1));
    assert_eq!(history::civil_from_days(19_723), (2024, 1, 1));
    // 閏日。ここを外すと 1 日ずれた記録が残る。
    assert_eq!(history::civil_from_days(19_783), (2024, 3, 1));
}

// ---------------------------------------------------------------- 構文層 (Bridge)

const BRIDGE_SOURCE: &str = "\
fn value() {}

struct Holder<'a> {
    value: &'a str,
}

fn caller() {
    let x = value();
    // value mentioned only in a comment
    let s = \"value\";
    let c = 'x';
}
";

/// BRIDGE_SOURCE を 1 ファイル書き出してビルドした索引と、対応する CodeMask。
fn bridge_fixture() -> (tempfile::TempDir, SymbolIndex, CodeMask) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), BRIDGE_SOURCE).unwrap();
    let index = SymbolIndex::new(dir.path().to_path_buf());
    index.build();
    (dir, index, CodeMask::compute(BRIDGE_SOURCE, "lib.rs"))
}

/// 語として答えたときに span が覆うテキスト。
enum Expect {
    Word(&'static str),
    NotWord,
    Unknown,
}

#[test]
fn 語として答えるのはコード上の識別子だけ() {
    use sheaf_core::{SyntacticLayer, Token};

    let (dir, index, mask) = bridge_fixture();
    let path = dir.path().join("lib.rs");
    let bridge = Bridge {
        abs_path: &path,
        source: BRIDGE_SOURCE,
        mask: &mask,
        index: &index,
    };

    let cases = [
        (
            "コード上の識別子",
            7,
            "value",
            0,
            true,
            Expect::Word("value"),
        ),
        ("コメントの中", 8, "value", 0, true, Expect::NotWord),
        ("文字列リテラルの中", 9, "value", 0, true, Expect::NotWord),
        // ライフタイムは識別子の前に ' が付くが、tree-sitter 側の語には含まれない。
        ("ライフタイム", 3, "'a", 1, true, Expect::Word("'a")),
        // char_literal は丸ごと NonCode になる。誤って範囲を広げても誤答にはならないが、
        // 広げないことをここで固定しておく。
        ("文字リテラル", 10, "'x'", 1, true, Expect::NotWord),
        ("別のファイル", 7, "value", 0, false, Expect::Unknown),
    ];

    for (why, line_idx, needle, offset, same_file, expect) in cases {
        let text = BRIDGE_SOURCE.lines().nth(line_idx).unwrap();
        let col = text.find(needle).unwrap() as u32 + offset;
        let asked = if same_file {
            path.clone()
        } else {
            dir.path().join("other.rs")
        };
        match (expect, bridge.token_at(&asked, line_idx as u32, col)) {
            (Expect::Word(spelled), Token::Word(span)) => assert_eq!(
                &text[span.start_col as usize..span.end_col as usize],
                spelled,
                "{why}"
            ),
            (Expect::NotWord, Token::NotWord) | (Expect::Unknown, Token::Unknown) => {}
            (_, got) => panic!("{why}: {got:?}"),
        }
    }
}

#[test]
fn シンボル行の1始まりを0始まりの位置に直す() {
    use sheaf_core::{SyntacticAnswer, SyntacticLayer};

    let (dir, index, mask) = bridge_fixture();
    let path = dir.path().join("lib.rs");
    let bridge = Bridge {
        abs_path: &path,
        source: BRIDGE_SOURCE,
        mask: &mask,
        index: &index,
    };
    let (line, col) = site(BRIDGE_SOURCE, "let x = value();");
    let col = col + "let x = ".len() as u32;

    let symbol = index
        .find_definitions("value", Path::new("lib.rs"))
        .into_iter()
        .next()
        .expect("value の定義が索引に無い");
    assert_eq!(
        symbol.line, 1,
        "fixture の前提: fn value() は 1 始まりで 1 行目"
    );

    match bridge.definition_at(&path, line, col) {
        SyntacticAnswer::Found(locations) => {
            assert_eq!(locations.len(), 1);
            assert_eq!(locations[0].line, symbol.line as u32 - 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 別の言語の同名の定義には落とさない() {
    use sheaf_core::{SyntacticAnswer, SyntacticLayer};

    // tree-sitter の索引は名前でしか引けないので、Go の rollbar が TypeScript の
    // const rollbar に当たりうる。
    let go = "package main\n\nfunc use() { rollbar.SetToken(\"x\") }\n";
    let dir = tree(&[
        ("main.go", go),
        ("page.tsx", "const rollbar = useRollbar();\n"),
    ]);
    let index = SymbolIndex::new(dir.path().to_path_buf());
    index.build();

    let path = dir.path().join("main.go");
    let mask = CodeMask::compute(go, "main.go");
    let bridge = Bridge {
        abs_path: &path,
        source: go,
        mask: &mask,
        index: &index,
    };
    let (line, col) = site(go, "rollbar");

    let SyntacticAnswer::Found(locations) = bridge.definition_at(&path, line, col) else {
        panic!("識別子として認識されていない");
    };
    assert!(
        locations
            .iter()
            .all(|l| l.path.extension().is_none_or(|e| e != "tsx")),
        "Go のファイルから TypeScript の定義に落ちた: {locations:?}"
    );
}

#[test]
fn 読めない種別は見出しごと出さない() {
    // 種別を読めなかったときにそれらしい名前を返すと、ホバーが自信を持って嘘を出す。
    use sheaf_core::SymbolKind;
    assert_eq!(kind_label(SymbolKind::Unknown), "");
    assert_eq!(kind_label(SymbolKind::Function), "fn");
    assert_eq!(kind_label(SymbolKind::Variable), "let");
}

// ------------------------------------------------ 実索引 (要 CONDUCTOR_TEST_REPO)

fn test_repo() -> PathBuf {
    PathBuf::from(
        std::env::var("CONDUCTOR_TEST_REPO")
            .expect("CONDUCTOR_TEST_REPO に .conductor/ へ索引を置いたリポジトリのパスを渡すこと"),
    )
}

/// 索引が実際に説明できている Rust ファイルを、ルートからの相対パスで最大 `n` 本。
///
/// ファイル名を直に書くと、リポジトリの構成が変わるたびに検査が意味を失う。
fn covered_rust_files(repo_root: &Path, store: &Store, n: usize) -> Vec<PathBuf> {
    let found: Vec<PathBuf> = ignore::WalkBuilder::new(repo_root)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(repo_root).ok().map(Path::to_path_buf))
        .filter(|rel| rel.extension().is_some_and(|x| x == "rs"))
        .filter(|rel| store.is_current(rel))
        .take(n)
        .collect();
    assert!(
        !found.is_empty(),
        "索引が説明できる Rust ファイルが 1 つも無い"
    );
    found
}

/// 実際に置かれている索引で、git2 のツリー走査と投入が通ることを見る。合成した索引では、
/// 実索引の Document 数やパスの綴りまでは検査できない。
#[test]
#[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
fn 実索引は生成元のリポジトリから読める() {
    let repo_root = test_repo();
    let store = load(&repo_root, &repo_root).expect("索引と出自の申告が揃っている");
    println!(
        "{} Document / ルート外 {} / 保持 {:.1}MB",
        store.len(),
        store.outside_root(),
        store.retained_bytes() as f64 / 1048576.0,
    );
    assert!(!store.is_empty());
    assert_eq!(store.outside_root(), 0, "ツリー外を指す Document がある");
}

/// 実索引で、行を囲んでいるシンボルが取れる。合成フィクスチャでは producer が
/// enclosing_range を実際に書くかを検査できない。
#[test]
#[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
fn 実索引は行を囲むものを答える() {
    let repo_root = test_repo();
    let store = load(&repo_root, &repo_root).expect("索引と出自の申告が揃っている");

    let mut checked = 0;
    for rel in covered_rust_files(&repo_root, &store, 8) {
        let source = std::fs::read_to_string(repo_root.join(&rel)).unwrap();
        let lines = source.lines().count();
        for (declaration, _) in source
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("pub fn ") && !l.trim_end().ends_with(';'))
        {
            let inside = declaration + 3;
            if inside >= lines {
                continue;
            }
            let sheaf_core::Enclosures::Exact(found) =
                sheaf_core::enclosures_at(&store, &rel, inside as u32)
            else {
                continue;
            };
            // 画面の外にある宣言のうちいちばん内側、が sticky に出るもの。
            let Some(innermost) = found
                .iter()
                .map(|e| e.declaration.line as usize)
                .find(|line| *line < inside)
            else {
                continue;
            };
            assert_eq!(
                innermost,
                declaration,
                "{}:{} を囲むいちばん内側の宣言が違う",
                rel.display(),
                inside + 1
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "索引が囲みを 1 件も答えなかった");
}

/// 呼び出し口が選ばせうる位置を、リポジトリの実ファイルで全部叩く。索引が実際にどれだけ
/// 答えるか、答えたものが説明を持つか、飛び先が実在するかを一度に見る。
///
/// 合成した索引では、rust-analyzer が実際に何を書くかを検査できない。ここが落ちるのは、
/// 宣言の綴りが変わって読めなくなったとき (`Signature` のフィールド番号がまさにそれ) と、
/// 種別の番号が変わったとき。
#[test]
#[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
fn 実索引は実ファイルに答え説明と飛び先を持つ() {
    use crate::symbol_index::{code_identifiers_on_line, occurrence_span_in_source};

    let repo_root = test_repo();
    let store = load(&repo_root, &repo_root).expect("索引と出自の申告が揃っている");

    let (mut asked, mut exact, mut described, mut signed) = (0usize, 0usize, 0usize, 0usize);
    let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut slowest = Duration::ZERO;
    let mut examples: Vec<String> = Vec::new();

    for rel in covered_rust_files(&repo_root, &store, 3) {
        let abs = repo_root.join(&rel);
        let source = std::fs::read_to_string(&abs).unwrap();
        let mask = CodeMask::compute(&source, &rel.to_string_lossy());
        let index = SymbolIndex::new(repo_root.clone());
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
                let started = Instant::now();
                let answer =
                    sheaf_core::definition_at(&store, &bridge, &rel, line as u32, start as u32);
                slowest = slowest.max(started.elapsed());
                let Definition::Exact(locations) = answer else {
                    continue;
                };
                exact += 1;
                for loc in &locations {
                    // 飛び先は必ずリポジトリ内の実在する行でなければならない。
                    let target = repo_root.join(&loc.path);
                    let text = std::fs::read_to_string(&target)
                        .unwrap_or_else(|_| panic!("飛び先が存在しない: {}", target.display()));
                    assert!(
                        (loc.line as usize) < text.lines().count(),
                        "飛び先の行がファイルの外: {}:{}",
                        loc.path.display(),
                        loc.line
                    );
                }
                if examples.len() < 5 {
                    examples.push(format!(
                        "  {}:{line} {word} -> {:?}",
                        rel.display(),
                        locations[0]
                    ));
                }
                // ツリーの中に定義がある = このワークスペース自身のシンボル。索引は必ず
                // SymbolInformation を書くので、説明が欠けていれば読めなくなっている。
                let Some(detail) =
                    sheaf_core::describe_at(&store, &bridge, &rel, line as u32, start as u32)
                        .into_iter()
                        .next()
                else {
                    continue;
                };
                described += 1;
                if detail.signature.is_some() {
                    signed += 1;
                }
                let label = kind_label(detail.kind);
                if !label.is_empty() {
                    *kinds.entry(label).or_default() += 1;
                }
            }
        }
    }

    println!("問い合わせ {asked} / Exact {exact} / 説明 {described} / 宣言 {signed}");
    println!("種別の内訳: {kinds:?} / 最遅 1 クエリ {slowest:?}");
    for e in &examples {
        println!("{e}");
    }

    assert!(described > 100, "索引がほとんど答えていない: {described}");
    assert!(
        signed * 20 >= described * 19,
        "自前のシンボルの宣言が読めていない: {signed}/{described}"
    );
    let with_kind: usize = kinds.values().sum();
    assert!(
        with_kind * 20 >= described * 19,
        "自前のシンボルの種別が読めていない: {with_kind}/{described}"
    );
    // 分類が 1 種類に潰れていたら、番号の対応表が壊れている。
    assert!(kinds.len() >= 5, "種別が偏りすぎ: {kinds:?}");
    assert!(
        slowest < Duration::from_millis(100),
        "1 クエリが gd の予算 (100ms) を超えた: {slowest:?}"
    );
}

// 「向き先が違うツリーには答えない」の検査は sheaf 側 (`Slot`) にある。判定そのものが
// あちらにあるので、こちらに写しを置くと片方だけが古くなる。
