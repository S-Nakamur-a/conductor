//! TypeScript の producer (`ScipTypescript`) を通した end-to-end の検査。
//!
//! `npx` が要る。版を固定してあるので、npm キャッシュにその版があれば
//! ネットワークが無くても走る。

use sheaf_core::{
    Definition, Outcome, ScipTypescript, Span, SyntacticAnswer, SyntacticLayer, Target, Token,
    definition_at, generate_once,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 語の切り出しをわざと素朴にした構文層。フォールバックは常に NotCode を返すので、
/// 索引が答えられなかったことが `Exact` の不在としてそのまま出る。
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

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-test-ts-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `src/greet.ts` に定義を、`src/main.ts` にそこからの取り込みと呼び出しを持つ
/// 最小のツリーを `at` の下に書く。
///
/// `tsconfig.json` はフィクスチャ側で置く。producer には `--infer-tsconfig` を
/// 渡さないので、無いと索引が空になる。
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
    let root = workdir("min");
    write_two_file_project(&root);

    let outcome = generate_once(
        target_in(&root, &workdir("min-out")),
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
fn scip_typescript_は対象のツリーに何も書かない() {
    // tsconfig.json だけを見る形にすると、別の経路で書かれたときに素通りする。
    // ツリー全体を突き合わせる。
    let root = workdir("readonly");
    write_two_file_project(&root);

    let before = hash_tree(&root);
    let outcome = generate_once(
        target_in(&root, &workdir("readonly-out")),
        Arc::new(ScipTypescript),
    );
    assert!(
        matches!(outcome, Outcome::Ready { .. }),
        "生成が Ready に到達しなかった"
    );
    let after = hash_tree(&root);

    assert_eq!(before, after, "producer が対象のツリーを変えた");
}

#[test]
fn tsconfigの無いツリーでも対象に書き込まない() {
    // `--infer-tsconfig` が tsconfig.json を書き込むのは、無いときだけ。
    // 上の判定器はツリーに tsconfig.json があるので、この経路を通らない。
    //
    // 索引が作れないこと自体は許す。書き込んで作れるようにするのを許さない。
    let root = workdir("no-tsconfig");
    write_two_file_project(&root);
    std::fs::remove_file(root.join("tsconfig.json")).unwrap();

    let before = hash_tree(&root);
    let _ = generate_once(
        target_in(&root, &workdir("no-tsconfig-out")),
        Arc::new(ScipTypescript),
    );
    let after = hash_tree(&root);

    assert_eq!(before, after, "producer が対象のツリーに書き込んだ");
}
