//! `conductor mcp-serve` — the review database as a stdio MCP server.
//!
//! The headless walkthrough session (and any Claude Code session running inside
//! the TUI) reaches the review database through this subcommand rather than
//! through a separately-published Node package. Shipping the tools inside the
//! binary is what makes it impossible for the two to disagree about the schema:
//! there is only one artifact to install.
//!
//! stdout belongs to JSON-RPC. Nothing in here may print to it — logging goes to
//! stderr (env_logger's default), and `main` must reach [`run`] before any of
//! the terminal setup runs.

mod args;
mod reply;
mod resolve;
mod tools;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::stdio;

use crate::review_store::ReviewStore;

/// Serve the review database on stdin/stdout until the client disconnects.
///
/// Returns once stdin reaches EOF, which is how the parent session shuts us
/// down: the process must not outlive the `claude` that spawned it, or it would
/// sit holding a connection to the database.
pub fn run() -> Result<()> {
    let db_arg = resolve::parse_db_arg(std::env::args());
    let db_path = resolve::resolve_db_path(db_arg)?;
    let store = ReviewStore::open(&db_path)
        .with_context(|| format!("failed to open review database at {}", db_path.display()))?;

    log::info!("mcp-serve: using database {}", db_path.display());

    // One client on one pipe — a current-thread runtime is all this needs, and
    // it keeps the binary from pulling in the multi-threaded scheduler.
    // Timers are not optional: rmcp's service loop arms them for request
    // timeouts and panics on a runtime without them.
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
