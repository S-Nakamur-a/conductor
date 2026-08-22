//! Go の producer (`ScipGo`) を通した end-to-end の検査。
//!
//! `definition.rs` は Rust のフィクスチャで固まっているので、Go 用はこちらに分ける。
//! (a)(b) は `scip-go` があれば決定的に通る。実リポジトリの回帰値だけ `#[ignore]` にする。

use sheaf_core::{
    Definition, Outcome, ScipGo, Span, SyntacticAnswer, SyntacticLayer, Target, Token,
    definition_at, generate_once,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 語の切り出しをわざと素朴にした構文層。英数字・下線が続く範囲を 1 語とする。
/// 索引が答えられるはずの位置で構文層を呼ばずに済むかを見るためのもので、
/// フォールバック自体は常に NotCode を返す。
struct Rough;

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl SyntacticLayer for Rough {
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

    fn definition_at(&self, _path: &Path, _line: u32, _col: u32) -> SyntacticAnswer {
        SyntacticAnswer::NotCode
    }

    fn references_at(&self, _path: &Path, _line: u32, _col: u32) -> SyntacticAnswer {
        SyntacticAnswer::NotCode
    }
}

/// テスト用の作業ディレクトリ。呼び出しごとに別の場所になる。
fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-test-go-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `pkg/greet` に定義を、`cmd/app` に別パッケージからの呼び出しを持つ、
/// go.mod 込みの最小 2 パッケージ構成を `at` の下に書く。
fn write_two_package_module(at: &Path) {
    std::fs::create_dir_all(at.join("pkg/greet")).unwrap();
    std::fs::create_dir_all(at.join("cmd/app")).unwrap();
    std::fs::write(at.join("go.mod"), "module example.com/app\n\ngo 1.21\n").unwrap();
    std::fs::write(
        at.join("pkg/greet/greet.go"),
        "package greet\n\nfunc Greet() string {\n\treturn \"hi\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        at.join("cmd/app/main.go"),
        "package main\n\nimport (\n\t\"fmt\"\n\n\t\"example.com/app/pkg/greet\"\n)\n\nfunc main() {\n\tfmt.Println(greet.Greet())\n}\n",
    )
    .unwrap();
}

/// 成果物の置き場所を索引の対象と分けて渡せるようにしてある。実在するリポジトリへ
/// 向けたときに、そこへ 4 ファイル書き込んでしまうため。
fn target_in(root: &Path, artifacts: &Path) -> Target {
    Target {
        root: root.to_path_buf(),
        index: artifacts.join("index.scip"),
        hashes: artifacts.join("index.hashes"),
        log: artifacts.join("index.log"),
        lock: artifacts.join("index.lock"),
    }
}

fn target_at(root: &Path) -> Target {
    target_in(root, root)
}

#[test]
fn scip_go_は別パッケージの定義にexactで飛ぶ() {
    let root = workdir("min");
    write_two_package_module(&root);

    let outcome = generate_once(target_at(&root), Arc::new(ScipGo));
    let Outcome::Ready { store, .. } = outcome else {
        panic!("scip-go での生成が Ready に到達しなかった");
    };

    let caller_rel = Path::new("cmd/app/main.go");
    let caller_src = std::fs::read_to_string(root.join(caller_rel)).unwrap();
    let call_line = caller_src
        .lines()
        .position(|l| l.contains("greet.Greet()"))
        .expect("呼び出し行が見つからない");
    let call_col = caller_src
        .lines()
        .nth(call_line)
        .unwrap()
        .rfind("Greet")
        .unwrap() as u32;

    let def_rel = Path::new("pkg/greet/greet.go");
    let def_src = std::fs::read_to_string(root.join(def_rel)).unwrap();
    let (def_line, def_line_text) = def_src
        .lines()
        .enumerate()
        .find(|(_, l)| l.starts_with("func Greet"))
        .expect("定義行が見つからない");
    let def_col = def_line_text.find("Greet").unwrap() as u32;

    let answer = definition_at(&store, &Rough, caller_rel, call_line as u32, call_col);

    assert_eq!(
        answer,
        Definition::Exact(vec![sheaf_core::Location {
            path: def_rel.to_path_buf(),
            line: def_line as u32,
            col: def_col,
        }])
    );
}

#[test]
fn ルートを間違えると空の索引でfailedになる() {
    // go.mod をツリー直下ではなく api/ の下に置く。root をツリー直下にすると
    // scip-go は go.mod を見つけられず、Document 0 件の索引を exit 0 で書く。
    let root = workdir("wrong-root");
    write_two_package_module(&root.join("api"));

    let wrong = generate_once(target_at(&root), Arc::new(ScipGo));
    assert!(
        matches!(wrong, Outcome::Failed(_)),
        "go.mod の無いツリーで空の索引を Ready にしてしまった"
    );
    assert!(!root.join("index.scip").is_file());
    assert!(!root.join("index.hashes").is_file());

    let right = generate_once(target_at(&root.join("api")), Arc::new(ScipGo));
    assert!(
        matches!(right, Outcome::Ready { .. }),
        "go.mod のあるツリーなのに Ready にならなかった"
    );
}

// ここから先は実リポジトリが要る。SHEAF_TEST_GO_ROOT でモジュールルートを渡す。
//   cargo test -- --ignored

#[test]
#[ignore = "実リポジトリが要る"]
fn 実リポジトリ_main_goからlookup_or_panicへexactで飛ぶ() {
    let root = PathBuf::from(
        std::env::var("SHEAF_TEST_GO_ROOT")
            .expect("SHEAF_TEST_GO_ROOT に go.mod のあるモジュールルートを渡すこと"),
    );

    let outcome = generate_once(target_in(&root, &workdir("real")), Arc::new(ScipGo));
    let Outcome::Ready { store, .. } = outcome else {
        panic!("scip-go での生成が Ready に到達しなかった");
    };

    let rel = Path::new("cmd/api/main.go");
    let answer = definition_at(&store, &Rough, rel, 115, 15);

    assert_eq!(
        answer,
        Definition::Exact(vec![sheaf_core::Location {
            path: PathBuf::from("internal/pkg/envvar/lookup_or_panic.go"),
            line: 7,
            col: 5,
        }])
    );
}
