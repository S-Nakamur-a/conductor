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

Single-struct state model: `App` in `app/` (`app/mod.rs`, with logic split across `app/review.rs`, `app/terminal.rs`, `app/worktree.rs`) holds all application state as flat fields. No ECS or component architecture.

**Main loop** (`main.rs`): 60fps event loop — polls crossterm events at 16ms, handles keys/mouse, checks file watcher, refreshes worktrees periodically (3s), scans Claude Code PTY output for file-change patterns.

**Event dispatch** (`event/`, dispatched from `event/mod.rs` into per-context submodules `global`/`explorer`/`viewer`/`terminal`/`worktree`/`overlay`/`mouse`): Overlay modes (worktree input, cherry-pick, branch switch, review input, etc.) take absolute priority and consume all keys. Otherwise, the `Focus` enum routes input to the focused panel. Terminal panels forward all keys except Esc directly to PTY.

### Layout

```
Title bar
Worktree monitor strip (full width)
Explorer | Viewer | Terminal (Claude Code / Shell)
Status bar
```

- The worktree list is **not** a column: it lives in a full-width monitor strip
  along the top (`ui/worktree_bar.rs`), showing every worktree's branch, dirty
  count, ahead/behind, and Claude Code waiting/active state. Hidden while a panel
  is maximized.
- The main area is a three-column accordion (`ui/layout.rs`): Explorer | Viewer |
  Terminal, with focus-driven widths. Any panel can be maximized (`Ctrl+Alt+Z`),
  and resized tmux-style with `Ctrl+Alt+Arrow` (ratios persist to config.toml).
- Explorer column is split 50/50 (file tree top, diff/comment list bottom).
- Terminal column is split 80/20 vertically (Claude Code top, Shell bottom).
- When the embedded editor is active (`Focus::Editor`), it merges the
  Explorer+Viewer columns into one PTY panel.
- Tab cycles focus; panel-specific vim-style keys (j/k, h/l, g/G, /)

### Key Modules

| Module | Role |
|--------|------|
| `app/` | All application state and business logic methods (`mod.rs` + `review.rs`, `terminal.rs`, `worktree.rs`, `review_publish.rs`, `walkthrough_view.rs`) |
| `event/` | Keyboard/mouse event dispatch based on Focus and overlay state (per-context submodules) |
| `git_engine.rs` | All git operations via `git2` (no shell-out) — worktrees, diffs, branches, cherry-pick, merge |
| `diff_state.rs` | Diff data model (file diffs, hunks, lines) using `similar` crate |
| `viewer/` | File tree model (`file_tree.rs`) and file content buffer (`file_view.rs`) |
| `review_store.rs` | SQLite persistence (`.conductor/conductor.db`) for reviews, sessions, templates, history |
| `pty_manager.rs` | PTY session management — spawn, read/write, resize; vt100 parser for rendering; output scanner for Claude Code |
| `file_watcher.rs` | Filesystem change detection via `notify` crate, debounced at 500ms |
| `config.rs` | Config loading from `~/.config/conductor/config.toml` |
| `theme.rs` | Color themes (catppuccin-mocha default, dracula, nord, solarized-dark) |
| `term_caps.rs` | Rich-mode terminal capability detection (truecolor / graphics protocol tiers) |
| `pr_intake.rs` | Fetches a PR via `gh` and prepares its worktree for review (re-entrant: reuses an existing valid worktree) |
| `walkthrough.rs` | AI walkthrough data model and generation trigger — spawns a headless `claude -p` session that saves its result via the MCP server |
| `app/walkthrough_view.rs` | Explorer walkthrough-view methods for `App` — step selection, jumping to a step's diff location, and the "viewed" file/step toggle |
| `app/review_publish.rs` | Publishes review comments to GitHub via `gh`, tracking which comments are already posted |

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
