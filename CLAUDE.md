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

**A green test run from a second worktree proves nothing until `make wt-stamp` has
run there.** A crate's version feeds the hash that names its artifacts, so two
worktrees at the same `conductor-tui = 0.1.0` write the same
`deps/conductor_tui-<hash>` into the shared `target-dir`, and the later build
replaces the earlier one. `cargo test` runs whatever is at that path and exits 0:
measured here, the same filename answered 586 tests at one moment and 581 — another
branch's suite — the next. The false *errors* this also produces (`unresolved
import` in a crate you never touched) are the same cause, and `touch` only wins the
race back. `make wt-stamp` appends `-wt.<worktree dir name>` to the seven
`crates/*/Cargo.toml` versions; `make wt-reset` strips it before the pull request.
Commit the stamp — `git rebase` refuses to start with any unstaged change. Leave the
root version alone: a dependency's version already propagates downstream, so `-V`
can stay honest. When a count moves for no reason, check the count before you
re-diagnose the code.

CI checks `cargo fmt --all -- --check` and `cargo clippy --workspace -- -D warnings`
on every pull request — a warning is a failure, including one inherited from main. `.githooks/pre-commit` runs the same fmt check locally, but wiring
it up is each developer's own business — the repository does not install it.

### MCP Server (`conductor mcp-serve`, `crates/conductor-mcp/`)

The review DB tools and the code-index tools (`search_symbols` / `read_symbol`,
`crates/conductor-mcp/src/tools/symbols.rs`) are served by the conductor binary
itself over stdio — no separate build step, no Node. `cargo install --path .` updates the binary and its
MCP tools together, which is the point: they used to be two artifacts on two
release channels and drifted apart. `plugins/conductor/.mcp.json` starts it for
the Claude Code sessions inside the TUI, resolving the DB from
`$CONDUCTOR_DB_PATH` (injected by `conductor-svc`'s `pty/spawn.rs`). The AI review
itself is still made by `revidere`, not by MCP: `analyze_review` only asks the window
to start it, and `get_review` reads the artifact it wrote.

- **Tool contract:** the `#[tool]` handlers in `crates/conductor-mcp/src/tools.rs`.
  Their doc comments become the JSON Schema descriptions the model reads, so
  changing one changes the tool's public contract — treat them as API, not
  commentary.
- **The window tools** (`open_file` / `close_file` / `analyze_review`) ride the
  cc-notify socket as one `ui <json>` line (`cc_hook::UiRequest`), relayed by the
  PTY host like `cc-signal`. They can only answer *sent*: a host with no window
  drops the line, and it is deliberately not replayed on attach. An open_file /
  analyze_review request from another worktree switches the window there first —
  unless an editor is open, which the switch would kill; close_file does not, and
  only acts on the worktree being shown. The analyse confirmation it raises answers
  only to `y` / `n` / Esc, since it pops up while someone may be typing elsewhere —
  Enter and the selection keys are ignored, but a typed `y` still approves it. `get_review` reads
  `review.json` directly and needs no window.
  Adding a window tool runs an action at a moment nobody chose. Open one level of
  the callees of the action it reuses and check each side effect against someone
  who is typing, has an editor open, or is on another worktree: #403's three bugs
  were all visible that way (`follow_worktree` discards the editor, the top modal
  takes keys before the PTY, a diff result outlives a worktree switch).
- **Never print to stdout** from anything reachable by `mcp-serve`; it would
  corrupt the protocol. Logging goes to stderr, and `conductor-mcp/tests/no_stdout.rs` guards it.

### Session hooks (`conductor cc-hook` / `cc-signal`, `crates/conductor-core/src/cc_hook/`)

`/clear` rotates Claude Code's log to a **new session id** and nothing on disk
links the old file to the new one, so a panel pinned to its spawn-time
`--session-id` would show the pre-clear transcript forever. A `SessionStart` hook
runs inside the panel's own Claude process and reports the current id back over a
Unix socket, which the PTY host (below) listens on through `conductor-svc`'s
`watch/cc_notify.rs` and relays to the window. `cc-signal`
rides the same socket with the waiting/active state the monitor strip shows.
`pty/spawn.rs` writes `.conductor/claude-hooks.json` and passes it as `--settings`
on every spawn (that *adds a layer*, so the user's own settings keep working),
plus `CONDUCTOR_PANEL_ID` and `CONDUCTOR_NOTIFY_SOCK`.
They live in the binary rather than `plugins/` for the same reason `mcp-serve`
does: a separately released plugin drifts, and the failure is silent — and
waiting/active used to be plugin-only, so anyone without it had a dead strip.
`PostToolUse` is declared on top of the plugin's three events because it is the
only one that fires after a permission dialog is approved; without it the strip
stays "waiting" while the panel is working. **There is no hook-less fallback** —
the old log-shape inference is gone, so a panel whose hook stays quiet keeps its
spawn-time session id.

**The same settings file also declares `statusLine`**, as `conductor cc-statusline`
(`cc_hook/statusline.rs`), because the 5h / 7d usage reaches nothing on disk — only a
statusLine's stdin (measured on 2.1.310, grepped `~/.claude` for it). It writes the JSON
to `<runtime dir>/statusline/<session id>.json` for the session list, then runs the
user's own statusLine with the same stdin. A `--settings` statusLine replaces the
user's (measured), so cc-statusline looks the user's up on every run
(`.claude/settings.local.json`, `.claude/settings.json` in the stdin's
`workspace.project_dir`, then `$CLAUDE_CONFIG_DIR` or `~/.claude`): fixing it at spawn
would hide a later edit from conductor's panels only, while plain Claude picks it up.
`padding` and `refreshInterval` are not carried over. `CONDUCTOR_IN_STATUSLINE` marks
the relayed child, so a user command that calls conductor back is not relayed twice;
`pty/spawn.rs` removes an inherited one.

**The socket does not live in `.conductor/`.** A worktree path there overruns
`sun_path` (104 bytes on macOS) and neither `bind` nor `connect` gets through, so
it sits under `$XDG_RUNTIME_DIR` / `$TMPDIR` (falling back to `/tmp/conductor-<uid>`),
in a 0700 directory, named by a hash of the repository's `conductor_dir`. Keying
it off `conductor_dir` rather than the worktree root is what makes every linked
worktree share the one listener. Clients that cannot read the environment — the
plugin's shell scripts — find the path in `.conductor/cc-notify.path`, written
before the `bind`.

### PTY host (`conductor pty-host`, `crates/conductor-svc/src/pty/host.rs`)

Closing a PTY's master sends its child SIGHUP, so whoever holds the masters decides
whether a Claude panel survives the window closing. The window does not hold them:
the first window on a repository starts `conductor pty-host <repo>` under `setsid`,
and every window after it attaches to the same host. Closing the window — or the
terminal it ran in — leaves the work running; the next window gets each session's
current screen and carries on from there. Measured: a Claude turn kept calling tools
with no window open, and the window opened later showed it mid-tool.

- **The window holds a mirror, not the PTY.** `PtyStore` keeps the same API and its
  own vt100 parser, fed by bytes the host relays (`pty/wire.rs`: length-prefixed
  frames, JSON for control, raw bytes for output and keystrokes). The renderer still
  reads `PtyStore::screen` directly. The handshake hands over `state_formatted()` —
  input modes included, or bracketed paste and wheel forwarding are wrong after a
  reattach — behind a shell's raw history, so it can still reflow. The raw history is
  trimmed and may have lost its leading mode switches, which is why the current state
  always comes last.
- **The host answers what nobody else can while the window is away:** cursor-position
  queries (`CSI 6 n`), the alternate-screen SIGWINCH nudge, and cc-notify. It keeps
  each panel's session id and the last active/waiting for each cwd that still has a
  live Claude, and replays them on attach, which is what keeps the transcript and the
  strip right across a reopen.
- **Lock order is window → each session's parser.** Building the handshake under the
  window lock is what makes snapshot and stream meet with no gap and no repeat.
- **The child's environment is the window's, sent whole with each spawn.** The host's
  own environment belongs to whichever window started it.
- One per repository: `.conductor/pty-host.lock` (flock) and `<key>.pty.sock` beside
  the cc-notify socket. **A session goes to the host of the repository its working
  directory belongs to**, not the one the window opened with — `switch_repo` keeps the
  previous repository's sessions running, so a window talks to several hosts. A host
  talks to only one window, and a second one that reaches it (a switch into a repository
  another window has open) is answered `Busy` instead of taking over.
- **One window per runtime dir, and it attaches every host there.** The window lock is
  `window.lock` beside the sockets (`instance_lock::acquire_window`), so "one per Mac"
  holds only between windows that resolve the same `$XDG_RUNTIME_DIR` / `$TMPDIR` — the
  same ones that can see each other's hosts at all. After its own host, the window
  attaches every other `*.pty.sock` there (`PtyStore::attach_others`): that is what makes
  every repository's Claude answerable from the session list. It never starts, retries
  or shuts down those hosts — they are not its repository's — and skips one with no
  sessions, so the host can still exit; `Busy` (a window of an older version) and a
  silent host are only counted on the status line. It happens once, at start: a host
  freed later is picked up by reopening the window. The repository lock is still taken
  too, because an older window knows only that one.
- A session ends only with its own exit, closing its tab, **D** in the session list,
  deleting its worktree, and **Repo ▸ Quit and Stop Sessions**. The list's **D** takes
  the tab-closing path for this window's sessions; any other Claude gets SIGTERM, then
  SIGKILL after 3s, and only while `<pid>.json` and the process start time still name
  the process the list read (`registry::stop`). Editors are the exception and die
  with the window that opened them. The host exits
  once it has had no sessions and no window for 3s. **A host of another version is
  kept while it has sessions** (the status bar says so) and is told to `Shutdown`
  when it has none; the frame protocol carries its own number for that reason.
- **Not on Linux yet:** Claude Code (seen on 2.1.310) writes `procStart` there as
  `/proc/<pid>/stat` starttime ticks (`"37242291"`), not the ctime string macOS gets,
  and `registry` reads only the latter. So on Linux the list's **D** never signals a
  Claude outside this window (it reports "already ended"), and, as far as
  `live_session` reads, the list's pid-reuse check is skipped too. This window's own
  sessions are unaffected. `registry::tests` pins the first half.
- `--second-window`, tests, and a window whose host would not start run an
  in-process host over a socketpair, so there is one code path and those sessions
  end with the window.

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
- **The voice is part of the prompt, and it goes last.** `[review] style` (inline)
  or `style_path` (a Claude Code output-style file) is appended by
  `prompt::system`. Whatever voice the AI tool already carries loses to this
  prompt's own demand for verified fact, so naming it here is the only way it
  survives — and it only survives from the *end*: measured, the same text placed
  before the rules is overridden by them. The JSON shape and the standard of
  evidence are explicitly held constant in that block.
- **The base is guessed remote-first** (`git::guess_base`): `origin/HEAD`, then
  `origin/main` / `origin/master`, and only then the local branches. `git fetch`
  never advances a local `main`, so preferring it silently widened the review to
  include everything that landed upstream meanwhile.
- **Cache identity** includes `Ai::identity()` *and the composed prompt*, so
  changing the voice re-asks. If identity goes constant, changing models silently
  returns the old model's answer.
- **Failing coverage is not a failure.** `analyze` returns the artifact either
  way; `review.coverage.is_complete()` distinguishes them.
- revidere writes nothing to stdout/stderr (the host owns a TUI) — use `log`.

### Code index (`crates/sheaf-core`, `crates/conductor-core/src/semantic_index/`)

sheaf-core holds a SCIP index split per file, keyed by content hash, and answers
position queries (go-to-definition, find-references, go-to-implementation)
**with a confidence level attached**. It was developed in its own repository
(`../sheaf`) and vendored here as a workspace member. `semantic_index/` is
conductor-core's side of the seam; `conductor-tui/src/index.rs` owns it and turns
the heavy work into Tasks.

**tree-sitter answers where the words are, and nothing else.** sheaf-core defines
the syntactic layer as a trait and ships no implementation, and will not consult
the index unless `token_at` answers. `semantic_index::bridge::Bridge` implements
that one method over `CodeMask` (`conductor_core::syntax`). There is no name-based
fallback: a position the index cannot answer comes back `Definition::Unresolved`,
and `code_nav::unresolved` says why and when it will be answerable. The wait is read
per index root (`SemanticIndex::waiting_on`), never repo-wide — on a monorepo an any()
tells a Rust file "indexing now" while a Go root is building, and waiting never helps.
**Rebuild is only suggested where it can help**: with the index loaded and covering the
file, a missing answer means a symbol defined outside the tree, and rebuilding costs
14s / 2.36GB to change nothing. Guessing by name
looked like an answer and was not — hovering `rollbar` in a Go file printed a
TypeScript `const rollbar = useRollbar()` as the declaration, measured on a real
monorepo. Silence the user can act on beats a confident wrong jump.

Invariants. Breaking any of these turns a weak answer into a confident one:

- **The confidence cannot be bypassed.** `Definition` / `References` keep the
  positions inside their variants, so there is no way to get a `Location` without
  deciding which variant you got, and `Unresolved` ("a word, but no answer yet")
  is distinct from `NotCode` ("not a word at all"). `lib.rs` pins this with
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
- **A test over hand-written SCIP spellings proves nothing.** Any code that parses
  producer output must have at least one case pasted verbatim from a real index
  (`.conductor/index.<lang>.<key>.scip`). Counts grouped by descriptor *suffix*
  hide prefix grammar: `impl#[Store]load().` lands in the `().` bucket, which is
  how `display_name` shipped answering `[Store]load`.

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
- **Reading falls back to the generation that explains the file being read**
  (`IndexRoot::source`), and only to the newest one when none does. Requiring an
  exact key match would make one keystroke hide the whole index; taking the newest
  unconditionally rebuilt on every worktree switch, because an edit landing here
  while another tree was on screen moved the key without touching the open file.
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
  take all input (hover, references, inline threads) belong to
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
current one is dropped, and the generation is bumped when the *repository*
changes (`effect.rs`'s `switch_repo`, the only `bump_generation` call) — **not on
a worktree switch**. A result that outlives a worktree switch therefore still
arrives, so whoever installs it has to check it is still the one asked for.
The sender shares that generation through an `Arc<AtomicU64>` — copying the value
into `EventSender` makes every watcher signal after the first switch vanish.
`Services::spawn_unscoped` is the exception, used only by Smart Worktree: its result starts
Claude in a worktree and branch that already exist, so dropping it on a repository switch
would leave them with no session.

**A periodically spawned `Task` needs an in-flight guard.** On a repository with
175,388 tracked files the worktree list takes 25s, the working-tree diff 9s, and
the two whole-repository walks behind the Explorer 1.8s (the file-name index) and
0.58s (git status), against a 3s poll and a 500ms debounce. Spawning faster
than results return piles up one thread per second — measured 476 threads at 303%
CPU, with nothing ever settling, so the list and the Explorer stayed empty for
good. The spawn sites are `run.rs`'s poll, the Explorer's `refresh` (three tasks
under one guard), and the per-worktree change counts; each one grew this same hole
independently.

**PTY output is deliberately asymmetric.** Bytes are too frequent for the event
channel, so `PtyStore` (the mirror of the PTY host's screens) lives in svc and the
renderer reads it directly. There is no tokio; threads and mpsc are enough.

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
the accordion — it takes the main area whole as three columns (reading order | diff
| Claude), at fixed 24/38/38 that the accordion's ratios do not move.

**Sessions** (`panels/fleet/`, `Focus::Fleet`, alt+1) takes the main area too, and is
the screen conductor opens on: every Claude on the machine, built from the state
files Claude Code writes itself (`~/.claude/sessions/<pid>.json`, read by
`claude_sessions::registry` once a second on a worker) joined with this window's
PtyStore by conversation id. The hooks' waiting/active cannot be the source: they
stay "waiting" after a permission is approved until the tool finishes, and after an
Esc refusal until the next prompt (measured on 2.1.309). A row of this window is
keyed by its panel id, not the conversation id, because `/clear` changes the
latter. Rows hang under repository → worktree. The same worker lists the worktrees
(`GitEngine::worktree_heads`: path and HEAD only) and places each session dir under
the deepest worktree containing it, comparing canonicalised paths component-wise. A
repository once listed stays (`kept_roots`) so it can still be launched into after its
last session ends. When a selected row moves between the waiting box and its worktree,
the selection follows the session: re-selecting anything else would make
`apply_fleet` pull the keyboard out of the session being typed into.
**Whether the list is open is not derived from `Focus`**
(`chrome.fleet_open`): while it is, an `Effect::Focus` nobody made by hand is only
remembered as where Esc returns (`Workspace::screen_focus`, which `reviewing()` and
the focus cycle read), so a background result never moves the keyboard to a region
nobody can see, and window tools (`remote.rs`) do nothing but say so. Focus moves
made by hand — commands from keys, the palette or the menu, and the focus cycle —
go through `command::by_hand`, which turns them into `LeaveFleet`. The right side shows
one of this window's sessions live (`Focus::FleetSession`). Which one is derived from
the list's selection every time (`Workspace::fleet_session`), never stored, and
when the row being typed into disappears the keyboard goes back to the list rather
than to whatever got selected instead. That right side gives a PTY a second shape,
so `sync_sizes` compares each session's own grid with its region, not the region's
remembered size. Host notifications (Claude's
state, session ids) are sent with `send_watch_unscoped` so a repository switch does
not drop them; every other watch event keeps its generation.

### Key modules

| Module | Role |
|--------|------|
| `tui/route.rs` · `layout.rs` · `liveness.rs` | Where a key goes · where a region is · why the screen moves. All pure |
| `tui/workspace.rs` · `effect.rs` · `task.rs` | State, the effect vocabulary, every long-running job and its result |
| `tui/panels/<name>/` · `modal/<name>.rs` | One panel / one modal: its state, `update` and render, together. `panels/fleet/` is Sessions |
| `tui/command/` · `index.rs` · `markdown/` | The command table with `exec` and `enabled` · both indexes and their rebuilds · markdown, shared by the Viewer and the transcript |
| `svc/pty/` · `svc/watch/` | The PTY host (holds the masters), its window-side mirror and the vt100 screen · the filesystem, config and refresh-pipe watchers (cc-notify's listener runs in the PTY host) |
| `core/git_engine/` · `diff_state/` · `review_store/` | All git via `git2` bar the status shell-out below · the diff model over `similar` · SQLite for reviews, sessions and viewed files |
| `core/claude_log/` · `claude_sessions/` · `keymap/` · `theme/` · `config/` | The transcript `.jsonl` format · which one backs a panel · actions and layers · colour themes · `~/.config/conductor/config.toml` |
| `core/instance_lock/` | flock による単独起動の担保 — runtime dir の `window.lock` で窓は 1 つ (PTY host 節)、古い版の窓とは `.conductor/conductor.lock` でリポジトリにつき 1 つ。ロックは fd の寿命に紐づくので、クラッシュ後の後始末は要らない |
| `core/term_caps/` · `pr_intake/` | OSC 11 background probe and the icon-set guess · `gh` PR fetch into a worktree (re-entrant) |

Deliberately not abstracted: panels as a trait, an ECS, a widget tree over
ratatui, tokio/async, a subscription bus for `Effect`, a general shell-out layer
over git, and any rework of theme / keymap / text_input / config.

**The one place git is shelled out to is the working-tree status**
(`status_map.rs`'s `porcelain`). libgit2 consults neither `core.fsmonitor` nor
`core.untrackedCache`, so on 175,388 files `statuses()` costs 9.5s against
`git status --porcelain`'s 0.58s for the same answer, and no `StatusOptions`
narrows the gap — measured 9.0 / 12.5 / 14.1 / 10.2s across the combinations.
Nothing else may follow it out: the rest of git stays on `git2`.

## Data Paths

- **Config:** `~/.config/conductor/config.toml`
- **Per-repo DB:** `<repo-root>/.conductor/conductor.db` (gitignored)
- **Review artifact:** `<worktree>/.conductor/review.json`, with the stored AI answers alongside it in `review-cache/` (gitignored)
- **Code index:** `<main worktree>/.conductor/index.<lang>[.<root>].<key>.scip` plus `.hashes` (provenance) and `.log`, one set per index root per tree content (4 generations kept), and a single `generate.lock` — one directory per repository, shared by every worktree. `index-history.log` records every generation
- **PTY host log:** `<main worktree>/.conductor/pty-host.log` — the host's stderr; its socket sits beside the cc-notify one
- **statusLine copies:** `<runtime dir>/statusline/<session id>.json` — the last statusLine input of each Claude conductor started; the session list reads context use and 5h / 7d from them. Never cleaned up (about 2KB each, under `$TMPDIR`)
- **Unknown tools:** `<repo-root>/.conductor/unknown-tools.log` — one line per tool name `claude_log::classify` did not recognise, so a transcript that renders thin is traceable to the tool the table is missing (`mcp__*` names are excluded)
- **Worktree dir:** `<repo-parent>/<repo-name>-worktrees/<branch-dir-name>`

## Conventions

- **Rust edition 2024**
- **Error handling:** `anyhow::Result` throughout; `log::warn!` for non-fatal errors
- **Navigation:** vim-style keybindings (j/k up/down, h/l collapse/expand, g/G top/bottom, / search, n/N next/prev)
- **Code jumps target a word, not a line:** the Viewer has no in-line cursor, so
  `gd`/`gi`/`gr`/`gK` label every identifier on the top visible line and let you pick one
  (a single identifier jumps straight through). Picking the first one silently sent
  `pub use model::MenuItem;` to `model.rs:1` every time. Mouse Cmd+click needs no
  picker — it already carries the clicked column, and on a definition it opens the
  references instead. **Looking and going are split**: the hover popup only describes
  (kind, signature, doc, `path:line`), because a reference count costs an index query
  the UI thread pays on every pointer rest — measured 8–27ms for `Theme` (257
  references over 47 files). The declaration block and the `path:line` row both jump,
  and it stays put while the pointer is inside its rectangle.
- **A drag selection is bounded by where it started, not by where it is.** `Selection`
  carries the column range decided at mouse-down (`within_columns`), so `highlight`,
  `text` and `extend` all agree without any of them knowing about the Viewer. Pressing
  on the gutter keeps the old panel-wide rectangle; pressing on the code narrows it.
  The left edge is `gutter_zone`'s first `Zone::Text` plus `render::DIVIDER` — the zone
  reaches one divider further left than the text does, on purpose, so the `│` is easy to
  hit. The right edge drops a column **only when the scrollbar is actually drawn**
  (`render::scrollbar_covers`): `file_body` reserves nothing for it, so the bar overwrites
  the last column of code, and dropping it unconditionally would eat a real character on
  any file that fits on screen. Deliberately excluded: side-by-side diff (two gutters
  cannot be one range) and banner / thread rows (`Origin::Other`).
- **Status messages:** `Effect::Status(level, text)`
- **Doc comments:** `//!` at module level, `///` on public items
- **A regression test is not done until you have seen it fail.** Back the fix out (keep a
  copy, restore it right away), run the one test, and confirm the panic — a green test can
  pin nothing. One test here passed against a deliberately broken `state.rs` because
  `source_delta` returns `Sources::First` before it ever looks at the key; another passed
  because the root it meant to exercise was never in `self.roots` to begin with. Red proves
  as little as green: read the panic and confirm it names the behaviour you pinned. One test
  here failed *with* the fix in place, because its temp directory was not a git repository
  and `switch_repo` returned before reaching the line under test.
- **A test that names a file must put that file there.** Fixtures that only name paths pass
  for the wrong reason: `tour_stops`'s navigation tests pointed at a script whose files were
  never written, and stayed green until line-number validation arrived and failed three of
  them at once.
- **Throw away the test that only had to produce a number.** Before claiming a cap or a
  cache actually pays, write a scratch test in the crate, read it with
  `cargo test --release -- --nocapture`, then delete it — the measurement belongs in a
  comment, not in the suite. That is where `HIGHLIGHT_STASH`'s "本文の約 12 倍" came from;
  an estimate left in a comment gives the next person nothing to move the cap against.
- **Tests never wait on "quiet for N ms".** Use `conductor_tui::testing::wait_for(cond)`
  or `pump` (waits for `Services::in_flight() == 0`). A quiet-time wait only fails on a
  loaded machine; to prove a flake is pre-existing, check main out with
  `git worktree add --detach` and run both under `yes` × CPU count
