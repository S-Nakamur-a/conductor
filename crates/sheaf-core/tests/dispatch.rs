//! インタフェース経由の参照。仕様は `docs/spec-interface-references.md` にある。
//!
//! 回帰値は実リポジトリからしか採れないので、位置を固定する検査は `#[ignore]` にする。
//!   SHEAF_TEST_GO_ROOT=<go.mod のあるモジュールルート> cargo test --test dispatch -- --ignored

use protobuf::{EnumOrUnknown, Message, MessageField};
use scip::types::{
    Document, Index, Metadata, Occurrence, Relationship, SymbolInformation, TextEncoding,
};
use sheaf_core::{
    IndexSource, Outcome, References, ScipGo, ScipTypescript, Span, Store, SyntacticAnswer,
    SyntacticLayer, Target, Token, references_at,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 語の切り出しを素朴にした構文層。索引が答えられなかったことが
/// `Exact` の不在としてそのまま出るよう、フォールバックは常に NotCode を返す。
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
        "sheaf-test-dispatch-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn target_in(root: &Path, artifacts: &Path) -> Target {
    Target {
        root: root.to_path_buf(),
        index: artifacts.join("index.scip"),
        hashes: artifacts.join("index.hashes"),
        log: artifacts.join("index.log"),
        lock: artifacts.join("index.lock"),
    }
}

// 合成した索引で見るもの（素の cargo test で走る）

const SELF: &str = "scip-test cargo demo 0.1.0 Repo#Find().";
const LIB: &str = "impl Repo { fn Find() {} }\n";
const CALLER: &str = "fn a() { Find(); }\n";

/// `Repo#Find()` が自分自身を実装していると名乗る索引を書く。
fn self_loop_index(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("src/caller.rs"), CALLER).unwrap();

    let index = Index {
        metadata: MessageField::some(Metadata {
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![
            Document {
                relative_path: "src/lib.rs".to_string(),
                occurrences: vec![Occurrence {
                    range: vec![0, 15, 19],
                    symbol: SELF.to_string(),
                    symbol_roles: 1,
                    ..Default::default()
                }],
                symbols: vec![SymbolInformation {
                    symbol: SELF.to_string(),
                    relationships: vec![Relationship {
                        symbol: SELF.to_string(),
                        is_implementation: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
            Document {
                relative_path: "src/caller.rs".to_string(),
                occurrences: vec![Occurrence {
                    range: vec![0, 9, 13],
                    symbol: SELF.to_string(),
                    symbol_roles: 0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let path = root.join("index.scip");
    std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
    path
}

#[test]
fn 自分自身を実装していると名乗る符号は経由先にしない() {
    // scip-go はインタフェース埋め込みで SRC == DST の辺を出す（実索引に 3 件）。
    // 残すと、直接参照と同じ位置がインタフェース経由にも並ぶ。
    let root = workdir("self-loop");
    let index = self_loop_index(&root);
    let expected: HashMap<PathBuf, String> = [("src/lib.rs", LIB), ("src/caller.rs", CALLER)]
        .into_iter()
        .map(|(rel, body)| (PathBuf::from(rel), sheaf_core::blob_hash(body.as_bytes())))
        .collect();
    let store = Store::load(
        &[IndexSource {
            index,
            subroot: PathBuf::new(),
            expected,
        }],
        &root,
    )
    .unwrap();

    let answer = references_at(&store, &Rough, Path::new("src/lib.rs"), 0, 15);

    let References::Exact(found) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    assert_eq!(found.direct.len(), 1);
    assert!(
        found.via_interface.is_empty(),
        "自己ループを経由先にしている: {:?}",
        found.via_interface
    );
}

const IFACE: &str = "scip-test cargo demo 0.1.0 Iface#M().";
const IMPL: &str = "scip-test cargo demo 0.1.0 Impl#M().";
const IFACE_LIB: &str = "trait Iface { fn M(); }\nimpl Iface for Impl { fn M() {} }\n";
const IFACE_CALLER: &str = "fn a(x: &dyn Iface) { x.M(); }\n";

/// `Impl#M` が `Iface#M` を実装していて、`Iface#M` の参照が別ファイルにある索引を書く。
fn interface_index(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), IFACE_LIB).unwrap();
    std::fs::write(root.join("src/caller.rs"), IFACE_CALLER).unwrap();

    let def = |range: Vec<i32>, symbol: &str| Occurrence {
        range,
        symbol: symbol.to_string(),
        symbol_roles: 1,
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![
            Document {
                relative_path: "src/lib.rs".to_string(),
                // "trait Iface { fn M(); }" の M と、2 行目の impl 側の M。
                occurrences: vec![def(vec![0, 17, 18], IFACE), def(vec![1, 25, 26], IMPL)],
                symbols: vec![SymbolInformation {
                    symbol: IMPL.to_string(),
                    relationships: vec![Relationship {
                        symbol: IFACE.to_string(),
                        is_implementation: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
            Document {
                relative_path: "src/caller.rs".to_string(),
                occurrences: vec![Occurrence {
                    range: vec![0, 24, 25],
                    symbol: IFACE.to_string(),
                    symbol_roles: 0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let path = root.join("index.scip");
    std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
    path
}

fn load(index: PathBuf, root: &Path, expected: &[(&str, &str)]) -> Store {
    let expected: HashMap<PathBuf, String> = expected
        .iter()
        .map(|(rel, body)| (PathBuf::from(rel), sheaf_core::blob_hash(body.as_bytes())))
        .collect();
    Store::load(
        &[IndexSource {
            index,
            subroot: PathBuf::new(),
            expected,
        }],
        root,
    )
    .unwrap()
}

#[test]
fn 経由先の参照が載るファイルが変わったら構文層に回る() {
    // インタフェース経由の参照先も依拠集合に入る。規則を直接参照と分けると、
    // 空が「無い」なのか「言えない」なのかを利用側が区別できなくなる。
    let root = workdir("iface-stale");
    let index = interface_index(&root);

    let fresh = load(
        index.clone(),
        &root,
        &[("src/lib.rs", IFACE_LIB), ("src/caller.rs", IFACE_CALLER)],
    );
    let answer = references_at(&fresh, &Rough, Path::new("src/lib.rs"), 1, 25);
    let References::Exact(found) = answer else {
        panic!("最新の状態で Exact が返らなかった: {answer:?}");
    };
    assert!(found.direct.is_empty());
    assert_eq!(found.via_interface.len(), 1);
    assert_eq!(found.via_interface[0].implementations, 1);

    let stale = load(
        index,
        &root,
        &[("src/lib.rs", IFACE_LIB), ("src/caller.rs", "中身が違う\n")],
    );
    assert_eq!(
        references_at(&stale, &Rough, Path::new("src/lib.rs"), 1, 25),
        References::NotCode,
        "経由先のファイルが古いのに Exact を返した"
    );
}

#[test]
fn typescript_でもインタフェース経由の参照が返る() {
    // sheaf 側に言語の分岐を書かないことを見る。scip-typescript も
    // is_implementation の辺を出すので、Go と同じ経路でそのまま通る。
    let root = workdir("ts");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        "{\n  \"compilerOptions\": {\"target\":\"ES2020\",\"module\":\"commonjs\",\"strict\":true},\n  \"include\": [\"src\"]\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/greeter.ts"),
        "export interface Greeter {\n  greet(): string;\n}\n\nexport class Polite implements Greeter {\n  greet(): string {\n    return \"hi\";\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.ts"),
        "import { Greeter, Polite } from \"./greeter\";\n\nconst g: Greeter = new Polite();\nconsole.log(g.greet());\n",
    )
    .unwrap();

    let outcome = generate(&root, Arc::new(ScipTypescript), "ts-out");
    let Outcome::Ready { store, .. } = outcome else {
        panic!("scip-typescript での生成が Ready に到達しなかった");
    };

    // Polite#greet の定義位置。直接参照は 0 件で、`g.greet()` は Greeter#greet を指す。
    let answer = references_at(&store, &Rough, Path::new("src/greeter.ts"), 5, 2);
    let References::Exact(found) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    assert!(found.direct.is_empty());
    assert_eq!(found.via_interface.len(), 1);
    let via = &found.via_interface[0];
    assert!(
        via.interface_method.as_str().ends_with("Greeter#greet()."),
        "経由先が違う: {}",
        via.interface_method.as_str()
    );
    assert_eq!(via.implementations, 1);
    assert_eq!(via.reference.path, PathBuf::from("src/main.ts"));
    assert_eq!(via.reference.line, 3);
}

fn generate(root: &Path, producer: Arc<dyn sheaf_core::Producer>, tag: &str) -> Outcome {
    sheaf_core::generate_once(target_in(root, &workdir(tag)), producer)
}

// 実リポジトリで見るもの

fn go_store(tag: &str) -> Store {
    let root = PathBuf::from(
        std::env::var("SHEAF_TEST_GO_ROOT")
            .expect("SHEAF_TEST_GO_ROOT に go.mod のあるモジュールルートを渡すこと"),
    );
    match generate(&root, Arc::new(ScipGo), tag) {
        Outcome::Ready { store, .. } => store,
        Outcome::Failed(why) | Outcome::Unavailable(why) => {
            panic!("scip-go での生成が Ready に到達しなかった: {why}")
        }
        Outcome::Busy => panic!("ほかのプロセスが索引を作っている"),
    }
}

/// 索引が答えたかどうか。答えないのは、直接参照もインタフェース経由も 0 件のとき。
fn answered(store: &Store, rel: &str, line: u32, col: u32) -> bool {
    matches!(
        references_at(store, &Rough, Path::new(rel), line, col),
        References::Exact(_)
    )
}

/// 直接参照の位置。聞いた位置が本当にその符号を指していることを示すのに使う。
fn direct(store: &Store, rel: &str, line: u32, col: u32) -> Vec<String> {
    let answer = references_at(store, &Rough, Path::new(rel), line, col);
    let References::Exact(found) = answer else {
        panic!("{rel}:{line}:{col} で Exact が返らなかった: {answer:?}");
    };
    assert!(
        found.via_interface.is_empty(),
        "{rel}:{line} がインタフェース経由を返した: {:?}",
        found.via_interface
    );
    found
        .direct
        .iter()
        .map(|l| format!("{}:{}", l.path.display(), l.line + 1))
        .collect()
}

/// メソッド定義の位置を引いて、インタフェース経由の参照を (経由先の末尾, 実装数, 位置) で返す。
fn via(store: &Store, rel: &str, line: u32, col: u32) -> (String, u32, Vec<String>) {
    let answer = references_at(store, &Rough, Path::new(rel), line, col);
    let References::Exact(found) = answer else {
        panic!("{rel}:{line}:{col} で Exact が返らなかった: {answer:?}");
    };
    let interface = found
        .via_interface
        .first()
        .map(|v| {
            v.interface_method
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default();
    let implementations = found
        .via_interface
        .first()
        .map(|v| v.implementations)
        .unwrap_or(0);
    let places = found
        .via_interface
        .iter()
        .map(|v| format!("{}:{}", v.reference.path.display(), v.reference.line + 1))
        .collect();
    (interface, implementations, places)
}

#[test]
#[ignore = "実リポジトリが要る"]
fn 実リポジトリ_インタフェース経由の参照が位置ごと一致する() {
    let store = go_store("go-hit");

    let (iface, n, places) = via(
        &store,
        "internal/pkg/exercise/visible_task_repository_test.go",
        16,
        31,
    );
    assert_eq!(iface, "TaskRepository#Find.");
    assert_eq!(n, 1);
    assert_eq!(
        places,
        vec![
            "internal/pkg/exercise/visible_task_repository.go:25",
            "internal/pkg/feedback/server.go:146",
            "internal/pkg/submit/api_proto_daily_task_activities.go:33",
            "internal/pkg/submit/api_proto_daily_task_activities.go:54",
            "internal/pkg/submit/service.go:63",
            "internal/pkg/taskquestion/server.go:133",
            "internal/pkg/teachingagent/server.go:125",
            "internal/pkg/teachingagent/server.go:247",
            "internal/pkg/teachingagent/server.go:382",
            "internal/pkg/teachingagent/server.go:452",
            "pkg/path/public/rpc/career_profile_experience_service.go:134",
        ]
    );

    let (iface, n, places) = via(
        &store,
        "pkg/prospects/public/rpc/basemachina/role_reconciliation_logger.go",
        10,
        35,
    );
    assert_eq!(iface, "ReconciliationLogger#LogInfo.");
    assert_eq!(n, 2);
    assert_eq!(
        places,
        [75, 121, 129, 137, 144, 187, 233, 241, 263, 269, 276]
            .map(|l| format!("pkg/prospects/internal/domainservice/role_reconciliation.go:{l}"))
            .to_vec()
    );

    let (iface, n, places) = via(&store, "internal/pkg/rbac/mock_role_repository.go", 16, 28);
    assert_eq!(iface, "RoleRepository#FindRole.");
    assert_eq!(n, 1);
    assert_eq!(
        places,
        vec![
            "cmd/jobs/dharma/dharma/dharma.go:61",
            "internal/pkg/httpcontroller/stripe_webhook.go:135",
            "internal/pkg/payment/service.go:498",
            "internal/pkg/rbac/role_authorizer.go:37",
            "internal/pkg/rbac/role_authorizer.go:162",
            "internal/pkg/roleownership/service.go:115",
            "internal/pkg/roleownership/service.go:146",
        ]
    );

    let (iface, n, places) = via(
        &store,
        "pkg/prospects/internal/domain/assets/infrastructure.go",
        21,
        27,
    );
    assert_eq!(iface, "Repository#SaveImage.");
    assert_eq!(n, 1);
    assert_eq!(
        places,
        vec![
            "pkg/prospects/public/rpc/base_machina.go:124",
            "pkg/prospects/public/rpc/job_posting_detail.go:2234",
            "pkg/prospects/public/rpc/organization.go:618",
            "pkg/prospects/public/rpc/organization.go:689",
            "pkg/prospects/public/rpc/organization.go:999",
        ]
    );
}

#[test]
#[ignore = "実リポジトリが要る"]
fn 実リポジトリ_実装関係の辺を持たないメソッドは経由を返さない() {
    // 直接参照を持つメソッドを選んである。持たないものだと索引が何も答えず、
    // 「経由が空」なのか「その位置を索引が知らない」のかを判定器が区別できない。
    let store = go_store("go-miss");

    assert_eq!(
        direct(
            &store,
            "internal/pkg/repository/in_memory_task_repository.go",
            50,
            41
        ),
        vec![
            "cmd/jobs/sample_job/main.go:57",
            "internal/pkg/repository/in_memory_task_repository_test.go:77",
            "internal/pkg/repository/in_memory_task_repository_test.go:102",
            "internal/pkg/repository/in_memory_task_repository_test.go:120",
        ]
    );

    assert_eq!(
        direct(
            &store,
            "internal/pkg/repository/sql_career_profile_self_promotion_repository.go",
            102,
            49
        ),
        vec![
            "internal/pkg/repository/sql_career_profile_self_promotion_repository.go:122",
            "internal/pkg/repository/sql_career_profile_self_promotion_repository.go:144",
            "internal/pkg/repository/sql_career_profile_self_promotion_repository.go:168",
        ]
    );

    // 直接参照もインタフェース経由も 0 件なら索引は答えない（構文層に回る）。
    for (rel, line, col) in [
        (
            "internal/pkg/repository/sql_career_profile_experience_repository.go",
            365,
            46,
        ),
        (
            "internal/pkg/repository/sql_career_profile_experience_repository.go",
            194,
            46,
        ),
        (
            "internal/pkg/repository/sql_career_profile_experience_skill_repository.go",
            80,
            51,
        ),
        (
            "internal/pkg/repository/sql_user_last_active_repository.go",
            25,
            37,
        ),
        (
            "internal/pkg/repository/sql_stripe_payment_intent_trace.go",
            85,
            47,
        ),
    ] {
        assert!(
            !answered(&store, rel, line, col),
            "{rel}:{line} で索引が答えてしまった"
        );
    }
}

#[test]
#[ignore = "実リポジトリが要る"]
fn 実リポジトリ_同名で別パッケージのメソッドを取り違えない() {
    // MockTaskRepository#Find() は exercise と submit の両方に実在し、
    // 前者は 11 件、後者は 0 件。接尾辞で引くと同じ答えになる。
    let store = go_store("go-samename");

    let (_, _, exercise) = via(
        &store,
        "internal/pkg/exercise/visible_task_repository_test.go",
        16,
        31,
    );
    assert_eq!(exercise.len(), 11);

    assert!(
        !answered(
            &store,
            "internal/pkg/submit/api_proto_daily_task_activities_test.go",
            19,
            37
        ),
        "submit 側にも exercise 側の答えが付いた"
    );
}
