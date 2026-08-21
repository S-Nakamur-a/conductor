# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Conductor is a terminal-based Git workspace and code review TUI written in Rust. It manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Commands

- **Build:** `cargo build`
- **Run:** `cargo run` or `cargo run -- <repo-path>` (defaults to current directory)
- **Test:** `cargo test --workspace` (tests are inline `#[cfg(test)]` modules in `git_engine.rs`, `config.rs`, `review_store.rs`)
- **Run single test:** `cargo test <test_name>` (e.g., `cargo test test_parse_full_config`)
- **Lint:** `cargo clippy --workspace`
- **Check:** `cargo check --workspace`

Bare `cargo test` / `cargo clippy` only cover the `conductor` package — the
`crates/revidere*` members need `--workspace`. `default-members` is deliberately
left alone so `cargo run` stays unambiguous.
- **Logging:** Set `RUST_LOG=debug` (or `info`, `warn`) before running

### MCP Server (`conductor mcp-serve`, `src/mcp_serve/`)

The review DB tools are served by the conductor binary itself over stdio — no
separate build step, no Node. `cargo install --path .` updates the binary and its
MCP tools together, which is the point: they used to be two artifacts on two
release channels and drifted apart.

- **Run by hand:** `conductor mcp-serve --db <path>` (speaks JSON-RPC on stdout)
- **Who starts it:** `plugins/conductor/.mcp.json`, for the interactive Claude Code
  sessions inside the TUI (it resolves the DB from `$CONDUCTOR_DB_PATH`, injected by
  `pty_manager/spawn.rs`). The AI review does *not* go through MCP — its
  artifact is a JSON file written by `revidere` (see `revidere.rs` below)
- **Tool contract:** the 7 `#[tool]` handlers in `src/mcp_serve/tools.rs`. Their doc
  comments become the JSON Schema descriptions the model reads, so changing one
  changes the tool's public contract — treat them as API, not commentary.

Never print to stdout from anything reachable by `mcp-serve`; it would corrupt the
protocol. Logging goes to stderr (env_logger's default).

### Session hook (`conductor cc-hook`, `src/cc_hook.rs`)

`/clear` rotates Claude Code's log to a **new session id**, and nothing on disk
links the old file to the new one — so a panel pinned to its spawn-time
`--session-id` would keep showing the pre-clear transcript forever. The fix is a
`SessionStart` hook that runs inside the panel's own Claude process and reports
its current session id back.

- **Who installs it:** `pty_manager/spawn.rs` writes `.conductor/claude-hooks.json`
  and passes it as `--settings` on every Claude spawn, plus `CONDUCTOR_PANEL_ID`
  and `CONDUCTOR_NOTIFY_SOCK` in the environment. `--settings` *adds a layer* —
  the user's own settings and the project's `.claude/settings.json` keep working.
- **Why the binary and not `plugins/`:** same reason `mcp-serve` lives here — a
  separately released plugin drifts, and the failure is silent (scrollback just
  shows stale history). `cargo install --path .` ships the binary and the hook
  together.
- **Fallback:** when the hook stays silent (hooks disabled, older CLI),
  `claude_sessions/rotation.rs` infers the rotation from the logs instead. It is
  deliberately conservative — see its module docs for what it refuses to guess.

### Review analyser (`crates/revidere`)

revidere turns a git diff into `<worktree>/.conductor/review.json`: every changed
line sorted into sections by importance, plus a coverage check that no changed
line is left unexplained. It lives in this repo as workspace members:
`crates/revidere` (the whole analyser plus the artifact types and `ReadingOrder`)
and `crates/revidere-fixtures` (shared test scaffolding).

- **One entry point:** `revidere::analyze(&Options, &dyn Ai)`. It has no binary and
  no CLI — conductor is the only caller (`app/revidere.rs`, on a worker thread).
- **The AI is injected.** revidere never spawns anything; conductor implements
  `revidere::Ai` over `ai_caller`, so the review runs on the same `[api]` config as
  every other AI feature. `provider = "gemini"` will *not* work: the prompt hands
  over the ledger only, and the model is expected to read the repository itself, so
  it needs an agentic CLI under `provider = "command"`.
- **Cache identity:** the stored-answer key includes `Ai::identity()`. If that ever
  goes constant, changing models silently returns the old model's answer.
- **Failing coverage is not a failure.** `analyze` returns the artifact either way;
  `review.coverage.is_complete()` is what distinguishes them. Treating an
  incomplete review as an error throws away a readable one.
- revidere writes nothing to stdout/stderr (the host owns a TUI) — progress goes
  through `log`.

## Architecture

### Application Structure

Single-struct state model: `App` in `app/` (`app/mod.rs`, with logic split across `app/review.rs`, `app/terminal.rs`, `app/worktree.rs`) holds all application state as flat fields. No ECS or component architecture.

**Main loop** (`main.rs`): 60fps event loop — polls crossterm events at 16ms, handles keys/mouse, checks file watcher, refreshes worktrees periodically (3s), scans Claude Code PTY output for file-change patterns.

**Event dispatch** (`event/`, dispatched from `event/mod.rs` into per-context submodules `global`/`explorer`/`viewer`/`terminal`/`worktree`/`overlay`/`mouse`): Overlay modes (worktree input, cherry-pick, branch switch, review input, etc.) take absolute priority and consume all keys. Otherwise, the `Focus` enum routes input to the focused panel. Terminal panels forward all keys except Esc directly to PTY.

### Layout

```
Title bar
Menu bar (full width)
Worktree monitor strip (full width)
Explorer | Viewer | Terminal (Claude Code / Shell)
Status bar
```

- The **menu bar** (`ui/menu_bar.rs`, `menu/`) is a permanent one-row strip of
  dropdown menus directly under the title bar — the browsable route to the same
  commands the palette and the keybindings run. `f10` focuses it. Unlike the
  worktree strip it stays visible while a panel is maximized. Menu rows carry a
  `CommandId` and go through `App::execute_palette_command`, so the menu holds
  no command logic of its own; `menu/model.rs` is taxonomy and labels only.

- The worktree list is **not** a column: it lives in a full-width monitor strip
  along the top (`ui/worktree_bar.rs`), showing every worktree's branch, dirty
  count, ahead/behind, and Claude Code waiting/active state. Hidden while a panel
  is maximized.
- The main area is a three-column accordion (`ui/layout.rs`): Explorer | Viewer |
  Terminal, with focus-driven widths. Any panel can be maximized (`Ctrl+Alt+Z`),
  and resized tmux-style with `Ctrl+Alt+Arrow` (ratios persist to config.toml).
- Explorer column is split 50/50 (file tree top, diff/comment list bottom).
- The revidere review view (`Focus::Revidere`, `w`) is *not* part of the
  accordion: it takes `main_area` whole as two columns (reading order | diff)
  and hides the terminal column. `ui/layout/render.rs` short-circuits there.
- Terminal column is split 80/20 vertically (Claude Code top, Shell bottom).
- When the embedded editor is active (`Focus::Editor`), it merges the
  Explorer+Viewer columns into one PTY panel.
- Tab cycles focus; panel-specific vim-style keys (j/k, h/l, g/G, /)

### Key Modules

| Module | Role |
|--------|------|
| `app/` | All application state and business logic methods (`mod.rs` + `review.rs`, `terminal.rs`, `worktree.rs`, `review_publish.rs`, `revidere.rs`) |
| `event/` | Keyboard/mouse event dispatch based on Focus and overlay state (per-context submodules) |
| `menu/` | Menu bar model (`model.rs` — which command sits under which menu), interaction state (`state.rs`), and availability predicates for the greyed-out rows (`enabled.rs`) |
| `git_engine.rs` | All git operations via `git2` (no shell-out) — worktrees, diffs, branches, cherry-pick, merge |
| `diff_state.rs` | Diff data model (file diffs, hunks, lines) using `similar` crate |
| `viewer/` | File tree model (`file_tree.rs`) and file content buffer (`file_view.rs`) |
| `review_store.rs` | SQLite persistence (`.conductor/conductor.db`) for reviews, sessions, templates, history |
| `pty_manager.rs` | PTY session management — spawn, read/write, resize; vt100 parser for rendering; output scanner for Claude Code |
| `file_watcher.rs` | Filesystem change detection via `notify` crate, debounced at 500ms |
| `instance_lock.rs` | `.conductor/conductor.lock` の flock による単独起動の担保 — リポジトリ (全 worktree 込み) につき 1 ウィンドウ。ロックは fd の寿命に紐づくので、クラッシュ後の後始末は要らない |
| `cc_notify.rs` / `cc_hook.rs` | Unix-socket channel from Claude Code hooks — waiting/active state, and the `SessionStart` report that keeps a panel's session id correct across `/clear` |
| `claude_sessions/` | Resolving which `.jsonl` transcript backs a panel (`rotation.rs` is the hook-less fallback for `/clear`) |
| `config.rs` | Config loading from `~/.config/conductor/config.toml` |
| `theme.rs` | Color themes (catppuccin-mocha default, dracula, nord, solarized-dark) |
| `term_caps.rs` | Terminal capability probing — OSC 11 background-colour query driving light/dark theme auto-selection |
| `pr_intake.rs` | Fetches a PR via `gh` and prepares its worktree for review (re-entrant: reuses an existing valid worktree) |
| `revidere.rs` | Loads the review artifact (`<worktree>/.conductor/review.json`) via the `revidere` library and builds its `ReadingOrder` — read-only, no AI |
| `app/revidere.rs` | Runs `revidere::analyze` on a worker thread (one per branch, cancellable), wires `ai_caller` into its `Ai` seam, and jumps from a section into the Viewer |
| `ui/revidere_view.rs` | The full-screen two-column review view (reading order \| diff) |
| `app/review_publish.rs` | Publishes review comments to GitHub via `gh`, tracking which comments are already posted |

### UI Modules (`src/ui/`)

Each file renders one panel or overlay popup. `common.rs` has shared rendering helpers including vt100-to-ratatui style conversion.

### Data Paths

- **Config:** `~/.config/conductor/config.toml`
- **Per-repo DB:** `<repo-root>/.conductor/conductor.db` (gitignored)
- **Review artifact:** `<worktree>/.conductor/review.json`, with the stored AI answers alongside it in `review-cache/` (gitignored)
- **Worktree dir:** `<repo-parent>/<repo-name>-worktrees/<branch-dir-name>`

## Conventions

- **Rust edition 2024**
- **Error handling:** `anyhow::Result` throughout; `log::warn!` for non-fatal errors
- **Navigation:** vim-style keybindings (j/k up/down, h/l collapse/expand, g/G top/bottom, / search, n/N next/prev)
- **Status messages:** Flash via `app.status_message = Some(...)`
- **Doc comments:** `//!` at module level, `///` on public items
