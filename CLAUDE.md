# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Conductor is a terminal-based Git workspace and code review TUI written in Rust. It manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Commands

`cargo build` · `cargo run [-- <repo-path>]` · `cargo test --workspace` ·
`cargo clippy --workspace` · `cargo check --workspace` · `make fmt`.
Set `RUST_LOG=debug` for logging.

**Always pass `--workspace`** — bare `cargo test` / `cargo clippy` only cover the
root `conductor` package, which is a five-line `main.rs`; everything else is in
`crates/`. `default-members` is left alone so `cargo run` stays unambiguous.

CI checks `cargo fmt --all -- --check` and `cargo clippy --workspace` on every
pull request. `.githooks/pre-commit` runs the same fmt check locally, but wiring
it up is each developer's own business — the repository does not install it.

### MCP Server (`conductor mcp-serve`, `crates/conductor-mcp/`)

The review DB tools are served by the conductor binary itself over stdio — no
separate build step, no Node. `cargo install --path .` updates the binary and its
MCP tools together, which is the point: they used to be two artifacts on two
release channels and drifted apart. `plugins/conductor/.mcp.json` starts it for
the Claude Code sessions inside the TUI, resolving the DB from
`$CONDUCTOR_DB_PATH` (injected by `conductor-svc`'s `pty/spawn.rs`). The AI review
does *not* go through MCP — its artifact is a JSON file written by `revidere`.

- **Tool contract:** the `#[tool]` handlers in `crates/conductor-mcp/src/tools.rs`.
  Their doc comments become the JSON Schema descriptions the model reads, so
  changing one changes the tool's public contract — treat them as API, not
  commentary.
- **Never print to stdout** from anything reachable by `mcp-serve`; it would
  corrupt the protocol. Logging goes to stderr, and `conductor-mcp/tests/no_stdout.rs` guards it.

### Session hook (`conductor cc-hook`, `crates/conductor-core/src/cc_hook/`)

`/clear` rotates Claude Code's log to a **new session id** and nothing on disk
links the old file to the new one, so a panel pinned to its spawn-time
`--session-id` would show the pre-clear transcript forever. A `SessionStart` hook
runs inside the panel's own Claude process and reports the current id back over a
Unix socket, which `conductor-svc`'s `watch/cc_notify.rs` listens on (it carries
the waiting/active state too). `pty/spawn.rs` writes `.conductor/claude-hooks.json`
and passes it as `--settings` on every spawn (that *adds a layer*, so the user's
own settings keep working), plus `CONDUCTOR_PANEL_ID` and `CONDUCTOR_NOTIFY_SOCK`.
It lives in the binary rather than `plugins/` for the same reason `mcp-serve`
does: a separately released plugin drifts, and the failure is silent. **There is
no hook-less fallback** — the old log-shape inference is gone, so a panel whose
hook stays quiet keeps its spawn-time session id.

### Review analyser (`crates/revidere`)

revidere turns a git diff into `<worktree>/.conductor/review.json`: every changed
line sorted into sections by importance, plus a coverage check that no changed
line is left unexplained. `crates/revidere-fixtures` is shared test scaffolding.

- **One entry point:** `revidere::analyze(&Options, &dyn Ai)`. No binary, no CLI —
  conductor-tui is the only caller (`task.rs`, on a worker thread).
- **The AI is injected.** revidere never spawns anything; conductor-tui implements
  `revidere::Ai` over `conductor_core::ai_caller`, so the review runs on the same
  `[api]` config as every other AI feature. `provider = "gemini"` will *not* work:
  the prompt hands over the ledger only and the model must read the repository
  itself, so it needs an agentic CLI under `provider = "command"`.
- **Cache identity** includes `Ai::identity()`. If that goes constant, changing
  models silently returns the old model's answer.
- **Failing coverage is not a failure.** `analyze` returns the artifact either
  way; `review.coverage.is_complete()` distinguishes them.
- revidere writes nothing to stdout/stderr (the host owns a TUI) — use `log`.

### Code index (`crates/sheaf-core`, `crates/conductor-core/src/semantic_index/`)

sheaf-core holds a SCIP index split per file, keyed by content hash, and answers
position queries (go-to-definition, find-references, go-to-implementation)
**with a confidence level attached**. It was developed in its own repository
(`../sheaf`) and vendored here as a workspace member. `semantic_index/` is
conductor-core's side of the seam; `conductor-tui/src/index.rs` keeps both indexes
and turns the heavy work into Tasks.

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

Where it lives: `<main worktree>/.conductor/`, one set of
`index.<lang>[.<root>].<key>.{scip,hashes,log}` per index root *per tree
content*, plus a single `generate.lock` and `index-history.log`. Linked
worktrees have no `.conductor/`, so `semantic_index` walks
`git2::Repository::commondir()` to the main one — the same move
`conductor_mcp::resolve` makes for the review DB. The lock is per repository on
purpose: one producer peaks at 2.36GB, so the cap must not split per index root
or per worktree. The names must, though, or the second generation overwrites the
first.

- **The producer is chosen per index root** (`semantic_index/roots.rs`) by the
  marker naming one: `Cargo.toml` → `RustAnalyzer`, `go.mod` → `ScipGo`,
  `tsconfig.json` → `ScipTypescript`. A tree with no marker is not indexed:
  pointing a producer at a tree it cannot recognise is worse than not running it,
  because rust-analyzer and scip-go both write an empty index and exit 0. Roots
  regenerate independently — a missing `scip-go` must not cost the Rust index.
  Enumeration honours `.gitignore` and skips `node_modules` / `vendor`; a nested
  `Cargo.toml` is a workspace member and *not* a root, nested `go.mod` /
  `tsconfig.json` are module boundaries and *are*.
- **Only the root you are reading gets built.** A real monorepo has 109 roots
  (75 `tsconfig.json`, 19 `Cargo.toml`, 15 `go.mod`); building all of them takes
  minutes, and again every time edits go quiet. `note_open` requests exactly the
  root owning the file in the Viewer, `note_change` the root owning the edit, and
  `conductor index` is the way to build every root at once. That is why
  `note_open` is called every frame from `index.rs` rather than from the places
  that open a file — missing one would be silent.
- **Generations are keyed by content, which is what makes worktrees cheap to move
  between.** The key folds the `(path, blob hash)` table of the files *that
  root's producer reads* — not every file under it, or swapping a `.png` would
  rename the artifact and rebuild an identical index. `IndexRoot::fold` is the
  single place that filter lives, because the same key has to come out of a tree
  walk and out of a provenance table; if the two disagreed, every generation
  would write a name the next read cannot find. Four are kept per root (14.5MB
  each here) and `prune` drops the rest by mtime, along with any keyless artifact
  an older conductor left behind.
- **Computing it walks the tree, so it never happens on the UI thread.**
  Enumerating the roots is 149ms and the heaviest single key 110ms on that
  monorepo. `survey()` does both on a worker (`Task::SurveyIndex`); until it
  lands, `note_open` answers `Reading::Loading` rather than answering from the
  previous tree's roots. `survey` only keys the roots it has a reason to — the one
  owning the file being read, the ones with artifacts on disk, and the ones
  `needs_survey` names — because keying all 109 costs 0.6s. `needs_survey`
  returning them *by name* is load-bearing: a keyless root the survey did not pick
  would keep asking to be surveyed, every frame. A root with no artifact is in
  neither set, so opening a file in one answers `NotIndexed`; `note_open` asks for
  one more survey there, the only way that root's first index ever starts.
- **What is already on disk is not rebuilt, and staleness heals itself.**
  Regeneration re-checks `has_generation` first, so an edit landing the tree back
  on already-indexed content writes `result=reused took=0.0s` and starts nothing.
  `note_open` asks a narrower question: it starts a producer only when what is
  loaded cannot explain the file being read. A moved tree key alone is not a
  reason — per-file provenance keeps the untouched files answering `Exact` out of
  the previous generation, and keying off the tree would spend ~14s / 2.36GB every
  time git moves the worktree. `Reading::Stale` is the narrow case regenerating
  will not fix: the index describes this exact content yet does not cover the open
  file. **Repo ▸ Rebuild Code Index** skips the 3s quiescence, since the person
  who pressed it is waiting. A root whose key is stale (`note_change` clears it)
  waits for the survey, or the index would be written under the name of content it
  no longer describes.
- **Reading falls back to the newest generation when no key matches**
  (`IndexRoot::source`). Requiring an exact match would make one keystroke hide
  the whole index, when per-file provenance already keeps the untouched files
  answering `Exact`.
- **Every generation appends one `key=value` line to `index-history.log`**
  (`semantic_index/history.rs`) — enough to reconstruct both the causality and
  whether the work was worth doing:

  ```
  … lang=go root=services/api trigger=change cause=services/api/handler.go waited=3.1s took=0.2s result=ok documents=2 sources=+0~1-0
  … lang=rust root=. trigger=change cause=src/lib.rs waited=0.2s took=0.0s result=reused
  ```

  `cause` is the file that triggered it, so a line answers "opening *this* built
  *that*". `sources` compares the provenance table before the producer ran against
  the one it wrote, **counted only over that root's language** — the table lists
  every file in the root, so a `.md` edit would otherwise read as "the sources
  moved" and hide a pointless rebuild. `sources=none` earns `waste=no-source-change`;
  the others are `stale-on-arrival(N)` (edits landed mid-generation, so the index
  was old when written) and `discarded` (a worktree switch threw the run away). A
  change to a file no producer reads writes no line at all. Capped at 512KB,
  oldest half dropped. Separate from the producer's own `index.<lang>.<key>.log`.
- Regeneration waits for edits to go quiet for 3s, **ignoring gitignored paths** —
  without that, `target/` churn would reset the quiescence timer forever. That is
  why `WatchEvent` carries a path. `Lock::acquire` distinguishes "someone else is
  generating" from "the directory is not there yet"; collapsing the two made every
  first-ever `conductor index` answer `Busy`.
- sheaf's `tests/{go,ts}_definition.rs` really launch `scip-go` / `npx` and
  **fail rather than skip** where absent. `#[ignore]`d tests need a real index —
  see `SHEAF_TEST_INDEX` / `SHEAF_TEST_ROOT`, and `CONDUCTOR_TEST_REPO` for
  `semantic_index/`'s own.

sheaf-core's comments, test names, and error messages are Japanese, and its
public API deliberately exposes no `protobuf` / `scip` types.

**A name-only answer never crosses languages.** `SymbolIndex::find_definitions`
takes the file being read and drops candidates whose language differs
(`symbol_index::same_language`; an unclassifiable extension is kept, because
dropping it would silently remove answers that work today). Without it, hovering
`rollbar` in a Go file answered with a TypeScript `const rollbar = useRollbar()`
and printed its declaration as if it were the answer — measured on a real
monorepo, and the symptom that started this work. Note what the index alone does
*not* fix: a third-party package has no definition inside the tree, so those
positions fall through to this layer no matter how good the index is. The right
answer there is silence (`No definition indexed for 'rollbar'`), which is what
the filter produces.

## Architecture

### Crates

```
crates/
  conductor-core/   the leaf domain layer (git, review DB, diffs, config, keymap,
                    theme, transcripts, both indexes, …). Knows nothing above it.
  conductor-svc/    the side-effect runner: threads + one mpsc, PtyStore, watchers.
  conductor-mcp/    conductor mcp-serve — the review DB over stdio.
  conductor-tui/    the screen: panels, modals, route / layout / Effect / render.
  sheaf-core/ revidere/   the SCIP index with confidence attached · the review analyser.
src/main.rs         five lines: hands this package's version to conductor_tui::entry::run.
```

Dependency direction: `tui → svc → core ← {sheaf-core, revidere}`. core knows
neither tui nor svc, and cargo enforces it. The version string comes from the
root `Cargo.toml` and is passed in, so `conductor -V`, the status line and the
self-update comparison all read one number.

### State

There is no `App`. `Workspace` owns the repo state, the worktree list, `Focus`,
the panels, a `Vec<Modal>` and the chrome (status, menu state, layout ratios).

- **A panel's `update` only receives its own state mutably.** Its signature is
  `fn update(&mut self, action, ctx: &Ctx) -> Vec<Effect>`, so an effect on any
  other panel can only be expressed in the return value. That is what keeps the
  old `impl App` scattered across 32 files structurally impossible.
- **Panels are not a trait.** The five are shaped too differently (revidere sits
  outside the accordion, terminal owns PTYs); a trait would return `Any` through `dyn`.
- **Modals are a stack, and only the top receives input.** Popups that do *not*
  take all input (hover, symbol actions, references, inline threads) belong to
  the panel's own state instead.
- `Ctx` is the read-only side (theme, keymap, config, repo, review, index, root,
  focus, key context, version), built in one place: `Workspace::split`.

### Input → update → render

```
Input ─route()─> Routed ─update()─> Vec<Effect> ─apply()─> Workspace ─render(&Workspace)─> Frame
```

- `route` is the only function deciding where a key goes: menu → the top modal →
  a panel's second chord key → PTY forwarding → keymap. **The default is to
  consume**, which keeps an IME's composing glyphs from leaking outward; it is a
  property of the top modal, not a rule repeated per stage.
- `Action::fires_in_terminal()` is the single source of truth for what a terminal
  panel does *not* forward to the PTY.
- **`render` takes `&Workspace` only.** Hit geometry is never a by-product of
  drawing: `layout(&Workspace, area) -> Layout` is pure, and both the renderer and
  the mouse hit test call it.
- `Effect` is one small enum, not a message bus — every destination is known at
  compile time.

### Long-running work

A `Task` goes to svc and a `TaskResult` comes back on **one mpsc**. Generation
matching happens once, in svc's `try_recv`: an event whose generation is not the
current one is dropped, and the generation is bumped on every worktree switch.
The sender shares that generation through an `Arc<AtomicU64>` — copying the value
into `EventSender` makes every watcher signal after the first switch vanish.

**PTY output is deliberately asymmetric.** Bytes are too frequent for the event
channel, so `PtyStore` lives in svc and the renderer reads it directly. There is
no tokio; threads and mpsc are enough.

`liveness(&Workspace, input_recent) -> Liveness { Idle, Active, Terminal }` is
the one definition of why the screen is moving, read by both the tick rate and
the dirty check.

### Layout

```
Title bar / Menu bar / Worktree monitor strip   (all full width)
Explorer | Viewer | Terminal (Claude Code / Shell)
Status bar
```

The middle row is a three-column accordion with focus-driven widths: Explorer
(file tree over the changed-files / comment list), Viewer, Terminal (Claude Code
over shell). `Ctrl+Alt+Z` maximizes, `Ctrl+Alt+Arrow` resizes tmux-style (ratios
persist), `Focus::Editor` merges Explorer+Viewer into one PTY region. Menu rows
carry a `CommandId` and run through the same `command::exec` as the palette and
every keybinding. The revidere reading view (`Focus::Revidere`, `w`) is *not* in
the accordion — it takes the main area whole as two columns (reading order | diff).

### Key modules

| Module | Role |
|--------|------|
| `tui/route.rs` · `layout.rs` · `liveness.rs` | Where a key goes · where a region is · why the screen moves. All pure |
| `tui/workspace.rs` · `effect.rs` · `task.rs` | State, the effect vocabulary, every long-running job and its result |
| `tui/panels/<name>/` · `modal/<name>.rs` | One panel / one modal: its state, `update` and render, together |
| `tui/command/` · `index.rs` · `markdown/` | The command table with `exec` and `enabled` · both indexes and their rebuilds · markdown, shared by the Viewer and the transcript |
| `svc/pty/` · `svc/watch/` | PTY spawn/read/write/resize and the vt100 screen · the filesystem, config, cc-notify and refresh-pipe watchers |
| `core/git_engine/` · `diff_state/` · `review_store/` | All git via `git2`, no shell-out · the diff model over `similar` · SQLite for reviews, sessions and viewed files |
| `core/claude_log/` · `claude_sessions/` · `keymap/` · `theme/` · `config/` | The transcript `.jsonl` format · which one backs a panel · actions and layers · colour themes · `~/.config/conductor/config.toml` |
| `core/instance_lock/` | `.conductor/conductor.lock` の flock による単独起動の担保 — リポジトリ (全 worktree 込み) につき 1 ウィンドウ。ロックは fd の寿命に紐づくので、クラッシュ後の後始末は要らない |
| `core/term_caps/` · `pr_intake/` | OSC 11 background probe and the icon-set guess · `gh` PR fetch into a worktree (re-entrant) |

Deliberately not abstracted: panels as a trait, an ECS, a widget tree over
ratatui, tokio/async, a subscription bus for `Effect`, a shell-out layer over
git, and any rework of theme / keymap / text_input / config.

## Data Paths

- **Config:** `~/.config/conductor/config.toml`
- **Per-repo DB:** `<repo-root>/.conductor/conductor.db` (gitignored)
- **Review artifact:** `<worktree>/.conductor/review.json`, with the stored AI answers alongside it in `review-cache/` (gitignored)
- **Code index:** `<main worktree>/.conductor/index.<lang>[.<root>].<key>.scip` plus `.hashes` (provenance) and `.log`, one set per index root per tree content (4 generations kept), and a single `generate.lock` — one directory per repository, shared by every worktree. `index-history.log` records every generation
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
- **Status messages:** `Effect::Status(level, text)`
- **Doc comments:** `//!` at module level, `///` on public items
