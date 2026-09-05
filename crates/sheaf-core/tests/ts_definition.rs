//! TypeScript の producer (`ScipTypescript`) を通した end-to-end の検査。
//!
//! `npx` が要る。版を固定してあるので、npm キャッシュにその版があれば
//! ネットワークが無くても走る。

mod common;

use common::{silent, workdir};
use sheaf_core::{
    Definition, Location, Outcome, ScipTypescript, Target, definition_at, generate_once,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// producer には `--infer-tsconfig` を渡さないので、`tsconfig.json` が無いと索引が空になる。
fn write_two_file_project(at: &Path) {
    std::fs::create_dir_all(at.join("src")).unwrap();
    std::fs::write(
        at.join("package.json"),
        "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        at.join("tsconfig.json"),
        "{\n  \"compilerOptions\": {\n    \"target\": \"ES2020\",\n    \"module\": \"commonjs\",\n    \"strict\": true\n  },\n  \"include\": [\"src\"]\n}\n",
    )
    .unwrap();
    std::fs::write(
        at.join("src/greet.ts"),
        "export function greet(): string {\n  return \"hi\";\n}\n",
    )
    .unwrap();
    std::fs::write(
        at.join("src/main.ts"),
        "import { greet } from \"./greet\";\n\nconsole.log(greet());\n",
    )
    .unwrap();
}

/// 成果物の置き場所を索引の対象と分けて渡せるようにしてある。対象のツリーに
/// 何も書かないことを見る判定器が、sheaf 自身の成果物で汚れないようにするため。
fn target_in(root: &Path, artifacts: &Path) -> Target {
    Target {
        root: root.to_path_buf(),
        index: artifacts.join("index.scip"),
        hashes: artifacts.join("index.hashes"),
        log: artifacts.join("index.log"),
        lock: artifacts.join("index.lock"),
    }
}

/// ツリーの下にある全ファイルの内容ハッシュ。gitignore で絞らないので、
/// `node_modules` のような無視される場所への書き込みも見える。
fn hash_tree(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file()
                && let Ok(content) = std::fs::read(entry.path())
                && let Ok(rel) = entry.path().strip_prefix(root)
            {
                out.insert(rel.to_path_buf(), sheaf_core::blob_hash(&content));
            }
        }
    }
    out
}

#[test]
fn scip_typescript_は別ファイルの定義にexactで飛ぶ() {
    let root = workdir("ts-min");
    write_two_file_project(&root);

    let outcome = generate_once(
        target_in(&root, &workdir("ts-min-out")),
        Arc::new(ScipTypescript),
    );
    let Outcome::Ready { store, .. } = outcome else {
        panic!("scip-typescript での生成が Ready に到達しなかった");
    };

    let caller_rel = Path::new("src/main.ts");
    let caller_src = std::fs::read_to_string(root.join(caller_rel)).unwrap();
    let call_line = caller_src
        .lines()
        .position(|l| l.contains("console.log"))
        .expect("呼び出し行が見つからない");
    let call_col = caller_src
        .lines()
        .nth(call_line)
        .unwrap()
        .rfind("greet")
        .unwrap() as u32;

    let def_rel = Path::new("src/greet.ts");
    let def_src = std::fs::read_to_string(root.join(def_rel)).unwrap();
    let (def_line, def_line_text) = def_src
        .lines()
        .enumerate()
        .find(|(_, l)| l.starts_with("export function"))
        .expect("定義行が見つからない");
    let def_col = def_line_text.find("greet").unwrap() as u32;

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
fn scip_typescript_は対象のツリーに何も書かない() {
    // tsconfig.json だけを見る形にすると、別の経路で書かれたときに素通りする。
    // ツリー全体を突き合わせる。
    //
    // tsconfig.json が無いときは `--infer-tsconfig` がそれを書き込む経路があるので、
    // 索引が作れるかどうかとは別に、両方の入口で見る。索引が作れないこと自体は許す。
    for (why, tsconfig, expect_ready) in [
        ("tsconfig.json がある", true, true),
        ("tsconfig.json が無い", false, false),
    ] {
        let tag = if tsconfig { "ts-ro" } else { "ts-ro-none" };
        let root = workdir(tag);
        write_two_file_project(&root);
        if !tsconfig {
            std::fs::remove_file(root.join("tsconfig.json")).unwrap();
        }

        let before = hash_tree(&root);
        let outcome = generate_once(
            target_in(&root, &workdir(&format!("{tag}-out"))),
            Arc::new(ScipTypescript),
        );
        let after = hash_tree(&root);

        if expect_ready {
            assert!(
                matches!(outcome, Outcome::Ready { .. }),
                "{why}: 生成が Ready に到達しなかった"
            );
        }
        assert_eq!(before, after, "{why}: producer が対象のツリーを変えた");
    }
}
