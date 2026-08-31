//! シンボルインデックスの構築とクエリのテスト。

use std::path::PathBuf;

use super::extract_rust::extract_rust_symbols;
use super::index::SymbolIndex;
use super::model::{Scope, Symbol, SymbolKind};

#[test]
fn a_fresh_index_is_unavailable_until_it_is_built() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    assert!(!idx.is_available());
    assert_eq!(idx.root(), PathBuf::from("/tmp"));
}

#[test]
fn an_unbuilt_index_finds_no_definitions() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    let results = idx.find_definitions("foo", std::path::Path::new(""));
    assert!(results.is_empty());
}

#[test]
fn every_top_level_rust_item_kind_is_extracted() {
    let source = r#"
pub fn hello_world() {
    println!("hello");
}

struct MyStruct {
    field_a: u32,
}

enum Color {
    Red,
    Blue,
}

trait Drawable {
    fn draw(&self);
}

impl Drawable for MyStruct {
    fn draw(&self) {}
}

type Alias = Vec<u32>;

const MAX_SIZE: usize = 100;

static GLOBAL: &str = "test";

mod submodule;

macro_rules! my_macro {
    () => {};
}
"#;

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();

    let mut symbols = Vec::new();
    extract_rust_symbols(tree.root_node(), source, "test.rs", &mut symbols);

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"hello_world"));
    assert!(names.contains(&"MyStruct"));
    assert!(names.contains(&"Color"));
    assert!(names.contains(&"Drawable"));
    assert!(names.contains(&"Alias"));
    assert!(names.contains(&"MAX_SIZE"));
    assert!(names.contains(&"GLOBAL"));
    assert!(names.contains(&"submodule"));
    assert!(names.contains(&"my_macro"));

    // enum のバリアントを確認する。
    assert!(names.contains(&"Red"));
    assert!(names.contains(&"Blue"));

    // フィールドを確認する。
    assert!(names.contains(&"field_a"));

    // impl を確認する — parent は "MyStruct" のはず。
    let impl_sym = symbols.iter().find(|s| s.kind == SymbolKind::Impl).unwrap();
    assert_eq!(impl_sym.parent.as_deref(), Some("MyStruct"));

    // impl 内の関数を確認する。
    let draw_fns: Vec<_> = symbols.iter().filter(|s| s.name == "draw").collect();
    assert!(!draw_fns.is_empty());

    // 行番号が 1 始まりで妥当な値であることを確認する。
    let hello = symbols.iter().find(|s| s.name == "hello_world").unwrap();
    assert!(hello.line >= 1);
    assert_eq!(hello.kind, SymbolKind::Function);
}

#[test]
fn fields_are_not_offered_as_definitions() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    {
        let mut data = idx.data.lock().unwrap();
        data.symbols = vec![
            Symbol {
                name: "Foo".to_string(),
                kind: SymbolKind::Struct,
                file_path: "lib.rs".to_string(),
                line: 1,
                scope: Scope::Global,
                parent: None,
            },
            Symbol {
                name: "Foo".to_string(),
                kind: SymbolKind::Field,
                file_path: "lib.rs".to_string(),
                line: 5,
                scope: Scope::Global,
                parent: None,
            },
        ];
        data.available = true;
    }
    let defs = idx.find_definitions("Foo", std::path::Path::new(""));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].kind, SymbolKind::Struct);
}

// find_references はコード以外の拡張子・コード以外の一致を除外する

/// dir の下に name というファイルを contents で書く。親ディレクトリも作成する。
fn write_fixture(dir: &std::path::Path, name: &str, contents: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn find_references_skips_non_code_extensions() {
    let dir = std::env::temp_dir().join(format!("refs_ext_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    write_fixture(&dir, "notes.md", "widget appears here\n");
    write_fixture(&dir, "Cargo.toml", "widget = \"1.0\"\n");
    write_fixture(&dir, "config.yaml", "widget: true\n");
    write_fixture(&dir, "config.yml", "widget: true\n");
    write_fixture(&dir, "data.json", "{\"widget\": 1}\n");
    write_fixture(&dir, "lib.rs", "fn widget() {}\n");

    let idx = SymbolIndex::new(dir.clone());
    let refs = idx.find_references("widget", &dir);

    assert_eq!(refs.len(), 1, "only the .rs hit should survive: {refs:?}");
    assert_eq!(refs[0].file_path, "lib.rs");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_references_skips_comment_and_string_hits() {
    let dir = std::env::temp_dir().join(format!("refs_mask_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    write_fixture(
        &dir,
        "lib.rs",
        "// widget does things\nfn real() {\n    let s = \"widget\";\n    widget();\n}\n",
    );

    let idx = SymbolIndex::new(dir.clone());
    let refs = idx.find_references("widget", &dir);

    // 実際にコード位置の参照なのは4行目の呼び出しだけである — 1行目の
    // コメントと3行目の文字列リテラルは返ってきてはならない。
    assert_eq!(
        refs.len(),
        1,
        "expected exactly one code-position hit: {refs:?}"
    );
    assert_eq!(refs[0].line, 4);

    let _ = std::fs::remove_dir_all(&dir);
}

/// ホバー経路のフレーム予算ゲート。
///
/// あえて new を使う — このリポジトリでは最悪ケースで、約200ファイルに
/// 言及がある。このテストの以前のバージョンは find_references 自体を
/// 計測していたが、これは6ファイルにしか出現しないため、ヒット数に応じて
/// スケールするコストをまったく検証できていなかった。それは、ありふれた
/// 名前をホバーすると約157msかかりフレームを10枚落としていたにもかかわらず
/// パスしていたということである。上限が作業量を制限する仕組みなので、
/// 計測すべきは上限付きの呼び出しであり、しかも最も負荷の高い名前で行う
/// 必要がある。
#[test]
fn hover_reference_count_stays_within_a_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let idx = SymbolIndex::new(root.clone());

    // まず計測なしで1回実行しておく。こうしないと、このコードとは無関係な
    // コールドなページキャッシュのディスク読み取りに計測結果が支配されてしまう。
    idx.count_references_upto("new", &root, 50);

    let start = std::time::Instant::now();
    let (count, capped) = idx.count_references_upto("new", &root, 50);
    let elapsed = start.elapsed();

    assert!(count > 0, "sanity: `new` should be found at all");
    assert!(
        capped,
        "sanity: `new` should exceed a cap of 50 in this repo"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(30),
        "hover reference count took {elapsed:?}; uncapped this measured ~157ms \
         for `new`, which is ten dropped frames at 16ms"
    );
}

/// 上限なしの検索はユーザ起動（gr、参照オーバーレイ）なので、1フレームより
/// 長くかかってもよい — ただしツリー全体をパースする挙動へ退化しては
/// ならない。特徴的な名前ならファイル数は少なく、マスク導入前のベースラインで
/// ある 8〜10ms に近い値に収まるはず。
#[test]
fn find_references_defers_parsing_to_files_that_match() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let idx = SymbolIndex::new(root.clone());
    idx.find_references("count_references_upto", &root);

    let start = std::time::Instant::now();
    let refs = idx.find_references("count_references_upto", &root);
    let elapsed = start.elapsed();

    assert!(
        !refs.is_empty(),
        "sanity: the symbol should be found at all"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(80),
        "took {elapsed:?}; parsing every visited file instead of only the \
         matching ones measures ~121ms here"
    );
}

#[test]
fn find_implementations_matches_the_impl_symbol() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    {
        let mut data = idx.data.lock().unwrap();
        data.symbols = vec![Symbol {
            name: "impl MyStruct".to_string(),
            kind: SymbolKind::Impl,
            file_path: "lib.rs".to_string(),
            line: 10,
            scope: Scope::Global,
            parent: Some("MyStruct".to_string()),
        }];
        data.available = true;
    }
    let impls = idx.find_implementations("MyStruct");
    assert_eq!(impls.len(), 1);
}

// worktree をまたいだ re-root

/// 新規の一時ディレクトリの下にファイル（相対パス → 内容）を書く。
fn scratch_tree(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "symidx_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }
    dir
}

/// re-root した後、インデックスは新しいツリーについてのみ答えなければ
/// ならない。古いツリーについて答え続けることが、別のブランチの行番号で
/// もっともらしいファイルへジャンプしてしまうという失敗として現れる。
#[test]
fn rerooting_replaces_what_the_index_answers_for() {
    let a = scratch_tree("root_a", &[("a.rs", "pub fn only_in_a() {}\n")]);
    let b = scratch_tree("root_b", &[("b.rs", "pub fn only_in_b() {}\n")]);

    let idx = SymbolIndex::new(a.clone());
    idx.build().unwrap();
    assert!(
        !idx.find_definitions("only_in_a", std::path::Path::new(""))
            .is_empty()
    );
    assert!(
        idx.find_definitions("only_in_b", std::path::Path::new(""))
            .is_empty()
    );

    idx.set_root(b.clone());
    // 再構築が反映されるまで、インデックスはたった今離れたツリーを使って
    // 答え続けるのではなく、何も知らないと認めなければならない。
    assert!(
        !idx.is_available(),
        "re-rooting must invalidate until the rebuild lands"
    );
    assert!(
        idx.find_definitions("only_in_a", std::path::Path::new(""))
            .is_empty()
    );

    idx.build().unwrap();
    assert!(
        !idx.find_definitions("only_in_b", std::path::Path::new(""))
            .is_empty()
    );
    assert!(
        idx.find_definitions("only_in_a", std::path::Path::new(""))
            .is_empty(),
        "symbols from the previous root must not survive"
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// 同じパスへの re-root はインデックスを吹き飛ばしてはならない —
/// ファイルシステム変更経路はその場で再構築するので、そうしないと保存の
/// たびに「未準備」状態にばたついてしまう。
#[test]
fn rerooting_to_the_same_path_is_a_no_op() {
    let a = scratch_tree("root_same", &[("a.rs", "pub fn keep() {}\n")]);
    let idx = SymbolIndex::new(a.clone());
    idx.build().unwrap();

    idx.set_root(a.clone());
    assert!(idx.is_available(), "same root must not invalidate");
    assert!(
        !idx.find_definitions("keep", std::path::Path::new(""))
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(&a);
}

/// re-root より前に始まったビルドは、その結果を公開してはならない。
///
/// BackgroundOp は自身が spawn したワーカーをキャンセルできない — join
/// handle を drop するだけで、ワーカーは誰かが聞いているかどうかに関わらず
/// 共有インデックスに書き込む — なので唯一の防御策は、古い結果が届いた時点で
/// それを拒否することである。
///
/// 遅いビルドと re-root を実際に競合させるのではなく、publish のガードに
/// 対する3つの順序付き呼び出しとして再現している: テスト対象の絡み合いは
/// まさに「刻む → root を動かす → 完了する」であり、スレッドを使った版では
/// スケジューラがたまたま協力してくれた時にしかこの状況に到達しない。
#[test]
fn a_build_that_started_before_a_reroot_is_discarded() {
    let old = scratch_tree("stale_old", &[("old.rs", "pub fn from_old_tree() {}\n")]);
    let new = scratch_tree("stale_new", &[("new.rs", "pub fn from_new_tree() {}\n")]);

    let idx = SymbolIndex::new(old.clone());

    // old に対してビルドが始まり、自身に generation を刻む。
    let stamped = idx.generation();
    // まだ走査している最中に、ユーザが worktree を切り替える。
    idx.set_root(new.clone());
    // ビルドが完了し、見つけたものを差し出す。
    let stale = vec![Symbol {
        name: "from_old_tree".to_string(),
        kind: SymbolKind::Function,
        file_path: "old.rs".to_string(),
        line: 1,
        scope: Scope::Global,
        parent: None,
    }];
    let published = idx.publish(stale, stamped);

    assert_eq!(
        published, 0,
        "a build stamped with the previous generation must publish nothing"
    );
    assert!(
        idx.find_definitions("from_old_tree", std::path::Path::new(""))
            .is_empty(),
        "stale symbols leaked into the re-rooted index"
    );
    assert!(
        !idx.is_available(),
        "a discarded build must not mark the index ready"
    );

    // re-root の *後* に始まったビルドは通常どおり公開される。
    idx.build().unwrap();
    assert!(
        !idx.find_definitions("from_new_tree", std::path::Path::new(""))
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&new);
}

/// 文法を持たない言語であっても、参照検索の結果が空で返ってきてはならない。
/// すべてのヒットを捨てると、本当は「判定できなかった」だけなのに「参照は
/// 存在しない」と答えてしまう — これはこの作業がなくそうとしている
/// 「黙って間違った答えを返す」問題そのものを、別の場所に向けているだけである。
#[test]
fn find_references_keeps_hits_in_unparseable_languages() {
    let dir = scratch_tree(
        "refs_unsupported",
        &[
            ("used.py", "def wrapper():\n    return target_name()\n"),
            ("used.rs", "pub fn caller() { target_name(); }\n"),
        ],
    );

    let idx = SymbolIndex::new(dir.clone());
    let refs = idx.find_references("target_name", &dir);

    let files: Vec<&str> = refs.iter().map(|r| r.file_path.as_str()).collect();
    assert!(
        files.contains(&"used.py"),
        "Python hits must survive rather than be silently dropped; got {files:?}"
    );
    assert!(
        files.contains(&"used.rs"),
        "Rust hits must still resolve; got {files:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 関数の中の宣言は、どの言語でも定義候補にならない。
///
/// 名前でしか引けないので、載せると別のファイルの同名のローカルが答えになる
/// (.tsx の data が無関係な .ts の const data を引き当てたのがこれ)。言語を
/// またいで 1 つの性質として見る — 言語ごとに書くと、次に足した言語で同じ穴が空く。
#[test]
fn locals_are_never_definition_candidates() {
    let dir = scratch_tree(
        "locals",
        &[
            (
                "lib/helper.rs",
                "pub const SHARED_RS: u32 = 1;\n\npub fn load() -> u32 {\n    const HIDDEN_RS: u32 = 2;\n    static ALSO_RS: u32 = 3;\n    HIDDEN_RS + ALSO_RS\n}\n",
            ),
            (
                "lib/helper.go",
                "package helper\n\nconst SharedGo = 1\n\nvar (\n\tGroupedGo = 2\n)\n\nfunc load() int {\n\tconst hiddenGo = 3\n\tvar alsoGo = 4\n\treturn hiddenGo + alsoGo\n}\n",
            ),
            (
                "lib/helper.ts",
                "export const sharedTs = 1;\n\nfor (const topLoopTs of [1]) {\n    console.log(topLoopTs);\n}\n\nexport function load() {\n    const hiddenTs = 2;\n    return hiddenTs;\n}\n",
            ),
        ],
    );
    let idx = SymbolIndex::new(dir);
    idx.build().unwrap();

    // 問い合わせ元は同じ言語の別ファイル。tsx から ts を引く形が元の症状。
    for (from, hidden, visible) in [
        ("main.rs", &["HIDDEN_RS", "ALSO_RS"][..], &["SHARED_RS"][..]),
        (
            "main.go",
            &["hiddenGo", "alsoGo"][..],
            &["SharedGo", "GroupedGo"][..],
        ),
        (
            "Page.tsx",
            &["hiddenTs", "topLoopTs"][..],
            &["sharedTs"][..],
        ),
    ] {
        let at = std::path::Path::new(from);
        for name in hidden {
            assert!(
                idx.find_definitions(name, at).is_empty(),
                "{from} から見て {name} が定義候補に残っている"
            );
        }
        for name in visible {
            assert_eq!(
                idx.find_definitions(name, at).len(),
                1,
                "{from} から見て {name} が定義候補から消えている"
            );
        }
    }
}
