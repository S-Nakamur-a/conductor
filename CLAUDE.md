# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Conductor is a terminal-based Git workspace and code review TUI written in Rust. It manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Commands

- **Build:** `cargo build`
- **Run:** `cargo run` or `cargo run -- <repo-path>` (defaults to current directory)
- **Test:** `cargo test` (tests are inline `#[cfg(test)]` modules in `git_engine.rs`, `config.rs`, `review_store.rs`)
- **Run single test:** `cargo test <test_name>` (e.g., `cargo test test_parse_full_config`)
- **Lint:** `cargo clippy`
- **Check:** `cargo check`
- **Logging:** Set `RUST_LOG=debug` (or `info`, `warn`) before running

### MCP Server (plugins/conductor/mcp/conductor-comment/)

Node.js MCP server that exposes review DB tools to Claude Code sessions.

- **Build:** `cd plugins/conductor/mcp/conductor-comment && npm run build`
- **Dev:** `cd plugins/conductor/mcp/conductor-comment && npm run dev`

## Architecture

### Application Structure

Single-struct state model: `App` in `app.rs` holds all application state as flat fields. No ECS or component architecture.

**Main loop** (`main.rs`): 60fps event loop — polls crossterm events at 16ms, handles keys/mouse, checks file watcher, refreshes worktrees periodically (3s), scans Claude Code PTY output for file-change patterns.

**Event dispatch** (`event.rs`): Overlay modes (worktree input, cherry-pick, branch switch, review input, etc.) take absolute priority and consume all keys. Otherwise, the `Focus` enum routes input to the focused panel. Terminal panels forward all keys except Esc directly to PTY.

### Four-Column Layout

```
Worktree | Explorer | Viewer | Terminal (Claude Code / Shell)
```

- Worktree column shrinks when not focused
- Terminal column is split 80/20 vertically (Claude Code top, Shell bottom)
- Tab cycles focus; panel-specific vim-style keys (j/k, h/l, g/G, /)

### Key Modules

| Module | Role |
|--------|------|
| `app.rs` | All application state and business logic methods |
| `event.rs` | Keyboard/mouse event dispatch based on Focus and overlay state |
| `git_engine.rs` | All git operations via `git2` (no shell-out) — worktrees, diffs, branches, cherry-pick, merge |
| `diff_state.rs` | Diff data model (file diffs, hunks, lines) using `similar` crate |
| `viewer_state.rs` | File tree model and file content buffer |
| `review_store.rs` | SQLite persistence (`.conductor/conductor.db`) for reviews, sessions, templates, history |
| `pty_manager.rs` | PTY session management — spawn, read/write, resize; vt100 parser for rendering; output scanner for Claude Code |
| `file_watcher.rs` | Filesystem change detection via `notify` crate, debounced at 500ms |
| `config.rs` | Config loading from `~/.config/conductor/config.toml` |
| `theme.rs` | Color themes (catppuccin-mocha default, dracula, nord, solarized-dark) |

### UI Modules (`src/ui/`)

Each file renders one panel or overlay popup. `common.rs` has shared rendering helpers including vt100-to-ratatui style conversion.

### Data Paths

- **Config:** `~/.config/conductor/config.toml`
- **Per-repo DB:** `<repo-root>/.conductor/conductor.db` (gitignored)
- **Worktree dir:** `<repo-parent>/<repo-name>-worktrees/<branch-dir-name>`

## Conventions

- **Rust edition 2024**
- **Error handling:** `anyhow::Result` throughout; `log::warn!` for non-fatal errors
- **Navigation:** vim-style keybindings (j/k up/down, h/l collapse/expand, g/G top/bottom, / search, n/N next/prev)
- **Status messages:** Flash via `app.status_message = Some(...)`
- **Doc comments:** `//!` at module level, `///` on public items
