# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Conductor is a terminal-based Git workspace and code review TUI written in Rust. It manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Commands

`cargo build` · `cargo run [-- <repo-path>]` · `cargo test --workspace` ·
`cargo clippy --workspace` · `cargo check --workspace` · `make fmt`.
Set `RUST_LOG=debug` for logging.

**Always pass `--workspace`** — bare `cargo test` / `cargo clippy` only cover the
`conductor` package, not `crates/revidere*` or `crates/sheaf-core`.
`default-members` is deliberately left alone so `cargo run` stays unambiguous.

A pre-commit hook (`make hooks`, once per clone) runs `cargo fmt --all -- --check`.

### MCP Server (`conductor mcp-serve`, `src/mcp_serve/`)

The review DB tools are served by the conductor binary itself over stdio — no
separate build step, no Node. `cargo install --path .` updates the binary and its
MCP tools together, which is the point: they used to be two artifacts on two
release channels and drifted apart. `plugins/conductor/.mcp.json` starts it for
the Claude Code sessions inside the TUI, resolving the DB from
`$CONDUCTOR_DB_PATH` (injected by `pty_manager/spawn.rs`). The AI review does
*not* go through MCP — its artifact is a JSON file written by `revidere`.

- **Tool contract:** the `#[tool]` handlers in `src/mcp_serve/tools.rs`. Their doc
  comments become the JSON Schema descriptions the model reads, so changing one
  changes the tool's public contract — treat them as API, not commentary.
- **Never print to stdout** from anything reachable by `mcp-serve`; it would
  corrupt the protocol. Logging goes to stderr.

### Session hook (`conductor cc-hook`, `src/cc_hook.rs`)

`/clear` rotates Claude Code's log to a **new session id** and nothing on disk
links the old file to the new one, so a panel pinned to its spawn-time
`--session-id` would show the pre-clear transcript forever. A `SessionStart` hook
runs inside the panel's own Claude process and reports the current id back.
`pty_manager/spawn.rs` writes `.conductor/claude-hooks.json` and passes it as
`--settings` on every spawn (that *adds a layer*, so the user's own settings keep
working), plus `CONDUCTOR_PANEL_ID` and `CONDUCTOR_NOTIFY_SOCK`. It lives in the
binary rather than `plugins/` for the same reason `mcp-serve` does: a separately
released plugin drifts, and the failure is silent. When the hook stays quiet
(hooks disabled, older CLI), `claude_sessions/rotation.rs` infers the rotation
from the logs — deliberately conservative, see its module docs.

### Review analyser (`crates/revidere`)

revidere turns a git diff into `<worktree>/.conductor/review.json`: every changed
line sorted into sections by importance, plus a coverage check that no changed
line is left unexplained. `crates/revidere-fixtures` is shared test scaffolding.

- **One entry point:** `revidere::analyze(&Options, &dyn Ai)`. No binary, no CLI —
  conductor is the only caller (`app/revidere.rs`, on a worker thread).
- **The AI is injected.** revidere never spawns anything; conductor implements
  `revidere::Ai` over `ai_caller`, so the review runs on the same `[api]` config
  as every other AI feature. `provider = "gemini"` will *not* work: the prompt
  hands over the ledger only and the model must read the repository itself, so it
  needs an agentic CLI under `provider = "command"`.
- **Cache identity** includes `Ai::identity()`. If that goes constant, changing
  models silently returns the old model's answer.
- **Failing coverage is not a failure.** `analyze` returns the artifact either
  way; `review.coverage.is_complete()` distinguishes them.
- revidere writes nothing to stdout/stderr (the host owns a TUI) — use `log`.

### Code index (`crates/sheaf-core`, `src/semantic_index/`)

sheaf-core holds a SCIP index split per file, keyed by content hash, and answers
position queries (go-to-definition, find-references, go-to-implementation)
**with a confidence level attached**. It was developed in its own repository
(`../sheaf`) and vendored here as a workspace member; `src/semantic_index/` is
conductor's side of the seam. An LSP was evaluated and rejected — conductor's
jumps run at review time, so an LSP's residency buys nothing it needs.

**tree-sitter is not replaced — it is the layer underneath.** sheaf-core defines
the syntactic layer as a trait and ships no implementation, and will not consult
the index unless `token_at` answers. `semantic_index::bridge::Bridge` implements
that trait over `CodeMask` + `SymbolIndex`, so every position the index cannot
answer lands on today's answer, marked `Definition::Syntactic` rather than
`Exact`. Deliberately still tree-sitter-only: the symbol-action overlay, `gi`
when the index is silent, and the hover popup's reference count
(`count_references_upto`, capped at 50 — it runs on the UI thread).

Invariants. Breaking any of these turns a weak answer into a confident one:

- **The confidence cannot be bypassed.** `Definition` / `References` keep the
  positions inside their variants, so there is no way to get a `Location` without
  deciding which variant you got, and `Syntactic(vec![])` ("looked, found
  nothing") is distinct from `NotCode`. `lib.rs` pins this with
  ```compile_fail``` doctests — check they still fail to compile.
- **Provenance is all-or-nothing.** If any file an answer depended on changed
  since index time, the whole answer is dropped and the query falls through.
  Returning the surviving subset would hide the missing candidates behind an
  `Exact`.
- **Exact and Derived are never merged.** Go-to-implementation answers `Exact`
  from SCIP `relationships` where the producer emits them (scip-go,
  scip-typescript) and `Derived` from the symbol spelling where it does not
  (rust-analyzer emits none) — `store::impl_pair`. Merging would drag a
  producer-declared answer down to the weaker claim. The derived key is a bare
  spelling, so two same-named traits collide; that is why it is not `Exact`.
- When in doubt: never answer wrongly — no answer is better — and never let N
  worktrees multiply memory.

Two SCIP traps that typed access will not save you from:

- **`signature_documentation` is not what the scip crate says it is.** scip 0.9
  generates a `Signature` with the text at field 2; rust-analyzer and scip-go
  write the older spelling, a `Document` with the text at field 5. Typed access
  compiles and returns an empty string forever, so `store::signature_text` reads
  both. scip-typescript writes none — its declaration is a ```` ```ts ```` fence
  at `documentation[0]`, lifted out by `store::fenced_declaration`.
- **`kind`'s numbers are not the crate's.** The enum was renumbered (function is
  17 in scip 0.9, 24 in what the producers write), but the producers agree with
  *each other*, so `store/kind.rs` is one table with no tool name in it. Do not
  route these through `scip::types::Kind`, and do not reintroduce a per-tool
  table: scip-typescript writes no `kind` at all, and a tool-keyed table would
  silently answer `Unknown`. Producers that omit it fall to
  `kind::from_declaration`.

Where it lives: `<main worktree>/.conductor/{index.scip,index.hashes,index.log,
generate.lock}`. Linked worktrees have no `.conductor/`, so `semantic_index`
walks `git2::Repository::commondir()` to the main one — the same move
`mcp_serve::resolve` makes for the review DB. The lock is per repository on
purpose: one producer peaks at 2.36GB, so the cap must not split per worktree.

- **Generation is Rust only; reading is not.** `RustAnalyzer` is the sole
  producer conductor runs, gated on a root `Cargo.toml`, so a Go or TypeScript
  repository still answers entirely through tree-sitter. sheaf-core itself is
  producer-independent and verified against real `scip-go` / `scip-typescript`
  indexes; what is missing is the host side — choosing a producer per tree and
  holding several index roots at once.
- `Regenerator` rebuilds when edits go quiet for 3s, **ignoring gitignored
  paths** — without that, `target/` churn would reset the quiescence timer
  forever. That is why `FsEvent::Changed` carries a path. `Lock::acquire`
  distinguishes "someone else is generating" from "the directory is not there
  yet"; collapsing the two made every first-ever `conductor index` answer `Busy`.
- sheaf's `tests/{go,ts}_definition.rs` really launch `scip-go` / `npx` and
  **fail rather than skip** where absent. `#[ignore]`d tests need a real index —
  see `SHEAF_TEST_INDEX` / `SHEAF_TEST_ROOT`, and `CONDUCTOR_TEST_REPO` for
  `src/semantic_index/`'s own.

sheaf-core's comments, test names, and error messages are Japanese, and its
public API deliberately exposes no `protobuf` / `scip` types.

## Architecture

### Application Structure

`App` in `app/` holds all state as flat fields — no ECS, no components. `main.rs`
runs a 60fps loop: crossterm events at 16ms, file watcher, a worktree refresh
every 3s, and a scan of Claude Code's PTY output for file-change patterns.
`event/` dispatches per context: overlay modes (worktree input, cherry-pick,
branch switch, …) take absolute priority and consume all keys; otherwise `Focus`
routes to the focused panel, and terminal panels forward everything but Esc
straight to the PTY.

### Layout

```
Title bar / Menu bar / Worktree monitor strip   (all full width)
Explorer | Viewer | Terminal (Claude Code / Shell)
Status bar
```

A three-column accordion (`ui/layout.rs`) with focus-driven widths: Explorer
(50/50 tree over diff/comment list), Viewer, Terminal (80/20 Claude Code over
shell). `Ctrl+Alt+Z` maximizes, `Ctrl+Alt+Arrow` resizes tmux-style (ratios
persist), `Focus::Editor` merges Explorer+Viewer into one PTY panel. The menu bar
(`ui/menu_bar.rs`, `menu/`, `f10`) and the worktree strip (`ui/worktree_bar.rs`)
are full-width rows, not columns; only the menu bar survives a maximize. Menu
rows carry a `CommandId` and go through `App::execute_palette_command`, so
`menu/model.rs` is taxonomy and labels only, with no command logic.

- **The Viewer keeps several files open at once.** `ViewerState` owns a `tabs`
  list plus an active index; only the active tab's state lives in the flat
  `content`/`search`/`diff_view`/`selection` fields, and the rest is stashed on
  the tab it belongs to (`viewer/tabs.rs`) — one copy, never two. `open_file` is
  the single entry point and reuses an existing tab. The strip draws on the
  block's first inner row (`ui/viewer_panel/tab_row.rs`), the same slot the
  breadcrumb uses, so `screen_row_map` needs a placeholder for it.
- The revidere review view (`Focus::Revidere`, `w`) is *not* part of the
  accordion: it takes `main_area` whole as two columns (reading order | diff) and
  hides the terminal column. `ui/layout/render.rs` short-circuits there.

### Key Modules

| Module | Role |
|--------|------|
| `app/` | All application state and business logic (`mod.rs` plus `review.rs`, `terminal.rs`, `worktree.rs`, `review_publish.rs`, `revidere.rs`) |
| `event/` | Keyboard/mouse dispatch by Focus and overlay state (per-context submodules) |
| `menu/` | Menu taxonomy (`model.rs`), interaction state (`state.rs`), greyed-out predicates (`enabled.rs`) |
| `git_engine/` | All git operations via `git2`, no shell-out — worktrees, diffs, branches, cherry-pick, merge |
| `diff_state/` | Diff data model (file diffs, hunks, lines) over the `similar` crate |
| `viewer/` | File tree (`file_tree.rs`), content buffer (`file_view.rs`), open-file tabs (`tabs.rs`) |
| `review_store/` | SQLite persistence (`.conductor/conductor.db`) for reviews, sessions, templates, history |
| `pty_manager/` | PTY spawn/read/write/resize, vt100 parsing for rendering, output scanner for Claude Code |
| `claude_sessions/` | Which `.jsonl` transcript backs a panel (`rotation.rs` is the hook-less `/clear` fallback) |
| `cc_notify.rs` / `cc_hook.rs` | Unix-socket channel from Claude Code hooks — waiting/active state and the `SessionStart` session-id report |
| `semantic_index/` | conductor's side of sheaf-core (`mod.rs`) and the `SyntacticLayer` over tree-sitter (`bridge.rs`) |
| `revidere.rs` / `app/revidere.rs` | Loading the review artifact and building its `ReadingOrder` (read-only) / running `analyze` on a worker thread |
| `file_watcher.rs` | Filesystem changes via `notify`, debounced at 500ms |
| `instance_lock.rs` | `.conductor/conductor.lock` の flock による単独起動の担保 — リポジトリ (全 worktree 込み) につき 1 ウィンドウ。ロックは fd の寿命に紐づくので、クラッシュ後の後始末は要らない |
| `term_caps.rs` | OSC 11 background-colour probe driving light/dark theme auto-selection |
| `pr_intake.rs` | Fetches a PR via `gh` and prepares its worktree (re-entrant: reuses a valid one) |
| `config/` · `theme/` | Config loading from `~/.config/conductor/config.toml` · colour themes |

### UI Modules (`src/ui/`)

Each file renders one panel or overlay popup. `common.rs` has shared rendering helpers including vt100-to-ratatui style conversion.

### Data Paths

- **Config:** `~/.config/conductor/config.toml`
- **Per-repo DB:** `<repo-root>/.conductor/conductor.db` (gitignored)
- **Review artifact:** `<worktree>/.conductor/review.json`, with the stored AI answers alongside it in `review-cache/` (gitignored)
- **Code index:** `<main worktree>/.conductor/index.scip` plus `index.hashes` (provenance), `index.log`, `generate.lock` — one per repository, shared by every worktree
- **Worktree dir:** `<repo-parent>/<repo-name>-worktrees/<branch-dir-name>`

## Conventions

- **Rust edition 2024**
- **Error handling:** `anyhow::Result` throughout; `log::warn!` for non-fatal errors
- **Navigation:** vim-style keybindings (j/k up/down, h/l collapse/expand, g/G top/bottom, / search, n/N next/prev)
- **Code jumps target a word, not a line:** the Viewer has no in-line cursor, so
  `gd`/`gi`/`gr`/`gK` label every identifier on the top visible line and let you pick one
  (a single identifier jumps straight through). Picking the first one silently sent
  `pub use model::MenuItem;` to `model.rs:1` every time. Mouse Cmd+click needs no
  picker — it already carries the clicked column. The hover popup's footer rows are
  the mouse route: the location row jumps to the definition, the "N refs" row opens
  the list.
- **Status messages:** Flash via `app.status_message = Some(...)`
- **Doc comments:** `//!` at module level, `///` on public items
