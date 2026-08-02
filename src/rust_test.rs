//! 開いている *.rs ファイルの中から、実行可能な Rust のテストを検出する。
//!
//! [crate::go_test] の Rust 版。Go は平坦な規約 (func TestXxx) を持つのに対し、
//! Rust は #[test] / #[tokio::test] の関数を #[cfg(test)] mod ブロックの中に
//! 入れ子にし、テストハーネスは各テストを完全なモジュールパス
//! (<ファイルのモジュール>::<内側のモジュール群>::<関数>、例:
//! ai_caller::tests::command::echoes_prompt_via_stdin) で名指しする。正確に
//! 絞り込むにはファイルのモジュールパス (ソースツリー上の位置から決まる) と、
//! ファイル内の mod の入れ子の両方が必要になる。そのためこのスキャナは行単位の
//! 正規表現ではなく tree-sitter-rust でファイルをパースする。
//!
//! 1 始まりの行番号から、そのスコープを実行する cargo test コマンドを表す
//! [TestRun] へのマップを作る。出すボタンは 3 種類:
//!
//! - File: 1 行目。ファイル内の全テスト (cargo test '<mod>::')。
//! - Func: テストの fn の各行。そのテスト 1 つだけ
//!   (cargo test '<完全なパス>' -- --exact)。
//! - Module: (間接的にでも) テストを含む mod の各行。その配下の全テスト
//!   (cargo test '<mod のパス>::')。
//!
//! コマンドは Shell の PTY の作業ディレクトリがクレートルート (worktree ルート)
//! であることを前提にする。go_test が go test について置いている前提と同じ。

use std::collections::HashMap;

use crate::test_run::{TestRun, TestRunKind, shell_single_quote};

/// ファイルが cargo test のターゲットとモジュールパスの接頭辞にどう対応するか。
enum FileTarget {
    /// 既定の (bin / lib) ターゲットにコンパイルされる単体テスト。prefix は
    /// ファイルのモジュールパス (クレートルート、つまり main.rs / lib.rs では空)。
    Unit { prefix: Vec<String> },
    /// トップレベルの tests/<name>.rs の結合テストバイナリ。--test <name> で
    /// 選択する。その中のテストパスにはファイルのモジュール接頭辞が付かない。
    Integration { name: String },
}

/// 開いているファイルの内容から実行可能な Rust のテストを走査する。
///
/// relative_path が対応対象の .rs ファイルでない (src/ とトップレベルの
/// tests/ の外にある) か、テストを含まない場合は空のマップを返す。
pub fn scan_rust_test_runs(file_content: &[String], relative_path: &str) -> HashMap<usize, TestRun> {
    let mut runs = HashMap::new();

    let Some(target) = file_target(relative_path) else {
        return runs;
    };

    // パーサ向けにソーステキストを組み直す。file_content はファイルの論理行
    // (タブは既に空白へ展開済み) なので、'\n' で連結すれば同等の行と桁の構造が
    // 再現される。tree-sitter の行は file_content の添字と 1 対 1 に対応し、
    // 識別子はタブ展開の影響を受けない。
    let source = file_content.join("\n");

    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return runs;
    }
    let Some(tree) = parser.parse(&source, None) else {
        return runs;
    };

    let ctx = Ctx { target };
    let mut mod_stack: Vec<String> = Vec::new();
    let found = scan_node(tree.root_node(), &source, &mut mod_stack, &ctx, &mut runs);

    // 1 行目にファイル単位のボタンを置くが、たまたま 1 行目にある本物の
    // 関数・モジュールのボタンは決して潰さない (or_insert)。
    if found {
        runs.entry(1).or_insert_with(|| TestRun {
            kind: TestRunKind::File,
            label: file_label(relative_path),
            command: ctx.file_command(),
        });
    }

    runs
}

/// ファイルごとのコマンド組み立て用の文脈。
struct Ctx {
    target: FileTarget,
}

impl Ctx {
    /// 結合テストバイナリ向けの --test <name> セレクタ。既定のターゲットなら空。
    /// cargo test{...} にそのまま差し込めるよう先頭に空白を含む。
    fn test_flag(&self) -> String {
        match &self.target {
            FileTarget::Integration { name } => format!(" --test {}", shell_single_quote(name)),
            FileTarget::Unit { .. } => String::new(),
        }
    }

    /// ファイル内の全テストパスの前に付く、ファイルのモジュールパスの各セグメント
    /// (クレートルートと結合テストバイナリでは空)。
    fn file_prefix(&self) -> &[String] {
        match &self.target {
            FileTarget::Unit { prefix } => prefix,
            FileTarget::Integration { .. } => &[],
        }
    }

    /// テスト 1 つに対するハーネス上の完全なパス:
    /// <ファイルの接頭辞>::<内側のモジュール群>::<関数>。
    fn full_path(&self, mod_stack: &[String], name: &str) -> String {
        let mut segs: Vec<&str> = self.file_prefix().iter().map(String::as_str).collect();
        segs.extend(mod_stack.iter().map(String::as_str));
        segs.push(name);
        segs.join("::")
    }

    /// mod_stack (そのモジュール自身を既に含む) に対するモジュール絞り込みの
    /// フィルタ。連結したパスの末尾に :: を付け、部分一致がそのモジュールの
    /// 子孫に限られるようにする。
    fn module_prefix(&self, mod_stack: &[String]) -> String {
        let mut segs: Vec<&str> = self.file_prefix().iter().map(String::as_str).collect();
        segs.extend(mod_stack.iter().map(String::as_str));
        if segs.is_empty() {
            String::new()
        } else {
            format!("{}::", segs.join("::"))
        }
    }

    /// cargo test '<完全なパス>' -- --exact。このテスト 1 つだけを実行する。
    fn func_command(&self, full_path: &str) -> String {
        format!(
            "cargo test{} {} -- --exact",
            self.test_flag(),
            shell_single_quote(full_path)
        )
    }

    /// cargo test '<接頭辞>::'。このモジュール配下の全テストを実行する。
    fn module_command(&self, module_prefix: &str) -> String {
        format!(
            "cargo test{} {}",
            self.test_flag(),
            shell_single_quote(module_prefix)
        )
    }

    /// ファイル内の全テストを実行する。ファイルのモジュール接頭辞で絞るか、
    /// 結合テストバイナリなら --test <name> を使うか、クレートルート
    /// (main.rs / lib.rs) なら絞り込み無し。
    fn file_command(&self) -> String {
        let prefix = self.module_prefix(&[]);
        if prefix.is_empty() {
            format!("cargo test{}", self.test_flag())
        } else {
            format!(
                "cargo test{} {}",
                self.test_flag(),
                shell_single_quote(&prefix)
            )
        }
    }
}

/// node の直下の子を辿り、関数とモジュールのボタンを出しつつ mod の本体へ
/// 再帰する。この部分木でテストが見つかったかどうかを返す (mod は自分の
/// Module ボタンを描くかどうか、呼び出し側は File ボタンを描くかどうかを
/// これで判断する)。
fn scan_node(
    node: tree_sitter::Node,
    source: &str,
    mod_stack: &mut Vec<String>,
    ctx: &Ctx,
    runs: &mut HashMap<usize, TestRun>,
) -> bool {
    let mut found = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if is_test_fn(child, source)
                    && let Some(name_node) = child.child_by_field_name("name")
                {
                    found = true;
                    let name = node_text(name_node, source).to_string();
                    let line = child.start_position().row + 1;
                    let command = ctx.func_command(&ctx.full_path(mod_stack, &name));
                    runs.entry(line).or_insert(TestRun {
                        kind: TestRunKind::Func,
                        label: name,
                        command,
                    });
                }
            }
            "mod_item" => {
                let Some(name_node) = child.child_by_field_name("name") else {
                    continue;
                };
                let name = node_text(name_node, source).to_string();
                mod_stack.push(name.clone());
                let sub_found = match child.child_by_field_name("body") {
                    Some(body) => scan_node(body, source, mod_stack, ctx, runs),
                    None => false, // mod foo; (外部ファイル) はここに本体を持たない。
                };
                if sub_found {
                    found = true;
                    let line = child.start_position().row + 1;
                    let command = ctx.module_command(&ctx.module_prefix(mod_stack));
                    runs.entry(line).or_insert(TestRun {
                        kind: TestRunKind::Module,
                        label: name,
                        command,
                    });
                }
                mod_stack.pop();
            }
            _ => {}
        }
    }
    found
}

/// function_item がテスト属性を持つかどうか。tree-sitter-rust では属性は前方の
/// 兄弟である attribute_item ノードだが、一部の文法バージョンでは子として
/// 入れ子にもなるので両方を調べる。
fn is_test_fn(fn_node: tree_sitter::Node, source: &str) -> bool {
    // 前方の兄弟: fn の直上にある #[…] の行。あいだにコメントが挟まることもある。
    let mut sib = fn_node.prev_sibling();
    while let Some(s) = sib {
        match s.kind() {
            "attribute_item" => {
                if attr_is_test(s, source) {
                    return true;
                }
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        sib = s.prev_sibling();
    }

    // 保険: function_item の内側に入れ子になった attribute_item も見る。
    let mut cursor = fn_node.walk();
    for child in fn_node.children(&mut cursor) {
        if child.kind() == "attribute_item" && attr_is_test(child, source) {
            return true;
        }
    }
    false
}

/// attribute_item のパスがテストを示すかどうか。:: で区切った最後の
/// セグメントが test (#[test], #[tokio::test], #[async_std::test],
/// #[actix_web::test] など) であるか、test で終わらない有名なテストマクロの
/// 小さな許可リストに載っているか。#[cfg(test)] は正しく除外される
/// (パスのセグメントが cfg なので)。
fn attr_is_test(attr_item: tree_sitter::Node, source: &str) -> bool {
    let text = node_text(attr_item, source).trim();
    // 外側の #[ … ] を剥がす。内側属性の #![ … ] は無視する。
    let inner = match text.strip_prefix("#[").and_then(|s| s.strip_suffix(']')) {
        Some(inner) => inner.trim(),
        None => return false,
    };
    // 引数リストを落とす: cfg(test) → cfg、test → test。
    let path = inner.split('(').next().unwrap_or("").trim();
    let last = path.rsplit("::").next().unwrap_or("").trim();
    last == "test" || matches!(path, "rstest")
}

/// ファイルのリポジトリ相対パスから cargo test のターゲットとモジュール接頭辞を
/// 導く。対応していない位置なら None (実行ボタンを出さない)。
fn file_target(relative_path: &str) -> Option<FileTarget> {
    let stem = relative_path.strip_suffix(".rs")?;

    if let Some(rest) = stem.strip_prefix("src/") {
        // クレートルート: 単体テストのパスにモジュール接頭辞は付かない。
        if rest == "main" || rest == "lib" {
            return Some(FileTarget::Unit { prefix: Vec::new() });
        }
        let mut prefix: Vec<String> = rest.split('/').map(str::to_string).collect();
        // foo/mod.rs はモジュール foo であって foo::mod ではない。
        if prefix.last().map(String::as_str) == Some("mod") {
            prefix.pop();
        }
        return Some(FileTarget::Unit { prefix });
    }

    if let Some(rest) = stem.strip_prefix("tests/") {
        // 独立したバイナリになるのはトップレベルの tests/<name>.rs だけ。その下に
        // 入れ子になったファイルは共有のサブモジュールで、単独では実行できない。
        if rest.contains('/') {
            return None;
        }
        return Some(FileTarget::Integration {
            name: rest.to_string(),
        });
    }

    None
}

fn file_label(relative_path: &str) -> String {
    relative_path
        .rsplit_once('/')
        .map(|(_, f)| f)
        .unwrap_or(relative_path)
        .to_string()
}

fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<String> {
        src.lines().map(str::to_string).collect()
    }

    #[test]
    fn non_rust_file_yields_nothing() {
        let src = lines("fn main() {}\n#[test]\nfn t() {}");
        assert!(scan_rust_test_runs(&src, "README.md").is_empty());
    }

    #[test]
    fn file_outside_src_or_tests_yields_nothing() {
        let src = lines("#[test]\nfn t() {}");
        assert!(scan_rust_test_runs(&src, "benches/bench.rs").is_empty());
    }

    #[test]
    fn detects_file_module_and_func() {
        let src = lines(
            "pub fn foo() {}\n\
             \n\
             #[cfg(test)]\n\
             mod tests {\n\
             \x20   #[test]\n\
             \x20   fn it_works() {}\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/ai_caller.rs");

        // 1 行目のファイルボタンはファイル内のすべてを実行する。
        let file = &runs[&1];
        assert_eq!(file.kind, TestRunKind::File);
        assert_eq!(file.label, "ai_caller.rs");
        assert_eq!(file.command, "cargo test 'ai_caller::'");

        // mod tests の行 (4 行目) のモジュールボタン。
        let module = &runs[&4];
        assert_eq!(module.kind, TestRunKind::Module);
        assert_eq!(module.label, "tests");
        assert_eq!(module.command, "cargo test 'ai_caller::tests::'");

        // fn it_works の行 (6 行目) の関数ボタン。
        let func = &runs[&6];
        assert_eq!(func.kind, TestRunKind::Func);
        assert_eq!(func.label, "it_works");
        assert_eq!(
            func.command,
            "cargo test 'ai_caller::tests::it_works' -- --exact"
        );
    }

    #[test]
    fn tokio_test_async_fn_is_detected() {
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   #[tokio::test]\n\
             \x20   async fn talks() {}\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/net.rs");
        let func = &runs[&4];
        assert_eq!(func.kind, TestRunKind::Func);
        assert_eq!(func.command, "cargo test 'net::tests::talks' -- --exact");
    }

    #[test]
    fn nested_modules_build_full_paths() {
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   mod command {\n\
             \x20       #[test]\n\
             \x20       fn echoes() {}\n\
             \x20   }\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/ai_caller.rs");

        // 内側のモジュールボタン (3 行目) は入れ子のモジュールに絞られる。
        assert_eq!(
            runs[&3].command,
            "cargo test 'ai_caller::tests::command::'"
        );
        // 外側のモジュールボタン (2 行目)。
        assert_eq!(runs[&2].command, "cargo test 'ai_caller::tests::'");
        // 関数は入れ子を含む完全なパスを持つ (5 行目)。
        assert_eq!(
            runs[&5].command,
            "cargo test 'ai_caller::tests::command::echoes' -- --exact"
        );
    }

    #[test]
    fn cfg_test_module_without_tests_yields_nothing() {
        // ヘルパーしか無い #[cfg(test)] モジュールはテストのスコープではない。
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   fn helper() {}\n\
             }\n",
        );
        assert!(scan_rust_test_runs(&src, "src/foo.rs").is_empty());
    }

    #[test]
    fn mod_rs_maps_to_directory_module() {
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/app/mod.rs");
        // app/mod.rs はモジュール app。トップレベルの #[test] はその直下に置かれる。
        assert_eq!(runs[&2].command, "cargo test 'app::t' -- --exact");
        assert_eq!(runs[&1].command, "cargo test 'app::'");
    }

    #[test]
    fn crate_root_runs_all_for_file_button() {
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/main.rs");
        // クレートルートではモジュール接頭辞が無いので、ファイルボタンは全部を実行する。
        assert_eq!(runs[&1].command, "cargo test");
        assert_eq!(runs[&2].command, "cargo test 't' -- --exact");
    }

    #[test]
    fn integration_file_uses_test_flag() {
        let src = lines("#[test]\nfn smoke() {}\n");
        let runs = scan_rust_test_runs(&src, "tests/e2e.rs");
        assert_eq!(runs[&1].command, "cargo test --test 'e2e'");
        assert_eq!(
            runs[&2].command,
            "cargo test --test 'e2e' 'smoke' -- --exact"
        );
    }

    #[test]
    fn hostile_file_name_is_shell_quoted() {
        // シングルクォートを含むパス (信用できないリポジトリならあり得る) は、
        // モジュール接頭辞の '\'' エスケープで無力化されなければならない。
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/o'clock.rs");
        assert_eq!(runs[&1].command, "cargo test 'o'\\''clock::'");
    }
}
