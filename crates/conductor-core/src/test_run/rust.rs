//! 開いている `*.rs` ファイルの中から、実行可能な Rust のテストを検出する。
//!
//! [super::go] の Rust 版。Go は平坦な規約 (`func TestXxx`) を持つのに対し、
//! Rust は `#[test]` / `#[tokio::test]` の関数を `#[cfg(test)] mod` ブロックの中に
//! 入れ子にし、テストハーネスは各テストを完全なモジュールパス
//! (`<ファイルのモジュール>::<内側のモジュール群>::<関数>`、例:
//! `ai_caller::tests::command::echoes_prompt_via_stdin`) で名指しする。正確に
//! 絞り込むにはファイルのモジュールパス (ソースツリー上の位置から決まる) と、
//! ファイル内の mod の入れ子の両方が必要になるため、行単位の正規表現ではなく
//! tree-sitter-rust でファイルをパースする。
//!
//! 1 始まりの行番号から、そのスコープを実行する cargo test コマンドを表す
//! [TestRun] へのマップを作る。出すボタンは 3 種類:
//!
//! - File: 1 行目。ファイル内の全テスト (`cargo test '<mod>::'`)。
//! - Func: テストの fn の各行。そのテスト 1 つだけ
//!   (`cargo test '<完全なパス>' -- --exact`)。
//! - Module: (間接的にでも) テストを含む mod の各行。その配下の全テスト
//!   (`cargo test '<mod のパス>::'`)。
//!
//! コマンドは Shell の PTY の作業ディレクトリがクレートルート (worktree ルート)
//! であることを前提にする。go スキャナが go test について置いている前提と同じ。

use std::collections::HashMap;

use super::{TestRun, TestRunKind, shell_single_quote};

/// ファイルが cargo test のターゲットとモジュールパスの接頭辞にどう対応するか。
enum FileTarget {
    /// 既定の (bin / lib) ターゲットにコンパイルされる単体テスト。prefix は
    /// ファイルのモジュールパス (クレートルート、つまり main.rs / lib.rs では空)。
    Unit { prefix: Vec<String> },
    /// トップレベルの `tests/<name>.rs` の結合テストバイナリ。`--test <name>` で
    /// 選択する。その中のテストパスにはファイルのモジュール接頭辞が付かない。
    Integration { name: String },
}

/// 開いているファイルの内容から実行可能な Rust のテストを走査する。
///
/// relative_path が対応対象の .rs ファイルでない (src/ とトップレベルの
/// tests/ の外にある) か、テストを含まない場合は空のマップを返す。
pub fn scan_rust_test_runs(
    file_content: &[String],
    relative_path: &str,
) -> HashMap<usize, TestRun> {
    let mut runs = HashMap::new();

    let Some(target) = file_target(relative_path) else {
        return runs;
    };

    // パーサ向けにソーステキストを組み直す。file_content はファイルの論理行
    // (タブは既に空白へ展開済み) なので、'\n' で連結すれば同等の行と桁の構造が
    // 再現される。tree-sitter の行は file_content の添字と 1 対 1 に対応する。
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
    /// 結合テストバイナリ向けの `--test <name>` セレクタ。既定のターゲットなら空。
    /// `cargo test{...}` にそのまま差し込めるよう先頭に空白を含む。
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
    /// `<ファイルの接頭辞>::<内側のモジュール群>::<関数>`。
    fn full_path(&self, mod_stack: &[String], name: &str) -> String {
        let mut segs: Vec<&str> = self.file_prefix().iter().map(String::as_str).collect();
        segs.extend(mod_stack.iter().map(String::as_str));
        segs.push(name);
        segs.join("::")
    }

    /// 連結したパスの末尾に `::` を付ける。部分一致がそのモジュールの子孫に限られる。
    fn module_prefix(&self, mod_stack: &[String]) -> String {
        let mut segs: Vec<&str> = self.file_prefix().iter().map(String::as_str).collect();
        segs.extend(mod_stack.iter().map(String::as_str));
        if segs.is_empty() {
            String::new()
        } else {
            format!("{}::", segs.join("::"))
        }
    }

    /// `cargo test '<完全なパス>' -- --exact`。このテスト 1 つだけを実行する。
    fn func_command(&self, full_path: &str) -> String {
        format!(
            "cargo test{} {} -- --exact",
            self.test_flag(),
            shell_single_quote(full_path)
        )
    }

    /// `cargo test '<接頭辞>::'`。このモジュール配下の全テストを実行する。
    fn module_command(&self, module_prefix: &str) -> String {
        format!(
            "cargo test{} {}",
            self.test_flag(),
            shell_single_quote(module_prefix)
        )
    }

    /// モジュール接頭辞で絞るか、結合テストバイナリなら `--test <name>`、
    /// クレートルート (main.rs / lib.rs) なら絞り込み無し。
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

/// この部分木でテストが見つかったかを返す。mod は自分の Module ボタンを、
/// 呼び出し側は File ボタンを描くかどうかをこれで決める。
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

/// tree-sitter-rust では属性は前方の兄弟だが、文法バージョンによっては子として
/// 入れ子にもなるので両方を調べる。
fn is_test_fn(fn_node: tree_sitter::Node, source: &str) -> bool {
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

    let mut cursor = fn_node.walk();
    for child in fn_node.children(&mut cursor) {
        if child.kind() == "attribute_item" && attr_is_test(child, source) {
            return true;
        }
    }
    false
}

/// `::` で区切った最後のセグメントが test か、test で終わらない有名なテストマクロの
/// 許可リストに載っているか。`#[cfg(test)]` はセグメントが cfg なので外れる。
fn attr_is_test(attr_item: tree_sitter::Node, source: &str) -> bool {
    let text = node_text(attr_item, source).trim();
    let inner = match text.strip_prefix("#[").and_then(|s| s.strip_suffix(']')) {
        Some(inner) => inner.trim(),
        None => return false,
    };
    let path = inner.split('(').next().unwrap_or("").trim();
    let last = path.rsplit("::").next().unwrap_or("").trim();
    last == "test" || matches!(path, "rstest")
}

/// ファイルのリポジトリ相対パスから cargo test のターゲットとモジュール接頭辞を
/// 導く。対応していない位置なら None (実行ボタンを出さない)。
fn file_target(relative_path: &str) -> Option<FileTarget> {
    let stem = relative_path.strip_suffix(".rs")?;

    if let Some(rest) = stem.strip_prefix("src/") {
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
