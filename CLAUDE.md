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
`crates/revidere*` and `crates/sheaf-core` members need `--workspace`.
`default-members` is deliberately left alone so `cargo run` stays unambiguous.
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

### Code index (`crates/sheaf-core`, `src/semantic_index/`)

sheaf-core holds a SCIP index split per file, keyed by content hash, and answers
position queries (go-to-definition, find-references) **with a confidence level
attached**. It was developed in its own repository (`../sheaf`) and vendored here
as a workspace member. `src/semantic_index/` is conductor's side of the seam.

**tree-sitter is not replaced — it is the layer underneath.** sheaf-core defines
the syntactic layer as a trait and ships no implementation, and it will not even
consult the index unless `token_at` answers. `semantic_index::bridge::Bridge`
implements that trait over the existing `CodeMask` + `SymbolIndex`, so every
position where the index has nothing to say lands on today's answer, marked
`Definition::Syntactic` rather than `Exact`.

Why an index and not a language server: conductor's jumps only run at review
time, after the implementation is finished, so the thing an LSP's residency buys
— incremental re-analysis on every keystroke — is a requirement conductor does
not have. Measured on this repository, rust-analyzer peaks at 3.70GB
(`analysis-stats`) and does not persist its analysis, so a per-worktree or
per-focus server re-pays ~13s on every switch. The index is 14.5MB on disk and
1.10x that resident, and **one index serves every worktree** — freshness is
per-file by content hash, so a worktree on the same branch answers `Exact` and
only its edited files fall through.

- **The confidence cannot be bypassed.** `Definition` / `References` keep the
  positions inside their variants, so there is no way to get a `Location` without
  deciding which variant you got. Claims of different strength live in different
  types (`Exact` vs `Enclosing`, `Found::direct` vs `Found::via_interface`), and
  `Syntactic(vec![])` ("looked, found nothing") is distinct from `NotCode`
  ("not an identifier"). `lib.rs` pins this with ```compile_fail``` doctests —
  if you touch the public API, check they still fail to compile.
- **Provenance is all-or-nothing.** If any file an answer depended on differs from
  its state at index time, the whole answer is dropped and the query falls through
  to the syntactic layer. Returning the surviving subset would silently hide the
  missing candidates behind an `Exact`.
- **The syntactic layer cannot fake an `Exact`.** `SyntacticAnswer` has no variant
  that reaches it. A fallback answer stays visibly a fallback.
- **Design tie-breakers, in order:** (1) N worktrees must not multiply memory,
  (2) keep the producer's accuracy, (3) never answer wrongly — no answer is better.
- Comments, test names, and error messages are Japanese, and the public API
  deliberately exposes no `protobuf` / `scip` types (conductor pulls them in as
  dev-dependencies only, to fabricate index fixtures in tests).

Where the index lives and who builds it:

- `<main worktree>/.conductor/`, one set of `index.<lang>.{scip,hashes,log}` per
  index root, plus a single `generate.lock`. Linked worktrees have no `.conductor/`
  of their own, so `semantic_index` walks `git2::Repository::commondir()` to the
  main one — the same move `mcp_serve::resolve` makes for the review DB. The lock
  is per repository on purpose: one producer peaks at 2.36GB, so the cap must not
  split per index root or per worktree. The artifact names must, though — sharing
  one name means the second generation overwrites the first index.
- **The producer is chosen per index root** (`semantic_index/roots.rs`), by the
  marker file that names one: `Cargo.toml` → `RustAnalyzer`, `go.mod` → `ScipGo`,
  `tsconfig.json` → `ScipTypescript`. A tree with no marker is not indexed at all;
  pointing a producer at a tree it cannot recognise is worse than not running it,
  because rust-analyzer and scip-go both write an empty index and exit 0. Each root
  regenerates independently — a missing `scip-go` must not cost the Rust index.
- **Index roots are found by walking the tree, and only the one you are reading
  gets built.** Measured on a real monorepo: 109 roots (75 `tsconfig.json`, 19
  `Cargo.toml`, 15 `go.mod`), 324ms to enumerate. Building all of them would take
  tens of minutes, and again every time edits go quiet — so `note_open` requests
  exactly the root that owns the file in the Viewer, and only when that root has
  no index yet. `note_change` does the same for the root that owns the edit.
  `conductor index` is the way to build every root at once. Two consequences: a
  repository with no index yet stays syntactic until a file is opened (opening one
  is the only way to jump anyway), and `note_open` is called every frame from
  `tick_semantic_regeneration` rather than from the 12 places that open a file —
  missing one of those would be silent.
- Enumeration honours `.gitignore` and skips `node_modules` / `vendor`; without
  that, a committed `node_modules` alone contributes hundreds of roots. Nested
  `Cargo.toml` under a root one is a workspace member and is *not* a separate
  root (rust-analyzer covers it, and each producer peaks at 2.36GB); nested
  `go.mod` and `tsconfig.json` are module/project boundaries and *are*.
- `conductor index` builds the first ones (~14s for this repository). After that
  `Regenerator` rebuilds whenever edits go quiet for 3s. It ignores gitignored
  paths — without that, `target/` churn would reset the quiescence timer forever
  and the index would never be rebuilt. That is why `FsEvent::Changed` carries a
  path. Routine rebuilds stay silent; the status bar reports only the first index.
- **Staleness is reported, not repaired.** One index serves every worktree and
  freshness is per file by content hash, so a worktree whose files differ from the
  ones the index was generated against loses `Exact` on exactly those files — the
  diff you are reviewing. Editing anything in that root heals it (`note_change`
  regenerates against the tree you are in), but a read-only review never triggers
  that. So `note_open` returns [`Reading::Stale`] when `Store::is_current` says the
  open file is not the one the index describes, the status bar says so once, and
  **Repo ▸ Rebuild Code Index** is the manual repair. Rebuilding automatically was
  rejected: the index can only describe one tree, so bouncing between two worktrees
  would re-pay ~14s / 2.36GB each way — the per-switch cost that ruled out an LSP.
- **Every generation appends one `key=value` line to
  `.conductor/index-history.log`** (`semantic_index/history.rs`) — enough to
  reconstruct both the causality and whether the work was worth doing:

  ```
  … lang=go root=services/api trigger=change cause=services/api/handler/handler.go waited=3.1s took=0.2s result=ok documents=2 sources=+0~1-0
  … lang=go root=services/api trigger=change cause=services/api/handler/handler.go waited=3.1s took=0.2s result=ok documents=2 sources=none waste=no-source-change
  ```

  `cause` is the file that triggered it, so a line answers "opening *this* built
  *that*". `sources` is the provenance table before the producer ran against the
  one it wrote, **counted only over files of that root's language** — the table
  lists every file in the root, so a `.md` edit would otherwise read as "the
  sources moved" and hide a pointless rebuild. `sources=none` means the producer
  re-derived the same index from the same inputs, and that is what `waste=` names.
  The other waste markers are `stale-on-arrival(N)` (edits landed mid-generation,
  so the index was old the moment it was written) and `discarded` (a worktree
  switch threw the run away). A change to a file no producer reads writes no line
  at all, because it starts no generation. Separate from `index.<lang>.log`, which
  is the producer's own output and is overwritten each run. Capped at 512KB,
  oldest half dropped. Note that `Lock::acquire` distinguishes "someone
  else is generating" from "the directory is not there yet" — collapsing the two
  made every first-ever `conductor index` answer `Busy`.
- The index producers are external tools. sheaf's `tests/go_definition.rs` and
  `tests/ts_definition.rs` really launch `scip-go` / `npx` and **fail rather than
  skip** where they are absent. `#[ignore]`d tests need a real index; see
  `SHEAF_TEST_INDEX` / `SHEAF_TEST_ROOT`, and `CONDUCTOR_TEST_REPO` for
  `src/semantic_index/`'s own.

The hover popup resolves its **definition** through the index like `gd` does, so
the two never disagree about where a symbol lives. Its **reference count** stays
on tree-sitter's capped counter (`count_references_upto`, 50) — that one runs on
the UI thread, and `references_at` may fall through to a full tree walk (measured
157ms). The list behind "N refs" is a click, which already pays that walk today.
`hover_info::DefSite` is the seam: whoever resolved the position, the popup is
built the same way, and a site the index found needs no tree-sitter definition at
all (that absence is why hover used to stay silent on locals, fields and module
names).

Its **declaration and kind** come from the index too, through `describe_at`.
`SymbolInformation` carries both for every symbol rust-analyzer emits (measured:
656 of 656 own-crate positions across three files), and the declaration is the
producer's resolved type, not the source text — `let message: String` where the
line reads `let message = who.greet();`. Scraping the definition line is the
fallback, not the default. Two things make this harder than reading a field:

- **`signature_documentation` is not what the scip crate says it is.** scip 0.9
  generates a `Signature` with the text at field 2; rust-analyzer and scip-go both
  write the older spelling, a `Document` with the text at field 5. Typed access
  compiles and returns an empty string forever, so `store::signature_text` reads
  both. **scip-typescript writes no `signature_documentation` at all** — its
  declaration is a ```` ```ts ```` fence at `documentation[0]`, which
  `store::fenced_declaration` lifts out (the remaining entries stay as doc).
- **`kind`'s numbers are not the crate's.** The SCIP enum was renumbered, and the
  producers' numbers do not line up with scip 0.9's (function is 17 there and 24
  here). They do line up with **each other** — rust-analyzer and scip-go write the
  same older numbering, verified against both indexes — so `store/kind.rs` is one
  table with no tool name in it. Do not route these through `scip::types::Kind`,
  and do not reintroduce a per-tool table: scip-typescript writes no `kind`, and a
  table keyed on the tool would have silently answered `Unknown` for everything a
  given tool had not been observed writing. Producers that omit `kind` fall to
  `kind::from_declaration`, which reads the declaration's leading word (or
  TypeScript's `(method)` / `(property)` prefix).

The popup keeps one fixed shape so the reading order does not move per kind:
container on the left of the header, kind on the right, declaration under it, doc
under that, then the clickable location and "N refs". The container
(`app::types::App`) arrives as `SymbolDetail.container` — sheaf-core builds it, so
conductor never parses a SCIP symbol string. Three things it has to get right:

- **A local has no spelling of its own.** `local 3` carries the enclosing function
  in `SymbolInformation.enclosing_symbol` instead, and that is where the container
  comes from. Adding it took the measured coverage from 99 to 193 of the `Exact`
  answers in this repository — locals and parameters are most of the positions a
  reader hovers.
- **The separator comes from the file extension**, not the producer: `::` for
  `.rs`, `.` for everything else.
- **A backticked descriptor is a file for TypeScript and a generic type for
  Rust.** ``src/`greet.tsx`/Loud#`` should drop the file (the popup shows the
  location anyway); `` `Bridge<'a>`#`` must not be dropped, because the result
  would name a different type. So it is dropped only in namespace position, and
  anywhere else the formatter returns `None` rather than guess.

Pressing a jump key **on** a definition shows its references instead. That test is
`answer_points_at`: the index's own answer must name the position that was asked
about. It used to compare the top visible line against a tree-sitter name lookup
with a ±2-line tolerance, which meant it never fired for fields or locals and
misfired whenever the viewport had scrolled.

Go-to-implementation answers at two different strengths, and the variant says
which. **rust-analyzer emits no SCIP `relationships` at all** (measured: 0 of
22,434 `SymbolInformation` in this repository's index); scip-go and
scip-typescript both do. Where they exist, the reverse of `implementers` *is* the
answer and it comes back as `Implementations::Exact`. Where they do not, all that
is left is the spelling of the symbols themselves: `impl#[Loud][Greeter]` names
both sides, `store::impl_pair` reads the pair off it, and the answer is
`Implementations::Derived`. The two are never mixed into one list — merging them
would drag a producer-declared answer down to the weaker claim. Two consequences
worth knowing before touching the derived path:

- **The key is a bare spelling, not a symbol.** Two same-named traits in one
  repository would merge. That is why the answer is not `Exact`.
- **A generic impl has no block symbol.** `impl<'a> SyntacticLayer for Bridge<'a>`
  puts nothing but its methods in the index, so the landing point is the earliest
  definition under that `(trait, type, file)` — the `impl` line when the block
  symbol exists, the first method inside it otherwise. Both land in the right
  block; only the former lands on the right line.

Still on tree-sitter, deliberately: the symbol-action overlay (it shows
definitions, implementations and references together by name, so it moves when
that overlay moves), and `gi` itself whenever the index stays silent.

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
- The Viewer keeps several files open at once. `ViewerState` owns a `tabs` list
  plus an active index; only the active tab's state lives in the flat
  `content`/`search`/`diff_view`/`selection` fields, and the rest is stashed on
  the tab it belongs to (`viewer/tabs.rs`) — one copy, never two. `open_file`
  is the single entry point and reuses an existing tab. The strip is drawn on
  the block's first inner row (`ui/viewer_panel/tab_row.rs`), the same slot the
  breadcrumb uses, so `screen_row_map` gets a placeholder for it.
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
| `viewer/` | File tree model (`file_tree.rs`), file content buffer (`file_view.rs`), and the open-file tabs (`tabs.rs`) |
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
| `semantic_index/` | conductor's side of sheaf-core: where the index lives (`mod.rs`) and the `SyntacticLayer` implementation over tree-sitter (`bridge.rs`) |
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
- **Code index:** `<main worktree>/.conductor/index.<lang>.scip` plus `index.<lang>.hashes` (provenance) and `index.<lang>.log`, one set per index root, and a single `generate.lock` — one directory per repository, shared by every worktree. `index-history.log` records every generation
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
