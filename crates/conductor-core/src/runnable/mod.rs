//! ファイルの行に紐づいた実行可能なもの。Viewer のクリック可能な ▶ 実行ボタンの
//! 背後にある共有モデル。
//!
//! 種類ごとのスキャナ ([go::scan_go_tests] が `*_test.go`、[rust::scan_rust_tests]
//! が `*.rs`、[make::scan_make_targets] が Makefile を担当) が、1 始まりの行番号から
//! [Runnable] へのマップを作る。Viewer はキーになっている各行に ▶ を描き、
//! クリックされたら [Runnable::command] を Shell の PTY へ送る。
//! スキャナから先は言語非依存で、利用側は command と label しか読まない。

mod go;
mod make;
mod rust;

#[cfg(test)]
mod tests;

pub use go::scan_go_tests;
pub use make::scan_make_targets;
pub use rust::scan_rust_tests;

/// 実行ボタンがカバーするスコープ (ステータスバーの文言に使う)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableKind {
    /// ファイル内の全テスト。
    File,
    /// テスト関数 1 つ。
    Func,
    /// モジュールとその配下にネストした全テスト (Rust の `#[cfg(test)] mod …`)。
    Module,
    /// 外側のテスト関数に属する Go の `Run("…")` サブテスト。
    Subtest,
    /// Makefile の `.PHONY` ターゲット 1 つ。
    Target,
}

/// ファイルの行に紐づいた、実行可能なもの 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runnable {
    pub kind: RunnableKind,
    /// ステータスバー用の人間可読なラベル (例: "TestFoo/case",
    /// "build_caller_rejects_empty_command")。
    pub label: String,
    /// 実行するシェルコマンド全体。例: `go test -run '^TestFoo$' ./pkg/foo`,
    /// `cargo test 'ai_caller::tests::foo' -- --exact`。
    pub command: String,
}

/// スキャナが組み立てるコマンドのフィルタには、レビュー対象の信用できない
/// リポジトリ由来の (敵対的かもしれない) パスが埋め込まれ得るため、
/// `'\''` イディオムでのエスケープが必須。
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
