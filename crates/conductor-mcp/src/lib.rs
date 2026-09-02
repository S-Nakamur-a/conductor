//! conductor mcp-serve — レビュー DB を stdio 上の MCP サーバとして公開する。
//!
//! ヘッドレスのウォークスルーセッションも TUI 内の Claude Code セッションも、
//! 別配布の Node パッケージではなくここを通して DB に触る。ツールをバイナリに
//! 同梱すると、インストールすべき成果物が 1 つになりスキーマの食い違いが
//! 起こり得なくなる。
//!
//! stdout は JSON-RPC が占有する。このクレートのどこからも stdout に印字しては
//! ならない (ログは stderr)。呼び出し側は端末に触る前に [run] へ到達すること。

mod args;
mod refresh_signal;
mod reply;
mod resolve;
mod tools;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::stdio;

use conductor_core::review_store::ReviewStore;

/// クライアントが切断するまで、レビュー DB を stdin/stdout 上で提供する。
///
/// `args` はプロセスの argv (`--db <path>` を読む)、`version` は MCP の
/// initialize でサーバ実装として名乗る版で、呼び出し元のバイナリのものを渡す。
///
/// stdin が EOF に達したら返る。親セッションはこうやってシャットダウンする —
/// 自分を起動した claude より長生きすると、DB 接続を握ったまま居座ることになる。
pub fn run(args: impl IntoIterator<Item = String>, version: &str) -> Result<()> {
    let db_path = resolve::resolve_db_path(resolve::parse_db_arg(args))?;
    let store = ReviewStore::open(&db_path)
        .with_context(|| format!("failed to open review database at {}", db_path.display()))?;
    let cwd = std::env::current_dir().context("failed to read the current directory")?;

    log::info!("mcp-serve: using database {}", db_path.display());

    // 1 本のパイプに 1 クライアントなので current-thread で足りる。タイマーは
    // 省略できない — rmcp のサービスループがリクエストのタイムアウトに使うので、
    // 無いランタイムでは panic する。
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .context("failed to start the async runtime for mcp-serve")?;

    runtime.block_on(async move {
        let service = tools::McpServer::new(store, db_path, cwd, version)
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
