//! 開いている *_test.go ファイルの中から、実行可能な Go のテストを検出する。
//!
//! ファイルの内容を 1 行ずつ走査し (正規表現。Go のパーサは持たない)、1 始まりの
//! 行番号から、そのスコープを実行する go test コマンドを表す [TestRun] への
//! マップを作る。作るボタンは 3 種類:
//!
//! - File: 1 行目。ファイル内のトップレベルの Test* 関数をすべて実行する。
//! - Func: func Test*(...) の各行。そのテスト 1 つを実行する。
//! - Subtest: テスト関数内の x.Run("name", ...) の各行。外側のテストの、
//!   そのサブテストを実行する。
//!
//! コマンドはファイルのパッケージディレクトリ (./dir、リポジトリルートなら .)
//! を対象にし、Shell の PTY の作業ディレクトリが worktree ルートであることを前提にする。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::test_run::{TestRun, TestRunKind, shell_single_quote};

/// トップレベルのテスト関数: 0 桁目から始まる func TestXxx(。レシーバ付きの
/// メソッド (func (s *Suite) TestX() はあえて対象外にしている。go test -run
/// では直接指定できないため。
static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^func\s+(Test\w*)\s*\(").unwrap());

/// 名前が文字列リテラルのサブテスト呼び出し: x.Run("name"。テーブル駆動の
/// t.Run(tt.name, …) のようなリテラルでないものは飛ばす。関数単位のボタンが
/// それをカバーする。
static SUBTEST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.Run\(\s*"([^"]*)""#).unwrap());

/// 開いているファイルの内容から実行可能な Go のテストを走査する。
///
/// relative_path が *_test.go でないか、ファイルにトップレベルの Test*
/// 関数が無い場合は空のマップを返す。
pub fn scan_go_test_runs(file_content: &[String], relative_path: &str) -> HashMap<usize, TestRun> {
    let mut runs = HashMap::new();
    if !relative_path.ends_with("_test.go") {
        return runs;
    }
    let target = package_target(relative_path);

    // 第 1 走査: トップレベルのテスト関数 (行と名前) を見つける。特別な入口である
    // TestMain は飛ばす (-run の対象にならない)。
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

    // 1 行目のファイル単位のボタン: ファイル内のトップレベルのテストをすべて実行する。
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

    // 関数単位のボタン。
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
        // 同じ行にある関数ボタンを上書きしない。
        if runs.contains_key(&line_1) {
            continue;
        }
        let Some(sub) = SUBTEST_RE.captures(line).and_then(|c| c.get(1)) else {
            continue;
        };
        let sub = sub.as_str();
        // 名前にシングルクォートが入っていると、シングルクォートで囲んだ -run
        // 引数が壊れる。壊れたコマンドを出すくらいなら飛ばす。
        if sub.contains('\'') {
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
    // target は信用できないリポジトリ (レビュー中の PR など) 由来のファイルパスから
    // 作られるので、シェルのメタ文字や空白を含み得る。シングルクォートで囲む。
    // run_pattern はリテラルのシングルクォートで囲んで安全: 関数名は \w のみで、
    // ' を含むサブテスト名はここへ来る前に弾かれているため、埋め込みの
    // シングルクォートは現れない。
    format!(
        "go test -run '{run_pattern}' {}",
        shell_single_quote(target)
    )
}

/// ファイルに対応する go test のパッケージ引数。入れ子のファイルなら ./dir、
/// リポジトリルートなら .。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<String> {
        src.lines().map(str::to_string).collect()
    }

    #[test]
    fn non_test_file_yields_nothing() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        assert!(scan_go_test_runs(&src, "foo/foo.go").is_empty());
    }

    #[test]
    fn detects_file_func_and_subtest() {
        let src = lines(
            "package foo\n\
             \n\
             func TestAlpha(t *testing.T) {\n\
             \tt.Run(\"case one\", func(t *testing.T) {})\n\
             }\n\
             \n\
             func TestBeta(t *testing.T) {}\n",
        );
        let runs = scan_go_test_runs(&src, "pkg/foo/foo_test.go");

        // 1 行目のファイルボタンは両方のテストを実行する。
        let file = &runs[&1];
        assert_eq!(file.kind, TestRunKind::File);
        assert_eq!(
            file.command,
            "go test -run '^(TestAlpha|TestBeta)$' './pkg/foo'"
        );

        // TestAlpha の行の関数ボタン。
        let alpha = &runs[&3];
        assert_eq!(alpha.kind, TestRunKind::Func);
        assert_eq!(alpha.command, "go test -run '^TestAlpha$' './pkg/foo'");

        // t.Run の行のサブテストボタン (空白はアンダースコアへ)。
        let sub = &runs[&4];
        assert_eq!(sub.kind, TestRunKind::Subtest);
        assert_eq!(sub.label, "TestAlpha/case one");
        assert_eq!(
            sub.command,
            "go test -run '^TestAlpha$/^case_one$' './pkg/foo'"
        );

        // TestBeta の行の関数ボタン。
        assert_eq!(runs[&7].command, "go test -run '^TestBeta$' './pkg/foo'");
    }

    #[test]
    fn test_main_is_excluded() {
        let src = lines(
            "package foo\n\
             func TestMain(m *testing.M) {}\n\
             func TestReal(t *testing.T) {}\n",
        );
        let runs = scan_go_test_runs(&src, "a_test.go");
        // 1 行目のファイルボタンは TestReal だけを並べる。TestMain にはボタンが無い。
        assert_eq!(runs[&1].command, "go test -run '^(TestReal)$' '.'");
        assert!(!runs.contains_key(&2));
        assert_eq!(runs[&3].kind, TestRunKind::Func);
    }

    #[test]
    fn root_file_targets_current_dir() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "x_test.go");
        assert_eq!(runs[&2].command, "go test -run '^TestX$' '.'");
    }

    #[test]
    fn hostile_directory_name_is_shell_quoted() {
        // シェルのメタ文字を含むディレクトリ名 (信用できないリポジトリならあり得る)
        // はクォートで無力化されなければならない。; はクォートの内側に留まる。
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "a; rm -rf x/x_test.go");
        assert_eq!(runs[&2].command, "go test -run '^TestX$' './a; rm -rf x'");
    }

    #[test]
    fn single_quote_in_directory_is_escaped() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "o'clock/x_test.go");
        // '\'' はクォートを閉じ、エスケープしたクォートを足し、また開く。
        assert_eq!(runs[&2].command, "go test -run '^TestX$' './o'\\''clock'");
    }

    #[test]
    fn no_tests_means_no_buttons() {
        let src = lines("package foo\nfunc helper() {}\n");
        assert!(scan_go_test_runs(&src, "foo_test.go").is_empty());
    }

    #[test]
    fn subtest_outside_test_func_is_ignored() {
        let src = lines(
            "package foo\n\
             func helper() {\n\
             \tx.Run(\"nope\", nil)\n\
             }\n\
             func TestX(t *testing.T) {}\n",
        );
        let runs = scan_go_test_runs(&src, "foo_test.go");
        assert!(!runs.contains_key(&3));
    }
}
