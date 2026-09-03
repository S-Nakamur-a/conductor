//! Go の producer (`ScipGo`) を通した end-to-end の検査。
//!
//! `definition.rs` は Rust のフィクスチャで固まっているので、Go 用はこちらに分ける。
//! `scip-go` があれば決定的に通る。実リポジトリの回帰値だけ `#[ignore]` にする。
//!   SHEAF_TEST_GO_ROOT=<go.mod のあるモジュールルート> cargo test --test go_definition -- --ignored

mod common;

use common::{silent, workdir};
use sheaf_core::{Definition, Location, Outcome, ScipGo, Target, definition_at, generate_once};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `cmd/app` から `pkg/greet` を別パッケージとして呼ぶ、go.mod 込みの最小構成。
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
    let root = workdir("go-min");
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

    let answer = definition_at(&store, &silent(), caller_rel, call_line as u32, call_col);

    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: def_rel.to_path_buf(),
            line: def_line as u32,
            col: def_col,
        }])
    );
}

#[test]
fn ルートを間違えると空の索引でfailedになる() {
    // go.mod をツリー直下ではなく api/ の下に置く。root をツリー直下にすると scip-go は
    // go.mod を見つけられず、Document 0 件の索引を exit 0 で書く。
    let root = workdir("go-wrong-root");
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

#[test]
#[ignore = "実リポジトリが要る"]
fn 実リポジトリ_main_goからlookup_or_panicへexactで飛ぶ() {
    let root = PathBuf::from(
        std::env::var("SHEAF_TEST_GO_ROOT")
            .expect("SHEAF_TEST_GO_ROOT に go.mod のあるモジュールルートを渡すこと"),
    );

    let outcome = generate_once(target_in(&root, &workdir("go-real")), Arc::new(ScipGo));
    let Outcome::Ready { store, .. } = outcome else {
        panic!("scip-go での生成が Ready に到達しなかった");
    };

    let answer = definition_at(&store, &silent(), Path::new("cmd/api/main.go"), 115, 15);

    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("internal/pkg/envvar/lookup_or_panic.go"),
            line: 7,
            col: 5,
        }])
    );
}
