//! 開いている `*_test.go` ファイルの中から、実行可能な Go のテストを検出する。
//!
//! ファイルの内容を 1 行ずつ走査し (正規表現。Go のパーサは持たない)、1 始まりの
//! 行番号から、そのスコープを実行する go test コマンドを表す [TestRun] への
//! マップを作る。作るボタンは 3 種類:
//!
//! - File: 1 行目。ファイル内のトップレベルの `Test*` 関数をすべて実行する。
//! - Func: `func Test*(...)` の各行。そのテスト 1 つを実行する。
//! - Subtest: テスト関数内の `x.Run("name", ...)` の各行。外側のテストの、
//!   そのサブテストを実行する。
//!
//! コマンドはファイルのパッケージディレクトリ (`./dir`、リポジトリルートなら `.`)
//! を対象にし、Shell の PTY の作業ディレクトリが worktree ルートであることを前提にする。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::{TestRun, TestRunKind, shell_single_quote};

/// トップレベルのテスト関数: 0 桁目から始まる `func TestXxx(`。レシーバ付きの
/// メソッド (`func (s *Suite) TestX()`) はあえて対象外にしている。go test -run
/// では直接指定できないため。
static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^func\s+(Test\w*)\s*\(").unwrap());

/// 名前が文字列リテラルのサブテスト呼び出し: `x.Run("name"`。テーブル駆動の
/// `t.Run(tt.name, …)` のようなリテラルでないものは飛ばす。関数単位のボタンが
/// それをカバーする。
static SUBTEST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.Run\(\s*"([^"]*)""#).unwrap());

/// 開いているファイルの内容から実行可能な Go のテストを走査する。
///
/// relative_path が `*_test.go` でないか、ファイルにトップレベルの `Test*`
/// 関数が無い場合は空のマップを返す。
pub fn scan_go_test_runs(file_content: &[String], relative_path: &str) -> HashMap<usize, TestRun> {
    let mut runs = HashMap::new();
    if !relative_path.ends_with("_test.go") {
        return runs;
    }
    let target = package_target(relative_path);

    // トップレベルのテスト関数 (行と名前) を見つける。TestMain は特別な入口で
    // -run の対象にならないため飛ばす。
    let funcs: Vec<(usize, String)> = file_content
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            let name = FUNC_RE.captures(line)?.get(1)?.as_str();
            (name != "TestMain").then(|| (i + 1, name.to_string()))
        })
        .collect();

    if funcs.is_empty() {
        return runs;
    }

    let all = funcs
        .iter()
        .map(|(_, n)| n.as_str())
        .collect::<Vec<_>>()
        .join("|");
    runs.insert(
        1,
        TestRun {
            kind: TestRunKind::File,
            label: file_label(relative_path),
            command: go_test_cmd(&format!("^({all})$"), &target),
        },
    );

    for (line, name) in &funcs {
        runs.insert(
            *line,
            TestRun {
                kind: TestRunKind::Func,
                label: name.clone(),
                command: go_test_cmd(&format!("^{name}$"), &target),
            },
        );
    }

    // サブテストのボタン: 各 Run("name") を、それを囲む一番近いトップレベルの
    // テストに結びつける。0 桁目から始まる func … の行がテスト関数でなければ、
    // そこで現在のテストのスコープが終わる。
    let mut current: Option<&str> = None;
    for (i, line) in file_content.iter().enumerate() {
        if line.starts_with("func ") {
            current = FUNC_RE
                .captures(line)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .filter(|n| *n != "TestMain");
        }
        let Some(func) = current else { continue };
        let line_1 = i + 1;
        if runs.contains_key(&line_1) {
            continue; // 同じ行の関数ボタンを上書きしない。
        }
        let Some(sub) = SUBTEST_RE.captures(line).and_then(|c| c.get(1)) else {
            continue;
        };
        let sub = sub.as_str();
        if sub.contains('\'') {
            // シングルクォート入りの名前は -run 引数のクォートを壊す。
            // 壊れたコマンドを出すくらいなら飛ばす。
            continue;
        }
        // Go は -run の照合にあたり、サブテスト名の空白をアンダースコアに対応づける。
        let sub_pattern = sub.replace(' ', "_");
        runs.insert(
            line_1,
            TestRun {
                kind: TestRunKind::Subtest,
                label: format!("{func}/{sub}"),
                command: go_test_cmd(&format!("^{func}$/^{sub_pattern}$"), &target),
            },
        );
    }

    runs
}

fn go_test_cmd(run_pattern: &str, target: &str) -> String {
    // target はレビュー中の PR など信用できないリポジトリ由来のファイルパスから
    // 作られるので、シェルのメタ文字や空白を含み得る。run_pattern はここへ来る前に
    // シングルクォート入りのサブテスト名を弾いているので、そのままクォートできる。
    format!(
        "go test -run '{run_pattern}' {}",
        shell_single_quote(target)
    )
}

/// ファイルに対応する go test のパッケージ引数。入れ子のファイルなら `./dir`、
/// リポジトリルートなら `.`。
fn package_target(relative_path: &str) -> String {
    match relative_path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => format!("./{dir}"),
        _ => ".".to_string(),
    }
}

fn file_label(relative_path: &str) -> String {
    relative_path
        .rsplit_once('/')
        .map(|(_, f)| f)
        .unwrap_or(relative_path)
        .to_string()
}
