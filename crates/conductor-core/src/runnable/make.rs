//! 開いている Makefile の中から、`.PHONY` に宣言されたターゲットを検出する。
//!
//! ファイルの内容を 1 行ずつ走査し、1 始まりの行番号から、そのターゲットを実行する
//! make コマンドを表す [Runnable] へのマップを作る。
//!
//! ボタンを出すのは `.PHONY` に挙がっている名前だけ。ファイルを生成する本物の
//! ターゲットはクリック 1 回で走らせるには副作用が重い。
//!
//! コマンドは Shell の PTY の作業ディレクトリが worktree ルートであることを前提に、
//! 入れ子の Makefile には `make -C <dir>` を付ける。

use std::collections::HashMap;

use super::{Runnable, RunnableKind, shell_single_quote};

/// 開いているファイルの内容から実行可能な make のターゲットを走査する。
///
/// relative_path が Makefile でないか、`.PHONY` の宣言が無い場合は空のマップを返す。
pub fn scan_make_targets(file_content: &[String], relative_path: &str) -> HashMap<usize, Runnable> {
    let mut runs = HashMap::new();
    if !is_makefile(relative_path) {
        return runs;
    }
    let phony = phony_names(file_content);
    if phony.is_empty() {
        return runs;
    }
    let dir = relative_path.rsplit_once('/').map(|(dir, _)| dir);

    for (i, line) in file_content.iter().enumerate() {
        let targets: Vec<&str> = rule_targets(line)
            .into_iter()
            .filter(|t| phony.contains(&t.to_string()))
            .collect();
        if targets.is_empty() {
            continue;
        }
        runs.insert(
            i + 1,
            Runnable {
                kind: RunnableKind::Target,
                label: targets.join(" "),
                command: make_cmd(dir, &targets),
            },
        );
    }
    runs
}

fn is_makefile(relative_path: &str) -> bool {
    let name = relative_path
        .rsplit_once('/')
        .map_or(relative_path, |(_, f)| f);
    name == "Makefile" || name == "makefile"
}

/// `.PHONY:` に挙げられた名前。行末の `\` で続く行も拾う。
fn phony_names(file_content: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    let mut continuing = false;
    for line in file_content {
        let rest = if continuing {
            line.as_str()
        } else {
            match line.strip_prefix(".PHONY") {
                Some(rest) => match rest.trim_start().strip_prefix(':') {
                    Some(rest) => rest,
                    None => continue,
                },
                None => continue,
            }
        };
        let (rest, more) = match rest.trim_end().strip_suffix('\\') {
            Some(head) => (head, true),
            None => (rest, false),
        };
        names.extend(rest.split_whitespace().map(str::to_string));
        continuing = more;
    }
    names
}

/// ルール行ならコロンの左に並ぶターゲット名。レシピ行・変数代入・コメントは空。
fn rule_targets(line: &str) -> Vec<&str> {
    if line.starts_with([' ', '\t', '#']) {
        return Vec::new();
    }
    let Some(colon) = line.find(':') else {
        return Vec::new();
    };
    // `FOO := x` や `FOO ::= x` は代入であってルールではない。
    let after = line[colon + 1..].trim_start_matches(':');
    if after.starts_with('=') {
        return Vec::new();
    }
    line[..colon].split_whitespace().collect()
}

fn make_cmd(dir: Option<&str>, targets: &[&str]) -> String {
    let mut cmd = String::from("make");
    if let Some(dir) = dir {
        cmd.push_str(&format!(" -C {}", shell_single_quote(dir)));
    }
    for target in targets {
        cmd.push(' ');
        cmd.push_str(&shell_single_quote(target));
    }
    cmd
}
