//! conductor mcp-serve — レビューデータベースを stdio 上の MCP サーバとして提供する。
//!
//! ヘッドレスなウォークスルーセッション（および TUI 内で動く Claude Code
//! セッション）は、別に配布された Node パッケージ経由ではなく、このサブコマンド
//! を通してレビューデータベースにアクセスする。ツールをバイナリの中に同梱する
//! ことで、両者がスキーマについて食い違うことがそもそも起こり得なくなる —
//! インストールすべき成果物が1つしかないため。
//!
//! stdout は JSON-RPC が占有する。ここから stdout に書き込んではならない —
//! ログは stderr に出す（env_logger のデフォルト）。main は端末セットアップの
//! いずれよりも先に [run] に到達しなければならない。

mod args;
mod reply;
mod resolve;
mod tools;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::stdio;

use crate::review_store::ReviewStore;

/// クライアントが切断するまで、レビューデータベースを stdin/stdout 上で提供する。
///
/// stdin が EOF に達したら返る。親セッションはこうやって自分をシャットダウン
/// する — このプロセスは自分を起動した claude より長生きしてはならない。
/// そうしないとデータベースへの接続を握ったまま居座ることになる。
pub fn run() -> Result<()> {
    let db_arg = resolve::parse_db_arg(std::env::args());
    let db_path = resolve::resolve_db_path(db_arg)?;
    let store = ReviewStore::open(&db_path)
        .with_context(|| format!("failed to open review database at {}", db_path.display()))?;

    log::info!("mcp-serve: using database {}", db_path.display());

    // 1本のパイプに1クライアントだけなので、current-thread ランタイムで足りる。
    // これによりバイナリがマルチスレッドスケジューラを引き込まずに済む。
    // タイマーは省略できない — rmcp のサービスループはリクエストのタイムアウト
    // のためにタイマーをセットするので、無いランタイムでは panic する。
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .context("failed to start the async runtime for mcp-serve")?;

    runtime.block_on(async move {
        let service = tools::McpServer::new(store, db_path)
            .serve(stdio())
            .await
            .context("MCP handshake over stdio failed")?;
        service
            .waiting()
            .await
            .context("MCP session ended abnormally")?;
        Ok(())
    })
}
