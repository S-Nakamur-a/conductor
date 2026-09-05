//! 定義ジャンプの受け入れ条件。
//!
//! 実索引 (12MB) を要求するものは `#[ignore]` にして、環境変数で場所を渡す。
//!   SHEAF_TEST_INDEX=<.scip> SHEAF_TEST_ROOT=<ツリー> cargo test --test definition -- --ignored

mod common;

use common::{
    Rough, doc, hashes_of, index, load_as_indexed_tree, load_one, real_index, silent, source,
    workdir_with_src,
};
use scip::types::TextEncoding;
use sheaf_core::{Definition, Location, Store, SyntacticAnswer, definition_at};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SOURCE: &str = "pub fn greet() {}\nfn caller() { greet(); }\n";
const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";

fn build_index(tag: &str, encoding: i32) -> (PathBuf, PathBuf) {
    let root = workdir_with_src(tag);
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();

    let index_path = index()
        .rooted_at(&root)
        .encoding(encoding)
        .add(
            doc("src/lib.rs")
                .lang("rust")
                .def([0, 7, 12], SYMBOL)
                .reference([1, 14, 19], SYMBOL),
        )
        .write(&root.join("index.scip"));
    (index_path, root)
}

/// 参照側を変えずに定義側だけ変えられるよう、定義と参照を別ファイルに置く。
fn build_two_file_index(tag: &str) -> (PathBuf, PathBuf) {
    let root = workdir_with_src(tag);
    std::fs::write(root.join("src/lib.rs"), "pub fn greet() {}\n").unwrap();
    std::fs::write(root.join("src/caller.rs"), "fn caller() { greet(); }\n").unwrap();

    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .add(doc("src/lib.rs").lang("rust").def([0, 7, 12], SYMBOL))
        .add(
            doc("src/caller.rs")
                .lang("rust")
                .reference([0, 14, 19], SYMBOL),
        )
        .write(&root.join("index.scip"));
    (index_path, root)
}

fn utf8_index(tag: &str) -> (PathBuf, PathBuf) {
    build_index(tag, TextEncoding::UTF8 as i32)
}

// 確信度を無視して位置を取れないことは型の性質なので doctest 側で見ている（cargo test --doc）。
// ここで見るのは、実際の呼び出しでそれが成り立つこと。

#[test]
fn 索引が答えられる位置は語のどの列から聞いても_exact_を返す() {
    let (index_path, root) = utf8_index("exact");
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let want = Definition::Exact(vec![Location {
        path: PathBuf::from("src/lib.rs"),
        line: 0,
        col: 7,
    }]);

    for (why, col) in [("語の先頭", 14), ("語の途中", 17)] {
        let syntactic = silent();
        let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, col);
        assert_eq!(answer, want, "{why}");
        assert!(
            syntactic.calls().is_empty(),
            "{why}: 索引が答えたのに構文層が呼ばれている"
        );
    }
}

#[test]
fn 索引が空なら構文層の答えを返す() {
    let root = workdir_with_src("noindex");
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();
    let index_path = index().write(&root.join("empty.scip"));

    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    assert!(store.is_empty());
    let syntactic = Rough::new(SyntacticAnswer::Found(vec![Location {
        path: PathBuf::from("src/lib.rs"),
        line: 0,
        col: 7,
    }]));

    let answer = definition_at(&store, &syntactic, Path::new("src/lib.rs"), 1, 14);

    assert!(matches!(answer, Definition::Syntactic(_)), "{answer:?}");
    assert_eq!(syntactic.calls().len(), 1, "構文層が呼ばれていない");
}

#[test]
fn 索引が答えられない位置は構文層に回る() {
    // Exact は「依拠したファイルがすべて索引生成時のまま」という主張なので、
    // 飛び先が変わっただけでも答えを丸ごと捨てる。「知らない」も「変わっていない」に丸めない。
    let gap = utf8_index("gap");
    let gap_store = load_as_indexed_tree(&gap.0, &gap.1).unwrap();

    let changed = utf8_index("changed");
    let changed_store = load_as_indexed_tree(&changed.0, &changed.1).unwrap();
    std::fs::write(
        changed.1.join("src/lib.rs"),
        "pub fn hello() {}\nfn caller() { greet(); }\n",
    )
    .unwrap();

    let bare = utf8_index("no-provenance");
    let bare_store = load_one(&bare.0, &bare.1, HashMap::new()).unwrap();

    let target = build_two_file_index("target-changed");
    let target_store = load_as_indexed_tree(&target.0, &target.1).unwrap();
    std::fs::write(
        target.1.join("src/lib.rs"),
        "// 定義を消した\npub fn greet() {}\n",
    )
    .unwrap();

    let unknown = utf8_index("unknown");
    let unknown_store = load_as_indexed_tree(&unknown.0, &unknown.1).unwrap();

    for (why, store, rel, line, col) in [
        ("occurrence の無い位置", &gap_store, "src/lib.rs", 1, 3),
        (
            "聞かれた側のファイルが変わった",
            &changed_store,
            "src/lib.rs",
            1,
            14,
        ),
        (
            "出自を申告されていないファイル",
            &bare_store,
            "src/lib.rs",
            1,
            14,
        ),
        (
            "飛び先のファイルが変わった",
            &target_store,
            "src/caller.rs",
            0,
            14,
        ),
        (
            "構文層が語を判定できない",
            &unknown_store,
            "src/missing.rs",
            1,
            14,
        ),
    ] {
        let syntactic = silent();
        let answer = definition_at(store, &syntactic, Path::new(rel), line, col);
        assert_eq!(answer, Definition::NotCode, "{why}");
        assert_eq!(syntactic.calls().len(), 1, "{why}: 構文層に回っていない");
    }
}

#[test]
fn 別のツリーでは内容が一致したファイルだけ_exact_を返す() {
    // worktree の形。ロード時にディスクを読んで期待値にすると、編集済みのファイルも
    // 「そのまま」と判定されて索引の言う古い行が Exact で返る。かといって全部落とすと、
    // worktree で索引を使い回す意味が無くなる。
    let (index_path, indexed) = build_two_file_index("worktree-pair");
    let expected = hashes_of(&indexed, &["src/lib.rs", "src/caller.rs"]);

    // 聞く側の caller.rs はどちらのツリーでも同じにしてある。ここが違うと、位置が
    // occurrence に当たらないというだけの理由で答えが消え、鮮度の検査を通り抜ける。
    let moved = workdir_with_src("worktree-moved");
    std::fs::write(
        moved.join("src/lib.rs"),
        "// 別のツリー\npub fn greet() {}\n",
    )
    .unwrap();
    std::fs::write(moved.join("src/caller.rs"), "fn caller() { greet(); }\n").unwrap();

    let same = workdir_with_src("worktree-same");
    std::fs::write(same.join("src/lib.rs"), "pub fn greet() {}\n").unwrap();
    std::fs::write(same.join("src/caller.rs"), "fn caller() { greet(); }\n").unwrap();

    for (why, root, want) in [
        ("飛び先が編集されたツリー", &moved, Definition::NotCode),
        (
            "内容が一致したツリー",
            &same,
            Definition::Exact(vec![Location {
                path: PathBuf::from("src/lib.rs"),
                line: 0,
                col: 7,
            }]),
        ),
    ] {
        let store = load_one(&index_path, root, expected.clone()).unwrap();
        let answer = definition_at(&store, &silent(), Path::new("src/caller.rs"), 0, 14);
        assert_eq!(answer, want, "{why}");
    }
}

#[test]
fn 出自の申告が無い_document_の数を数える() {
    let (index_path, indexed) = build_two_file_index("missing-provenance");
    let expected = hashes_of(&indexed, &["src/lib.rs"]);

    let store = load_one(&index_path, &indexed, expected).unwrap();

    assert_eq!(store.missing_provenance(), 1);
}

#[test]
fn 候補の一部だけが古いときは部分的な_exact_を返さない() {
    // 新しいほうだけ返すと、消えた候補があることを呼び出し側が知れないまま
    // 「索引がすべて答えた」と読まれる。
    let root = workdir_with_src("partial");
    std::fs::write(root.join("src/a.rs"), "pub struct A;\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "pub struct B;\n").unwrap();
    std::fs::write(root.join("src/use.rs"), "fn f() { name; }\n").unwrap();

    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .add(doc("src/a.rs").def([0, 11, 12], "sym/A#"))
        .add(doc("src/b.rs").def([0, 11, 12], "sym/B#"))
        .add(
            doc("src/use.rs")
                .reference([0, 9, 13], "sym/A#")
                .reference([0, 9, 13], "sym/B#"),
        )
        .write(&root.join("index.scip"));
    let store = load_as_indexed_tree(&index_path, &root).unwrap();

    let both = definition_at(&store, &silent(), Path::new("src/use.rs"), 0, 9);
    assert!(
        matches!(&both, Definition::Exact(l) if l.len() == 2),
        "{both:?}"
    );

    std::fs::write(root.join("src/b.rs"), "// 動かした\npub struct B;\n").unwrap();
    let syntactic = silent();
    let partial = definition_at(&store, &syntactic, Path::new("src/use.rs"), 0, 9);

    assert_eq!(partial, Definition::NotCode, "部分的な Exact を返している");
    assert_eq!(syntactic.calls().len(), 1, "構文層に回っていない");
}

#[test]
fn ツリーの外を指す相対パスの_document_は投入しない() {
    // 索引ファイルは外から来る入力。Path::join は絶対パスを渡されるとルートを捨てるので、
    // 検査しないとツリー外のファイルを読んで、その位置を答えとして返してしまう。
    let root = workdir_with_src("escape");
    let mut builder = index().rooted_at(&root).utf8();
    for rel in ["/etc/passwd", "../../outside.rs", "src/ok.rs"] {
        builder = builder.add(doc(rel).lang("rust").def([0, 0, 4], SYMBOL));
    }
    let index_path = builder.write(&root.join("index.scip"));

    let store = load_as_indexed_tree(&index_path, &root).unwrap();

    assert_eq!(store.len(), 1, "ツリー外の Document が投入されている");
}

#[test]
fn document_側のエンコーディング宣言を読む() {
    // metadata は UTF-8、Document は UTF-16 と言っている索引。greet の手前に非 ASCII が
    // あるので数え方がここでずれる (バイト 17、UTF-16 15)。Document 側のフィールド番号を
    // 読み違えて未指定扱いにすると、この occurrence は除外されて Exact が返らなくなる。
    let root = workdir_with_src("docenc");
    std::fs::write(root.join("src/lib.rs"), "/* あ */ pub fn greet() {}\n").unwrap();
    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .add(
            doc("src/lib.rs")
                .lang("rust")
                .utf16_positions()
                .def([0, 15, 20], SYMBOL),
        )
        .write(&root.join("index.scip"));

    let store = load_as_indexed_tree(&index_path, &root).unwrap();

    // バイト 18 は "greet" の中（変換後のバイト範囲は 17..22）。
    let answer = definition_at(&store, &silent(), Path::new("src/lib.rs"), 0, 18);

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
fn utf16_を宣言する索引は投入しない() {
    // 弾く理由は列の数え方ではなく、metadata.text_document_encoding がディスク上の
    // ファイルを UTF-16 だと申告していること (バイト列として安全に読めない)。
    let (index_path, root) = build_index("utf16", TextEncoding::UTF16 as i32);
    let err = load_as_indexed_tree(&index_path, &root).unwrap_err();
    assert!(
        matches!(err, sheaf_core::SheafError::UnsupportedEncoding { .. }),
        "{err:?}"
    );
}

/// scip-typescript の実際の出力を模した索引。Document.position_encoding は未指定のまま
/// (実物どおり) で、occurrence の range は実測した UTF-16 コードユニット値。
fn build_shift_index() -> (PathBuf, PathBuf) {
    let root = workdir_with_src("shift");
    std::fs::write(
        root.join("src/shift.ts"),
        "export const あいうえおかきく = 1\n\
         export const aVeryLongIdentifier = 2\n\
         export const t2 = 3\n\
         export const z = あいうえおかきく + aVeryLongIdentifier + t2\n",
    )
    .unwrap();

    let sym = |name: &str| format!("scip-typescript npm app 0.0.1 src/`shift.ts`/{name}.");
    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .add(
            doc("src/shift.ts")
                .def([0, 13, 21], &sym("`あいうえおかきく`"))
                .def([1, 13, 32], &sym("aVeryLongIdentifier"))
                .def([2, 13, 15], &sym("t2"))
                .def([3, 13, 14], &sym("z"))
                .reference([3, 17, 25], &sym("`あいうえおかきく`"))
                .reference([3, 28, 47], &sym("aVeryLongIdentifier"))
                .reference([3, 50, 52], &sym("t2")),
        )
        .write(&root.join("index.scip"));
    (index_path, root)
}

/// 同じく scip-typescript の出力を模した jp.ts 相当の索引。
fn build_jp_index() -> (PathBuf, PathBuf) {
    let root = workdir_with_src("jpts");
    std::fs::write(
        root.join("src/jp.ts"),
        "import {topLevel} from 'dep'\n\
         \n\
         export const 説明 = 'あ'\n\
         // コメント: 日本語のあとに識別子が来る行\n\
         export const messageJa = `パブリックリポジトリ数: ${topLevel(1)}`\n\
         export const plain = topLevel(2)\n",
    )
    .unwrap();

    let sym = |name: &str| format!("scip-typescript npm app 0.0.1 src/`jp.ts`/{name}.");
    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .add(
            doc("src/jp.ts")
                .def([2, 13, 15], &sym("`説明`"))
                .def([4, 13, 22], &sym("messageJa"))
                .reference(
                    [4, 41, 49],
                    "scip-typescript npm dep 2.0.0 `index.d.ts`/topLevel().",
                )
                .def([5, 13, 18], &sym("plain")),
        )
        .write(&root.join("index.scip"));
    (index_path, root)
}

#[test]
fn 未指定エンコーディングでは手前に非_ascii_がある語だけ無回答になる() {
    // 列が UTF-16 単位なのに Document がそれを宣言しないので、聞かれた位置より手前に
    // 非 ASCII があるとバイト位置とずれる。ずれた範囲で他の符号に重なるより無回答を選ぶ。
    // 見るのは開始位置より手前だけで、行全体ではない。
    let (shift_index, shift_root) = build_shift_index();
    let shift = load_as_indexed_tree(&shift_index, &shift_root).unwrap();
    let (jp_index, jp_root) = build_jp_index();
    let jp = load_as_indexed_tree(&jp_index, &jp_root).unwrap();

    let exact = |rel: &str, line: u32, col: u32| {
        Definition::Exact(vec![Location {
            path: PathBuf::from(rel),
            line,
            col,
        }])
    };

    for (why, store, rel, line, col, want) in [
        (
            "aVeryLongIdentifier の参照は手前に日本語がある",
            &shift,
            "src/shift.ts",
            3,
            44,
            Definition::NotCode,
        ),
        (
            "t2 の参照も手前に日本語がある",
            &shift,
            "src/shift.ts",
            3,
            66,
            Definition::NotCode,
        ),
        (
            "aVeryLongIdentifier の定義行は手前が ASCII だけ",
            &shift,
            "src/shift.ts",
            1,
            13,
            exact("src/shift.ts", 1, 13),
        ),
        (
            "messageJa の定義は行の後方に非 ASCII があっても手前は ASCII だけ",
            &jp,
            "src/jp.ts",
            4,
            15,
            exact("src/jp.ts", 4, 13),
        ),
        (
            "説明 は識別子自体が非 ASCII でも手前は ASCII だけ",
            &jp,
            "src/jp.ts",
            2,
            14,
            exact("src/jp.ts", 2, 13),
        ),
        (
            "テンプレート内の topLevel は手前に日本語がある",
            &jp,
            "src/jp.ts",
            4,
            63,
            Definition::NotCode,
        ),
    ] {
        let answer = definition_at(store, &silent(), Path::new(rel), line, col);
        assert_eq!(answer, want, "{why}");
    }
}

#[test]
fn 未指定エンコーディングで手前が_ascii_の語は末尾が縮んでも誤答しない() {
    // あいうえおかきく への参照は手前が ASCII なので通過するが、宣言された範囲 [17,25] は
    // UTF-16 単位のままで、真のバイト範囲 [17,41] より 8 文字ぶん短い。縮んだ範囲でも
    // 真の語のどの位置から聞いても同じ定義に解決し、他の符号を指さないことを見る。
    let (index_path, root) = build_shift_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let want = Definition::Exact(vec![Location {
        path: PathBuf::from("src/shift.ts"),
        line: 0,
        col: 13,
    }]);

    for col in 17..41u32 {
        let answer = definition_at(&store, &silent(), Path::new("src/shift.ts"), 3, col);
        assert_eq!(answer, want, "col={col} で誤答または無回答になった");
    }
}

#[test]
fn 同じ範囲に2つのシンボルが乗る位置は両方返す() {
    // 構造体リテラルのフィールド初期化省略記法。1 つの語にフィールドと束縛の 2 つの
    // occurrence が同じ範囲で乗る。先に見つけたほうだけを返すと、もう片方が黙って消える。
    let root = workdir_with_src("two-symbols");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub struct S { pub v: u8 }\nfn f(v: u8) -> S { S { v } }\n",
    )
    .unwrap();

    let index_path = index()
        .utf8()
        .add(
            doc("src/lib.rs")
                .def([0, 19, 20], "demo S#v.")
                .def([1, 5, 6], "demo f().(v)")
                .reference([1, 23, 24], "demo S#v.")
                .reference([1, 23, 24], "demo f().(v)"),
        )
        .write(&root.join("index.scip"));

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
fn 実索引_format_のインライン引数は構文層に回る() {
    let (index_path, root) = real_index();
    let store = load_as_indexed_tree(&index_path, &root).unwrap();
    let syntactic = silent();

    // let remote = format!("origin/{main_branch}"); の main_branch。
    // SCIP はこの位置に occurrence を持たないが、rust-analyzer は定義を返す。
    let rel = Path::new("src/git_engine/worktree_create.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let line_idx = 215;
    let line = src.lines().nth(line_idx).unwrap();
    let col = line.find("main_branch").expect("対象の行が変わっている") as u32;

    // 対照。これが無いと「索引が古いから NotCode」でもテストが通ってしまう。
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
    // worktree を切り替えると索引を捨てて読み直す。抱え続けると常駐が worktree の
    // 本数に比例するので捨てるほうを選んでいて、その代償がこの時間になる。出自は
    // ツリーを歩き直さず生成時の表から読む (歩くと実測で 95ms 対 46ms)。
    let (index_path, root) = real_index();
    let expected = sheaf_core::read_provenance(
        &index_path.with_file_name("index.hashes"),
        &sheaf_core::RustAnalyzer,
    )
    .expect("SHEAF_TEST_INDEX には rust-analyzer が作った索引を渡すこと");

    let start = std::time::Instant::now();
    let store = Store::load(&[source(&index_path, "", expected)], &root).unwrap();
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
    // 鮮度の検査はファイルの読み込みとハッシュを伴う。カーソル移動のたびに呼ばれるので
    // 数字を残す。閾値は置かず出力を読む。release と debug で 8 倍ほど違う。
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
