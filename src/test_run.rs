//! A runnable test scope anchored to a file line — the shared model behind the
//! Viewer's clickable ▶ run buttons.
//!
//! Language-specific scanners ([`crate::go_test`] for `*_test.go`,
//! [`crate::rust_test`] for `*.rs`) produce a map from 1-indexed line number to
//! a [`TestRun`]. The Viewer draws a ▶ on each keyed line and, on click, sends
//! [`TestRun::command`] to the Shell PTY (see `event/mouse.rs`). Everything past
//! the scanner is language-agnostic: consumers only read `command` and `label`.

/// What scope a run button covers (used for the status-bar label wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunKind {
    /// Every test in the file.
    File,
    /// A single test function.
    Func,
    /// A module and every test nested under it (Rust `#[cfg(test)] mod …`).
    Module,
    /// A Go `Run("…")` subtest of an enclosing test function.
    Subtest,
}

/// A single runnable test scope anchored to a file line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub kind: TestRunKind,
    /// Human-readable label for the status bar (e.g. `"TestFoo/case"` or
    /// `"build_caller_rejects_empty_command"`).
    pub label: String,
    /// Full shell command, e.g. `go test -run '^TestFoo$' ./pkg/foo` or
    /// `cargo test 'ai_caller::tests::foo' -- --exact`.
    pub command: String,
}

/// Wrap `s` in single quotes for safe use as one shell word, escaping any
/// embedded single quotes via the `'\''` idiom. Shared by the language scanners,
/// whose command filters can embed a filesystem-derived (possibly hostile) path
/// from an untrusted repo under review.
pub fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
