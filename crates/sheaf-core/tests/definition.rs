//! 受け入れ条件の検査。
//!
//! 小さい索引はテスト内で組み立てる。実索引（12MB）を要求するものは #[ignore] にして、
//! 環境変数で場所を渡す。scratchpad の索引は消えるので、無ければ明示的に失敗させる。

use protobuf::{EnumOrUnknown, Message, MessageField};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, TextEncoding};
use sheaf_core::{
    Definition, IndexSource, Location, Span, Store, SyntacticAnswer, SyntacticLayer, Token,
    definition_at,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SOURCE: &str = "pub fn greet() {}\nfn caller() { greet(); }\n";
const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";

/// 語の切り出しをわざと素朴にした構文層。英数字・下線・非 ASCII が続く範囲を 1 語とする。
/// コメントも文字列も予約語も区別しない。この雑な層で通ることが、索引側の規則が
/// 構文層の賢さに依存していないことの検査になる。
struct Recording {
    calls: RefCell<Vec<(PathBuf, u32, u32)>>,
    answer: SyntacticAnswer,
}

impl Recording {
    fn new(answer: SyntacticAnswer) -> Self {
        Recording {
            calls: RefCell::new(Vec::new()),
            answer,
        }
    }
    fn calls(&self) -> Vec<(PathBuf, u32, u32)> {
        self.calls.borrow().clone()
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

impl SyntacticLayer for Recording {
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token {
        let Ok(src) = std::fs::read(path) else {
            return Token::Unknown;
        };
        let Some(text) = src.split(|b| *b == b'\n').nth(line as usize) else {
            return Token::NotWord;
        };
        let col = col as usize;
        if col >= text.len() || !is_word_byte(text[col]) {
            return Token::NotWord;
        }
        let mut start = col;
        while start > 0 && is_word_byte(text[start - 1]) {
            start -= 1;
        }
        let mut end = col + 1;
        while end < text.len() && is_word_byte(text[end]) {
            end += 1;
        }
        Token::Word(Span {
            start_line: line,
            start_col: start as u32,
            end_line: line,
            end_col: end as u32,
        })
    }

    fn definition_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        self.calls
            .borrow_mut()
            .push((path.to_path_buf(), line, col));
        self.answer.clone()
    }

    fn references_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        self.calls
            .borrow_mut()
            .push((path.to_path_buf(), line, col));
        self.answer.clone()
    }
}

/// 構文層が答えを持たない場合。索引だけで何が引けるかを見るときに使う。
fn silent() -> Recording {
    Recording::new(SyntacticAnswer::NotCode)
}

/// テスト用の作業ディレクトリ。呼び出しごとに別の場所になる。
fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-test-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

/// 指定した相対パスについて、いまツリーにある内容のハッシュを表にする。
fn hashes_of(root: &Path, rels: &[&str]) -> HashMap<PathBuf, String> {
    rels.iter()
        .map(|rel| {
            let bytes = std::fs::read(root.join(rel)).unwrap();
            (PathBuf::from(rel), sheaf_core::blob_hash(&bytes))
        })
        .collect()
}

/// 索引 1 本を、索引ルート = root として投入する。
fn load_single(
    index_path: &Path,
    root: &Path,
    expected: HashMap<PathBuf, String>,
) -> sheaf_core::Result<Store> {
    Store::load(
        &[IndexSource {
            index: index_path.to_path_buf(),
            subroot: PathBuf::new(),
            expected,
        }],
        root,
    )
}

/// 「いま root にあるツリーが、索引を生成したツリーそのものである」と申告して投入する。
/// 索引を書き出した直後に読むテストのためのもので、実運用の申告元は git になる。
fn load_as_indexed_tree(index_path: &Path, root: &Path) -> sheaf_core::Result<Store> {
    let mut expected = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                // 実索引のツリーでは target が数十 GB あり、歩くと検査が分単位になる。
                if name != "target" && name != ".git" {
                    stack.push(path);
                }
            } else if entry.file_type().is_ok_and(|t| t.is_file())
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(rel) = path.strip_prefix(root)
            {
                expected.insert(rel.to_path_buf(), sheaf_core::blob_hash(&bytes));
            }
        }
    }
    load_single(index_path, root, expected)
}

fn occurrence(range: Vec<i32>, symbol: &str, roles: i32) -> Occurrence {
    Occurrence {
        range,
        symbol: symbol.to_string(),
        symbol_roles: roles,
        ..Default::default()
    }
}

/// `greet` の定義と呼び出しを 1 つずつ持つ索引を書き出し、ルートを返す。
fn build_index(tag: &str, encoding: i32) -> (PathBuf, PathBuf) {
    let root = workdir(tag);
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();

    let doc = Document {
        relative_path: "src/lib.rs".to_string(),
        language: "rust".to_string(),
        occurrences: vec![
            // pub fn greet() {}      -> greet は 7..12
            occurrence(vec![0, 7, 12], SYMBOL, 1),
            // fn caller() { greet(); } -> greet は 14..19
            occurrence(vec![1, 14, 19], SYMBOL, 0),
        ],
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(encoding),
            ..Default::default()
        }),
        documents: vec![doc],
        ..Default::default()
    };

    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

/// 定義と参照が別ファイルにある索引。参照側だけを変えずに定義側を変えられるようにする。
fn build_two_file_index(tag: &str) -> (PathBuf, PathBuf) {
    let root = workdir(tag);
    std::fs::write(root.join("src/lib.rs"), "pub fn greet() {}\n").unwrap();
    std::fs::write(root.join("src/caller.rs"), "fn caller() { greet(); }\n").unwrap();

    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![
            Document {
                relative_path: "src/lib.rs".to_string(),
                language: "rust".to_string(),
                occurrences: vec![occurrence(vec![0, 7, 12], SYMBOL, 1)],
                ..Default::default()
            },
            Document {
                relative_path: "src/caller.rs".to_string(),
                language: "rust".to_string(),
                occurrences: vec![occurrence(vec![0, 14, 19], SYMBOL, 0)],
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

// 確信度を無視して位置を取れないことは型の性質なので doctest 側で見ている（cargo test --doc）。
// ここで見るのは、実際の呼び出しでそれが成り立つこと。

#[test]
fn 索引が答えられる位置は_exact_を返す() {
    let (index_path, root) = build_index("exact", TextEncoding::UTF8 as i32);
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, 14);

    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("src/lib.rs"),
            line: 0,
            col: 7,
        }])
    );
    assert!(
        syntactic.calls().is_empty(),
        "索引が答えられたのに構文層が呼ばれている"
    );
}

#[test]
fn 語の途中の列でも引ける() {
    let (index_path, root) = build_index("mid", TextEncoding::UTF8 as i32);
    let store = load_as_indexed_tree(&index_path, &root).unwrap();

    // 語の先頭ではなく途中を聞く。位置から語の範囲を作り直して引く。
    let answer = definition_at(&store, &silent(), Path::new("src/lib.rs"), 1, 17);
    assert!(matches!(answer, Definition::Exact(_)), "{answer:?}");
}

#[test]
fn 索引が無くても構文層に回る() {
    let root = workdir("noindex");
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();
    // Document を 1 つも持たない索引 = まだ生成されていない状態
    let index_path = root.join("empty.scip");
    std::fs::write(&index_path, Index::default().write_to_bytes().unwrap()).unwrap();

    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    assert!(store.is_empty());
    let syntactic = Recording::new(SyntacticAnswer::Found(vec![Location {
        path: PathBuf::from("src/lib.rs"),
        line: 0,
        col: 7,
    }]));

    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, 14);

    assert!(matches!(answer, Definition::Syntactic(_)), "{answer:?}");
    assert_eq!(syntactic.calls().len(), 1, "構文層が呼ばれていない");
}

#[test]
fn occurrence_の無い位置は構文層に回る() {
    let (index_path, root) = build_index("gap", TextEncoding::UTF8 as i32);
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // caller の位置には occurrence を入れていない。索引は最新のままである。
    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, 3);

    assert_eq!(answer, Definition::NotCode);
    assert_eq!(
        syntactic.calls().len(),
        1,
        "索引が最新でも occurrence が無い位置は構文層に回さなければならない"
    );
}

#[test]
fn ファイルが変わったら_exact_にしない() {
    let (index_path, root) = build_index("changed", TextEncoding::UTF8 as i32);
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    // 呼び出し側の語はそのままで、定義側だけ変える。位置は生きているが索引は古い。
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn hello() {}\nfn caller() { greet(); }\n",
    )
    .unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, 14);

    assert_eq!(answer, Definition::NotCode);
    assert_eq!(syntactic.calls().len(), 1);
}

/// 索引を作ったツリーと、そこから 1 ファイルだけ変えた別のツリーを作る。
///
/// 聞く側の `caller.rs` は両方で同じにしてある。ここが違うと、位置が occurrence に
/// 当たらないというだけの理由で答えが消え、鮮度の検査を通り抜けてしまう。
fn build_worktree_pair(tag: &str) -> (PathBuf, HashMap<PathBuf, String>, PathBuf) {
    let (index_path, indexed) = build_two_file_index(tag);
    let expected = hashes_of(&indexed, &["src/lib.rs", "src/caller.rs"]);

    let other = workdir(&format!("{tag}-other"));
    // 定義を 0 行目から 1 行目へずらす。索引はまだ 0 行目と言っている。
    std::fs::write(
        other.join("src/lib.rs"),
        "// 別のツリー\npub fn greet() {}\n",
    )
    .unwrap();
    std::fs::write(other.join("src/caller.rs"), "fn caller() { greet(); }\n").unwrap();
    (index_path, expected, other)
}

#[test]
fn 索引を作ったのとは別のツリーに向けたら_exact_にしない() {
    // worktree の形。同じ base から checkout した別のツリーで、飛び先だけが編集されている。
    // ロード時にディスクを読んで期待値にすると、編集済みのファイルも「そのまま」と判定され、
    // 索引が言う 0 行目を Exact として返してしまう（実際の定義は 1 行目）。
    let (index_path, expected, other) = build_worktree_pair("other-tree");
    let store = load_single(&index_path, &other, expected).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/caller.rs"), 0, 14);

    assert_eq!(answer, Definition::NotCode);
    assert_eq!(syntactic.calls().len(), 1, "構文層に回っていない");
}

#[test]
fn 別のツリーでも内容が同じファイルなら_exact_を返す() {
    // 上の対照。全部を落とすなら worktree で索引を使い回す意味が無くなるので、
    // 「変わっていないファイルは答えられる」ことを一緒に固定する。
    let (index_path, indexed) = build_two_file_index("same-content");
    let expected = hashes_of(&indexed, &["src/lib.rs", "src/caller.rs"]);

    let other = workdir("same-content-other");
    std::fs::write(other.join("src/lib.rs"), "pub fn greet() {}\n").unwrap();
    std::fs::write(other.join("src/caller.rs"), "fn caller() { greet(); }\n").unwrap();

    let store = load_single(&index_path, &other, expected).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/caller.rs"), 0, 14);

    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("src/lib.rs"),
            line: 0,
            col: 7,
        }])
    );
}

#[test]
fn 期待ハッシュを渡されていないファイルは_exact_にしない() {
    // 索引には載っているが呼び出し側が出自を言えないファイル。
    // 「知らない」を「変わっていない」に丸めない。
    let (index_path, root) = build_index("no-provenance", TextEncoding::UTF8 as i32);
    let store = load_single(&index_path, &root, HashMap::new()).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, 14);

    assert_eq!(answer, Definition::NotCode);
}

#[test]
fn 出自の申告が無い_document_の数を数える() {
    // 2 つの Document のうち 1 つだけ expected に載せる。表に無かった 1 件だけが数えられること。
    let (index_path, indexed) = build_two_file_index("missing-provenance");
    let expected = hashes_of(&indexed, &["src/lib.rs"]);

    let store = load_single(&index_path, &indexed, expected).unwrap();

    assert_eq!(store.missing_provenance(), 1);
}

#[test]
fn 飛び先のファイルが変わったら_exact_にしない() {
    // 聞かれた位置のファイルが索引生成時のままでも、飛び先のファイルが変わっていれば
    // その行番号はもう信じられない。Exact は「依拠したファイルがすべて索引生成時のまま」を意味する。
    let (index_path, root) = build_two_file_index("target-changed");
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "// 定義を消した\npub fn greet() {}\n",
    )
    .unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/caller.rs"), 0, 14);

    assert_eq!(answer, Definition::NotCode);
    assert_eq!(syntactic.calls().len(), 1, "構文層に回っていない");
}

#[test]
fn 候補の一部だけが古いときは部分的な_exact_を返さない() {
    // 同じ語に定義が 2 つ乗っていて、片方の飛び先だけが変わっている状況。
    // 新しいほうだけ返すと、消えた候補があることを呼び出し側が知れないまま
    // 「索引がすべて答えた」と読まれる。
    let root = workdir("partial");
    std::fs::write(root.join("src/a.rs"), "pub struct A;\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "pub struct B;\n").unwrap();
    std::fs::write(root.join("src/use.rs"), "fn f() { name; }\n").unwrap();

    let doc = |path: &str, occs: Vec<Occurrence>| Document {
        relative_path: path.to_string(),
        language: "rust".to_string(),
        occurrences: occs,
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![
            doc("src/a.rs", vec![occurrence(vec![0, 11, 12], "sym/A#", 1)]),
            doc("src/b.rs", vec![occurrence(vec![0, 11, 12], "sym/B#", 1)]),
            doc(
                "src/use.rs",
                vec![
                    occurrence(vec![0, 9, 13], "sym/A#", 0),
                    occurrence(vec![0, 9, 13], "sym/B#", 0),
                ],
            ),
        ],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();

    let both = definition_at(&store, &silent(), Path::new("src/use.rs"), 0, 9);
    assert!(
        matches!(&both, Definition::Exact(l) if l.len() == 2),
        "{both:?}"
    );

    std::fs::write(root.join("src/b.rs"), "// 動かした\npub struct B;\n").unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);
    let partial = definition_at(&store, &syntactic, Path::new("src/use.rs"), 0, 9);

    assert_eq!(partial, Definition::NotCode, "部分的な Exact を返している");
    assert_eq!(syntactic.calls().len(), 1, "構文層に回っていない");
}

#[test]
fn ツリーの外を指す相対パスの_document_は投入しない() {
    // 索引ファイルは外から来る入力。Path::join は絶対パスを渡されるとルートを捨てるので、
    // 検査しないとツリー外のファイルを読んで、その位置を答えとして返してしまう。
    let root = workdir("escape");
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: ["/etc/passwd", "../../outside.rs", "src/ok.rs"]
            .iter()
            .map(|p| Document {
                relative_path: p.to_string(),
                language: "rust".to_string(),
                occurrences: vec![occurrence(vec![0, 0, 4], SYMBOL, 1)],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let store = load_as_indexed_tree(&index_path, &root).unwrap();

    assert_eq!(store.len(), 1, "ツリー外の Document が投入されている");
}

#[test]
fn document_側のエンコーディング宣言を読む() {
    // metadata は UTF-8、Document は UTF-16 と言っている索引。greet の手前に非 ASCII
    // なコメントを置いてあるので、バイトと UTF-16 の数え方はここでずれる
    // （バイト 17、UTF-16 15）。Document 側のフィールド番号を読み違えて未指定
    // 扱いにすると、この occurrence は規則4で除外されて Exact が返らなくなる。
    let root = workdir("docenc");
    let source = "/* あ */ pub fn greet() {}\n";
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            position_encoding: EnumOrUnknown::from_i32(
                PositionEncoding::UTF16CodeUnitOffsetFromLineStart as i32,
            ),
            occurrences: vec![occurrence(vec![0, 15, 20], SYMBOL, 1)],
            ..Default::default()
        }],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // バイト 18 は "greet" の中（変換後のバイト範囲は 17..22）。
    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 0, 18);

    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("src/lib.rs"),
            line: 0,
            col: 17,
        }])
    );
}

#[test]
fn 構文層が語を判定できなければ索引を引かない() {
    let (index_path, root) = build_index("unknown", TextEncoding::UTF8 as i32);
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    // token_at が Unknown を返す状態を、読めないパスを渡して作る。
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    let answer = definition_at(&store, &syntactic, Path::new("src/missing.rs"), 1, 14);

    assert_eq!(answer, Definition::NotCode);
    assert_eq!(syntactic.calls().len(), 1, "構文層に回っていない");
}

#[test]
fn utf16_を宣言する索引は投入しない() {
    // ここで弾かれるのは列の数え方の話ではなく、metadata.text_document_encoding が
    // ディスク上のファイルを UTF-16 だと申告しているから（sheaf はバイト列として
    // 安全に読めない）。Document.position_encoding はこの索引では未指定のまま。
    let (index_path, root) = build_index("utf16", TextEncoding::UTF16 as i32);
    let err = load_as_indexed_tree(&index_path, &root).unwrap_err();
    assert!(
        matches!(err, sheaf_core::SheafError::UnsupportedEncoding { .. }),
        "{err:?}"
    );
}

/// scip-typescript の実際の出力を模した索引を書き出す。Document.position_encoding は
/// 未指定のまま（実物どおり）、occurrence の range は実測した UTF-16 コードユニット値。
fn build_shift_index() -> (PathBuf, PathBuf) {
    let root = workdir("shift");
    let source = "export const あいうえおかきく = 1\n\
                  export const aVeryLongIdentifier = 2\n\
                  export const t2 = 3\n\
                  export const z = あいうえおかきく + aVeryLongIdentifier + t2\n";
    std::fs::write(root.join("src/shift.ts"), source).unwrap();

    let sym = |name: &str| format!("scip-typescript npm app 0.0.1 src/`shift.ts`/{name}.");
    let doc = Document {
        relative_path: "src/shift.ts".to_string(),
        occurrences: vec![
            occurrence(vec![0, 13, 21], &sym("`あいうえおかきく`"), 1),
            occurrence(vec![1, 13, 32], &sym("aVeryLongIdentifier"), 1),
            occurrence(vec![2, 13, 15], &sym("t2"), 1),
            occurrence(vec![3, 13, 14], &sym("z"), 1),
            occurrence(vec![3, 17, 25], &sym("`あいうえおかきく`"), 0),
            occurrence(vec![3, 28, 47], &sym("aVeryLongIdentifier"), 0),
            occurrence(vec![3, 50, 52], &sym("t2"), 0),
        ],
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![doc],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

#[test]
fn 未指定エンコーディングでは非_ascii_の手前にある語を_exact_で誤答しない() {
    // aVeryLongIdentifier の手前に「あいうえおかきく」(非 ASCII) があるので、
    // UTF-16 単位の列をバイトのまま読むと t2 の範囲へずれて誤って重なっていた
    // (このテストが固定するバグそのもの)。いまは無回答になるのが正しい。
    let (index_path, root) = build_shift_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // 44 は shift.ts の実ファイルで aVeryLongIdentifier が始まる真のバイト位置。
    let answer = definition_at(&store, &syntactic, Path::new("src/shift.ts"), 3, 44);

    assert_eq!(answer, Definition::NotCode, "{answer:?}");
}

#[test]
fn 未指定エンコーディングでは非_ascii_の手前にある語_t2_も誤答しない() {
    let (index_path, root) = build_shift_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // 66 は shift.ts の実ファイルで t2 が始まる真のバイト位置。
    let answer = definition_at(&store, &syntactic, Path::new("src/shift.ts"), 3, 66);

    assert_eq!(answer, Definition::NotCode, "{answer:?}");
}

#[test]
fn 未指定エンコーディングでも非_ascii_の手前が無い語は変わらず_exact_を返す() {
    // aVeryLongIdentifier 自身の定義（1行目）は非 ASCII を含まない行にあるので、
    // バイトと UTF-16 の数え方が一致し、これまでどおり Exact が返らなければならない。
    let (index_path, root) = build_shift_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // 1 行目は非 ASCII を含まない行なので、3 行目の参照が除外されるのとは無関係に
    // 定義そのものは引ける。
    let direct = definition_at(&store, &syntactic, Path::new("src/shift.ts"), 1, 13);

    assert_eq!(
        direct,
        Definition::Exact(vec![Location {
            path: PathBuf::from("src/shift.ts"),
            line: 1,
            col: 13,
        }])
    );
}

#[test]
fn 未指定エンコーディングで手前が_ascii_の語は末尾が縮んでも誤答しない() {
    // あいうえおかきく への参照 (3行目) は開始位置の手前が ASCII なので usable_range を
    // 通過するが、宣言された範囲 [17,25] は UTF-16 単位のままで、真のバイト範囲 [17,41]
    // より短い（8文字 * 2バイトぶん縮む）。この縮んだ範囲を使っても、真の語の範囲
    // [17,41] のどの位置を尋ねても同じ正しい定義に解決し、他のシンボルを誤って
    // 指さないことを実際に全バイト位置を尋ねて確かめる。
    let (index_path, root) = build_shift_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);
    let want = Definition::Exact(vec![Location {
        path: PathBuf::from("src/shift.ts"),
        line: 0,
        col: 13,
    }]);

    // 17 は宣言どおりの開始位置（狙いの位置）。25 は宣言された終端（ここより縮む）。
    // 41 は実ファイル上での真の終端。17..41 の全バイト位置で確かめる。
    for col in 17..41u32 {
        let answer = definition_at(&store, &syntactic, Path::new("src/shift.ts"), 3, col);
        assert_eq!(answer, want, "col={col} で誤答または無回答になった");
    }
}

/// scip-typescript の実際の出力を模した jp.ts 相当の索引。
fn build_jp_index() -> (PathBuf, PathBuf) {
    let root = workdir("jpts");
    let source = "import {topLevel} from 'dep'\n\
                  \n\
                  export const 説明 = 'あ'\n\
                  // コメント: 日本語のあとに識別子が来る行\n\
                  export const messageJa = `パブリックリポジトリ数: ${topLevel(1)}`\n\
                  export const plain = topLevel(2)\n";
    std::fs::write(root.join("src/jp.ts"), source).unwrap();

    let sym = |name: &str| format!("scip-typescript npm app 0.0.1 src/`jp.ts`/{name}.");
    let doc = Document {
        relative_path: "src/jp.ts".to_string(),
        occurrences: vec![
            occurrence(vec![2, 13, 15], &sym("`説明`"), 1),
            occurrence(vec![4, 13, 22], &sym("messageJa"), 1),
            occurrence(
                vec![4, 41, 49],
                "scip-typescript npm dep 2.0.0 `index.d.ts`/topLevel().",
                0,
            ),
            occurrence(vec![5, 13, 18], &sym("plain"), 1),
        ],
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![doc],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

#[test]
fn jp_ts_でも非_ascii_の手前にある語だけが無回答になる() {
    let (index_path, root) = build_jp_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // messageJa の定義自体は行頭からの手前が ASCII のみなので、行の後方に非 ASCII の
    // テンプレート文字列があっても関係なく Exact が返らなければならない
    // （開始位置より前だけを見る、行全体は見ない）。
    let message_ja = definition_at(&store, &syntactic, Path::new("src/jp.ts"), 4, 15);
    assert_eq!(
        message_ja,
        Definition::Exact(vec![Location {
            path: PathBuf::from("src/jp.ts"),
            line: 4,
            col: 13,
        }])
    );

    // 説明 も手前が ASCII のみ（識別子自体が非 ASCII でも手前は関係ない）。
    let setsumei = definition_at(&store, &syntactic, Path::new("src/jp.ts"), 2, 14);
    assert_eq!(
        setsumei,
        Definition::Exact(vec![Location {
            path: PathBuf::from("src/jp.ts"),
            line: 2,
            col: 13,
        }])
    );

    // テンプレート内の topLevel 参照は手前に日本語があるので誤って何かに重なるより無回答。
    let top_level = definition_at(&store, &syntactic, Path::new("src/jp.ts"), 4, 63);
    assert_eq!(top_level, Definition::NotCode, "{top_level:?}");
}

// ここから先は実索引が要る。SHEAF_TEST_INDEX と SHEAF_TEST_ROOT で場所を渡す。
//   cargo test -- --ignored

fn real_index() -> (PathBuf, PathBuf) {
    let index =
        std::env::var("SHEAF_TEST_INDEX").expect("SHEAF_TEST_INDEX に .scip のパスを渡すこと");
    let root =
        std::env::var("SHEAF_TEST_ROOT").expect("SHEAF_TEST_ROOT にソースツリーのルートを渡すこと");
    (PathBuf::from(index), PathBuf::from(root))
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_format_のインライン引数は構文層に回る() {
    let (index_path, root) = real_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = Recording::new(SyntacticAnswer::NotCode);

    // let remote = format!("origin/{main_branch}"); の main_branch。
    // SCIP はこの位置に occurrence を持たないが、rust-analyzer は定義を返す。
    let rel = Path::new("src/git_engine/worktree_create.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let line_idx = 215;
    let line = src.lines().nth(line_idx).unwrap();
    let col = line.find("main_branch").expect("対象の行が変わっている") as u32;

    // 対照。同じ行の remote は索引に occurrence があるので Exact が返らなければならない。
    // これが無いと「索引が古いから NotCode」でもテストが通ってしまう。
    let control_col = line.find("remote").expect("対象の行が変わっている") as u32;
    let control = definition_at(&store, &silent(), rel, line_idx as u32, control_col);
    assert!(
        matches!(control, Definition::Exact(_)),
        "同じ行の索引が効いていない。鮮度の問題と区別できない: {control:?}"
    );

    let answer = definition_at(&store, &syntactic, rel, line_idx as u32, col);

    assert_eq!(answer, Definition::NotCode);
    assert_eq!(
        syntactic.calls(),
        vec![(root.join(rel), line_idx as u32, col)],
        "構文層が呼ばれていない"
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_コメント行は意味索引の答えにならない() {
    // 各 Document の先頭には、モジュール自身の定義がファイル全体を覆う範囲で入っている。
    // これを候補にすると、コメントの中を聞いてもモジュールの先頭が Exact で返る。
    let (index_path, root) = real_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let rel = Path::new("src/git_engine/worktree_create.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();

    let syntactic = silent();
    let mut checked = 0;
    let mut wrong = Vec::new();
    for (line_idx, line) in src.lines().enumerate() {
        if !line.trim_start().starts_with("//") {
            continue;
        }
        for col in 0..line.len() as u32 {
            checked += 1;
            if let Definition::Exact(locs) =
                definition_at(&store, &syntactic, rel, line_idx as u32, col)
            {
                wrong.push((line_idx, col, locs));
            }
        }
    }
    assert!(
        checked > 0,
        "コメント行が見つからない。対象ファイルが変わっている"
    );
    assert!(
        wrong.is_empty(),
        "コメント内の {} / {checked} 箇所が Exact を返した（先頭: {:?}）",
        wrong.len(),
        wrong.first()
    );
}

#[test]
fn 同じ範囲に2つのシンボルが乗る位置は両方返す() {
    // 構造体リテラルのフィールド初期化省略記法。1 つの語にフィールドと束縛の
    // 2 つの occurrence が同じ範囲で乗る。先に見つけたほうだけを返すと、
    // もう片方の定義が黙って消える。
    let root = workdir("two-symbols");
    let src = "pub struct S { pub v: u8 }\nfn f(v: u8) -> S { S { v } }\n";
    std::fs::write(root.join("src/lib.rs"), src).unwrap();

    let index = Index {
        metadata: MessageField::some(Metadata {
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![Document {
            relative_path: "src/lib.rs".to_string(),
            occurrences: vec![
                occurrence(vec![0, 19, 20], "demo S#v.", 1),
                occurrence(vec![1, 5, 6], "demo f().(v)", 1),
                occurrence(vec![1, 23, 24], "demo S#v.", 0),
                occurrence(vec![1, 23, 24], "demo f().(v)", 0),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let answer = definition_at(&store, &silent(), Path::new("src/lib.rs"), 1, 23);

    let Definition::Exact(mut locs) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    locs.sort_by_key(|l| (l.line, l.col));
    assert_eq!(
        locs,
        vec![
            Location {
                path: PathBuf::from("src/lib.rs"),
                line: 0,
                col: 19,
            },
            Location {
                path: PathBuf::from("src/lib.rs"),
                line: 1,
                col: 5,
            },
        ]
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_モジュールを指す語はモジュールの定義に飛ぶ() {
    // use super::GitEngine; の super。ファイル全体を覆うメモを点の答えにはしないが、
    // モジュールの定義位置としては生きている必要がある。
    let (index_path, root) = real_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let rel = Path::new("src/git_engine/worktree_create.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let (line_idx, line) = src
        .lines()
        .enumerate()
        .find(|(_, l)| l.starts_with("use super::"))
        .expect("対象の行が変わっている");
    let col = line.find("super").unwrap() as u32;

    let answer = definition_at(&store, &silent(), rel, line_idx as u32, col);

    let Definition::Exact(locs) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    assert_eq!(locs.len(), 1, "{locs:?}");
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_同じシンボルidに定義が複数あればすべて返す() {
    // 別々の関数の中に同名の const があると、rust-analyzer は同じシンボルIDを振る。
    // 先に見つけた 1 つだけを表に残すと、他の定義の上に立ったときに別の行へ飛ぶ。
    let (index_path, root) = real_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let rel = Path::new("src/ui/reflow_view/render_tests.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let defs: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("const INNER"))
        .map(|(i, _)| i)
        .collect();
    assert!(defs.len() > 1, "対象のファイルが変わっている: {defs:?}");
    let col = src.lines().nth(defs[1]).unwrap().find("INNER").unwrap() as u32;

    let answer = definition_at(&store, &silent(), rel, defs[1] as u32, col);

    let Definition::Exact(locs) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    let lines: Vec<u32> = locs.iter().map(|l| l.line).collect();
    assert_eq!(
        lines,
        defs.iter().map(|d| *d as u32).collect::<Vec<_>>(),
        "定義が取りこぼされている"
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_索引を読み直す時間を測る() {
    // worktree を切り替えると Slot は索引を捨てて読み直す。抱え続けると常駐が
    // worktree の本数に比例するので、捨てるほうを選んでいる。その代償がこの時間で、
    // 読み直しの間は構文層で答える。背景スレッドなので入力は止まらない。
    // 出自はツリーを歩き直すのではなく、生成時に書いた表から読む。歩くほうを測ると
    // 本番に無いコストが混ざる（実測でこの索引だと 95ms 対 46ms）。
    let (index_path, root) = real_index();
    let hashes = index_path.with_file_name("index.hashes");
    let expected = sheaf_core::read_provenance(&hashes, &sheaf_core::RustAnalyzer)
        .expect("SHEAF_TEST_INDEX には rust-analyzer が作った索引を渡すこと");

    let start = std::time::Instant::now();
    let store = sheaf_core::Store::load(
        &[sheaf_core::IndexSource {
            index: index_path,
            subroot: PathBuf::new(),
            expected,
        }],
        &root,
    )
    .unwrap();
    let elapsed = start.elapsed();

    println!("索引 {} Document / 読み直し {:?}", store.len(), elapsed);
    assert!(store.len() > 100, "対象が小さすぎる: {}", store.len());
    // 閾値は release で測った値。全 Document をデコードして保持する実装に退化すると
    // ここが桁で伸びる。debug では 8 倍ほどかかるので落ちる。
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "読み直しが {elapsed:?} かかった"
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_1クエリの所要時間を測る() {
    // 鮮度の検査はファイルの読み込みとハッシュを伴い、行数に比例して伸びる。
    // カーソル移動のたびに呼ばれるので数字を残す。閾値は置かず、出力を読む。
    // release と debug で 8 倍ほど違うので、比べるときはプロファイルを揃える。
    let (index_path, root) = real_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let rel = Path::new("src/git_engine/worktree_create.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let syntactic = silent();

    let mut queries = 0;
    let mut exact = 0;
    let start = std::time::Instant::now();
    for (line_idx, line) in src.lines().enumerate() {
        for col in 0..line.len() as u32 {
            queries += 1;
            if let Definition::Exact(_) =
                definition_at(&store, &syntactic, rel, line_idx as u32, col)
            {
                exact += 1;
            }
        }
    }
    let elapsed = start.elapsed();

    println!(
        "{queries} クエリ / うち Exact {exact} / 合計 {:?} / 1 クエリ {:?}",
        elapsed,
        elapsed / queries
    );
    assert!(exact > 0, "1 件も解決していない。測定になっていない");
}
