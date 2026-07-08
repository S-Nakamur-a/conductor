//! Detection of runnable Go tests within an open `*_test.go` file.
//!
//! Scans file content line-by-line (regex, no full Go parser) and produces a
//! map from 1-indexed line number to a [`TestRun`] describing the `go test`
//! command that runs that scope. Three button kinds are produced:
//!
//! - **File**: on line 1 — runs every top-level `Test*` function in the file.
//! - **Func**: on each `func Test*(...)` line — runs that one test.
//! - **Subtest**: on each `x.Run("name", ...)` line inside a test function —
//!   runs that subtest of its enclosing test.
//!
//! Commands target the file's package directory (`./dir`, or `.` at the repo
//! root) and rely on the Shell PTY's working directory being the worktree root.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

/// Top-level test function: `func TestXxx(` at column 0. Receiver methods
/// (`func (s *Suite) TestX(`) are intentionally not matched — `go test -run`
/// can't target them directly.
static FUNC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^func\s+(Test\w*)\s*\(").unwrap());

/// A subtest call with a string-literal name: `x.Run("name"`. Table-driven
/// `t.Run(tt.name, …)` (non-literal) is skipped — the func-level button covers it.
static SUBTEST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.Run\(\s*"([^"]*)""#).unwrap());

/// What scope a run button covers (used for the status-bar label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunKind {
    /// Every top-level test in the file.
    File,
    /// A single top-level test function.
    Func,
    /// A `Run("…")` subtest of an enclosing test function.
    Subtest,
}

/// A single runnable test scope anchored to a file line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub kind: TestRunKind,
    /// Human-readable label for the status bar (e.g. `"TestFoo/case"`).
    pub label: String,
    /// Full shell command, e.g. `go test -run '^TestFoo$' ./pkg/foo`.
    pub command: String,
}

/// Scan an open file's content for runnable Go tests.
///
/// Returns an empty map when `relative_path` is not a `*_test.go` file or the
/// file contains no top-level `Test*` functions.
pub fn scan_go_test_runs(file_content: &[String], relative_path: &str) -> HashMap<usize, TestRun> {
    let mut runs = HashMap::new();
    if !relative_path.ends_with("_test.go") {
        return runs;
    }
    let target = package_target(relative_path);

    // First pass: locate top-level test functions (line + name), skipping the
    // special `TestMain` entry point (not matched by `-run`).
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

    // File-level button on line 1: run every top-level test in the file.
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

    // Func-level buttons.
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

    // Subtest buttons: associate each `Run("name")` with the nearest enclosing
    // top-level test. A `func …` line at column 0 that is not a test function
    // ends the current test's scope.
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
        // Don't shadow a func button that sits on the same line.
        if runs.contains_key(&line_1) {
            continue;
        }
        let Some(sub) = SUBTEST_RE.captures(line).and_then(|c| c.get(1)) else {
            continue;
        };
        let sub = sub.as_str();
        // A single quote in the name would break the single-quoted `-run`
        // argument; skip rather than emit a malformed command.
        if sub.contains('\'') {
            continue;
        }
        // Go maps spaces in subtest names to underscores for `-run` matching.
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
    // `target` is derived from a filesystem path that can come from an untrusted
    // repo (e.g. a PR under review), so it may contain shell metacharacters or
    // spaces — single-quote it. `run_pattern` is safe to wrap in literal single
    // quotes: func names are `\w`-only and subtest names containing a `'` are
    // rejected before we get here, so no embedded single quote can appear.
    format!("go test -run '{run_pattern}' {}", shell_single_quote(target))
}

/// Wrap `s` in single quotes for safe use as one shell word, escaping any
/// embedded single quotes via the `'\''` idiom.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The `go test` package argument for a file: `./dir` for a nested file, `.`
/// at the repo root.
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

        // File button on line 1 runs both tests.
        let file = &runs[&1];
        assert_eq!(file.kind, TestRunKind::File);
        assert_eq!(
            file.command,
            "go test -run '^(TestAlpha|TestBeta)$' './pkg/foo'"
        );

        // Func button on the TestAlpha line.
        let alpha = &runs[&3];
        assert_eq!(alpha.kind, TestRunKind::Func);
        assert_eq!(alpha.command, "go test -run '^TestAlpha$' './pkg/foo'");

        // Subtest button on the t.Run line (spaces → underscores).
        let sub = &runs[&4];
        assert_eq!(sub.kind, TestRunKind::Subtest);
        assert_eq!(sub.label, "TestAlpha/case one");
        assert_eq!(
            sub.command,
            "go test -run '^TestAlpha$/^case_one$' './pkg/foo'"
        );

        // Func button on the TestBeta line.
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
        // Line 1 file button only lists TestReal; TestMain has no button.
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
        // A dir name with shell metacharacters (as an untrusted repo could
        // contain) must be neutralized by quoting — the `;` stays inside quotes.
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "a; rm -rf x/x_test.go");
        assert_eq!(runs[&2].command, "go test -run '^TestX$' './a; rm -rf x'");
    }

    #[test]
    fn single_quote_in_directory_is_escaped() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "o'clock/x_test.go");
        // `'\''` closes the quote, adds an escaped quote, and reopens it.
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
