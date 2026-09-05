//! conductor の実行ファイル。画面もコマンドも conductor-tui にある。

fn main() -> anyhow::Result<()> {
    conductor_tui::entry::run(env!("CARGO_PKG_VERSION"))
}
