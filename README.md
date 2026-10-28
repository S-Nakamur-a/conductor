# Conductor

Terminal-based Git workspace and code review TUI written in Rust. Manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Prerequisites

| Dependency | Version | Notes |
|---|---|---|
| **Rust toolchain** | 1.88+ | Edition 2024. Install via [rustup](https://rustup.rs/) |
| **Git** | 2.x | Worktree operations (`git worktree add`, `git fetch`, …) |
| **Claude Code** | latest | `claude` must be in `$PATH` (`npm install -g @anthropic-ai/claude-code`) |
| **Node.js + npm** | 20+ | Required to install Claude Code |

Optional: **`gh`** ([install](https://cli.github.com/), then `gh auth login`) for
pulling a PR in and publishing review comments.

## Installation

```sh
git clone https://github.com/S-Nakamur-a/conductor.git
cd conductor
make install
```

`make install` puts the `conductor` binary in `~/.cargo/bin/`. The MCP server ships
inside that same binary, so there is nothing else to install for it.

Then, in a Claude Code session:

```
/plugin marketplace add S-Nakamur-a/conductor
/plugin install conductor@conductor-marketplace
```

That sets up the MCP server (review comment DB and change summaries), the hooks
that detect a waiting Claude Code session, and the `/address-conductor-comment`,
`/explain-comment` and `/revidere` skills. `/revidere` sorts the branch's changes
into sections by importance, checks that no changed line is left unexplained (and
shows any that are), and publishes the result as a review page (an Artifact) with
diff excerpts and links to GitHub.

## Usage

```sh
conductor                 # the current directory
conductor /path/to/repo   # a specific repo
make dev                  # cargo run, for development
```

## Layout

```
┌────────────────────────────────────────────────────────┐
│ Title bar                                               │
├────────────────────────────────────────────────────────┤
│ Worktree monitor strip (full width)                     │
├──────────────┬──────────────┬──────────────────────────┤
│              │              │ Claude Code              │
│  Explorer    │   Viewer     ├──────────────────────────┤
│ (tree + diff)│ (file/diff)  │ Shell                    │
├──────────────┴──────────────┴──────────────────────────┤
│ Status bar                                              │
└────────────────────────────────────────────────────────┘
```

The main area is a three-column accordion — **Explorer | Viewer | Terminal** —
with focus-driven widths. Explorer is split 50/50 (file tree / diff / comment
list) and Terminal 80/20 (Claude Code / shell). Any panel maximizes with
`Ctrl+Alt+Z` and resizes tmux-style with `Ctrl+Alt+Arrow`, and the ratios
persist to `config.toml`.

Dragging with the mouse selects inside one panel and copies to the clipboard on
release. In the Viewer the column you press decides what you get: start on the
line numbers and the whole panel width is copied, gutter and all; start on the
code and only the code comes along — no line numbers, no fold markers, no
scrollbar.

The worktree list is not a column. Every worktree — branch, dirty count,
ahead/behind, and Claude Code waiting/active state — sits in the full-width
monitor strip along the top, so parallel sessions can be watched at a glance.
Click one to jump to it, or `[+]` to create one. The dirty count is filled in
after the list appears — `~?` means it is still being counted, and deleting a
worktree in that state asks as if there were uncommitted changes.

### Sessions

conductor opens on **Sessions**: every Claude Code session running on the machine,
in any repository and in any window or terminal, one line each, under the worktree
(shown by its branch) of the repository it runs in, with the ones waiting for you (a
question or a permission) repeated at the top. Every worktree of the repository is
listed, those with no session too, and a repository stays listed until the window
closes once a session has run in it, or once this window opened it. Sessions in a plain
terminal are listed together last. The line above the list counts the sessions in each
state, and at its right end shows the 5h and 7d usage limits with the time left until
each resets. A line shows the state (waiting / working / finished and not yet looked at
/ idle), how long it has been in it, the session's title and how much context it
holds — as a bar and a percentage for sessions conductor started, as a token count for
the rest. Both come from Claude Code's statusLine: sessions conductor starts run
`conductor cc-statusline` as theirs, which keeps a copy and then runs your own
statusLine command with the same input. It is re-read from your settings every time
Claude redraws the status line, so an edit shows from the next redraw; its `padding`
and `refreshInterval` are not carried over. Usage appears once a session conductor
started has drawn its status line.

**j** / **k** move one line; **J** / **K** (or **Shift+↓** / **Shift+↑**) jump to the
next repository, or back to the top of the current one. Clicking a worktree's or a
session's line selects it, and a double click on a session does what **Tab** does.

Below the list, **Git Changes** shows the files the selected line's branch changed and
by how much, the same way the main screen's Git Changes does: from where it left the
repository's default branch to the working tree, uncommitted and untracked files
included. It is for reading only, is re-read about every five seconds while the list is
open, and is not shown when the window is too short.

**Ctrl+t** puts a shell of the selected line's worktree there instead and steps into
it, starting one only if that worktree has none running; **Ctrl+g** brings Git Changes
back and returns to the list, from inside the shell too. **Ctrl+Esc** steps
back to the list and leaves the shell showing. The shell is the same one the main
screen's Shell panel shows for that worktree, and it keeps running when Git Changes
comes back. It has no scrollback or tabs here, and is not started for a line outside
the listed git worktrees.

A session this window runs shows live on the right: **Tab** steps into it to
type, paste or drop a file there, and **Ctrl+Esc** steps back to the list. Showing it
there resizes it, and a resize drops what had scrolled off, there and in the Claude
panel alike. Scrolling up over it (the wheel works without stepping in, **Shift+PageUp**
once you have) opens the transcript view of that session, the same one the Claude panel
uses; scrolling down past the end or **Esc** goes back to it live, and so does
**Ctrl+Esc** once you have stepped in.
`Enter` opens that session's worktree in the main screen with the session in
the Claude panel; **Alt+1** (or **⌘+1**) comes back from anywhere. Sessions that
a plain terminal runs, or that a window of an older version still holds, are listed with
their state, but cannot be opened or typed into from here. The states come from the files
Claude Code writes under `~/.claude/sessions/`. With more than one Claude Code account
(one `CLAUDE_CONFIG_DIR` each), list them as `[[claude.accounts]]`
([docs/configuration.md](docs/configuration.md)): every account's sessions are read, and
each line shows its account's name in that account's color.

**Ctrl+n** starts a new session in the selected worktree — or, on a session's line, in the
worktree that session runs in. It asks first, and answers only to a digit: **1** Claude
Code, **2** Claude Code with Chrome, **3** Smart Worktree (in a git repository: it asks
for the task, creates a worktree and a branch for it, and starts Claude there). Any
other key closes the question. Ctrl+n is never part of typed text, so typing into the
list does not open this menu. The new session is selected
and the keyboard goes into it on the right. The `[+]` at the right end of a worktree's
line does the same as 1 and 2, and the `[✧]` beside a repository's name starts Smart
Worktree there. The result of a Smart Worktree started before switching to another
repository still arrives.

**g** opens the selected line's pull request in the browser: one whose head is the branch
checked out in that worktree (on a session's line, the worktree it runs in), as
`gh pr list --head` finds it — an open one first, otherwise a merged or closed one.
Pull requests from forks that happen to use the same branch name are not counted.
Without one — no pull request, not on a branch,
or gh cannot answer (not installed, not logged in, not GitHub) — it opens the repository
that `origin` points to, and the status line says which and why. It works on any
repository's line, not only the one this window opened. To reach the top of the list,
press **K** until it stops moving.

The list's bottom row names the keys that work on the selected line. **D** (Shift+d)
stops that session, wherever it runs, after asking. A `y` answers yes — upper case,
with Ctrl held, or with Alt held where the terminal sends Alt as a modifier, too. ← highlights
Yes and → highlights No, and Enter answers the one highlighted, which starts on No; any
other key cancels, `h` and `l` included. Keys the list does not use
are dropped, so a sentence typed into the list by mistake stops nothing unless a
capital D is followed by a y somewhere in it ("Dynamic", "READY"), or by arrow keys
ending in ← and then Enter; it then stops
whichever line is selected at that moment, which any j, k or G earlier in the
sentence has moved. A session elsewhere gets SIGTERM, and SIGKILL if it is still there
3s later, and only while it is still the process the list showed. On Linux, Claude Code
records its start time in a form conductor does not read yet, so **D** cannot stop a
session elsewhere there: it reports the session as already ended and leaves it running.

**D** on a worktree's line deletes that worktree and its branch. It first checks the
worktree, then asks — but only if the list still has the keyboard and the same line is
selected when the check returns; otherwise it says so and you press D again. The
question counts what is lost: uncommitted changes (ignored files such as `.env` are
not counted, and are deleted with the worktree), commits not on `origin/<default
branch>` (a squash-merged pull request shows here too), and the sessions that will
stop — every Claude whose working directory is inside the worktree, in this window or
not (outside this window, only those Claude Code has written a state file for), and
the shells this window opened there. On yes it closes this window's tabs,
stops the other Claudes the same way **D** stops one, and deletes only after each
Claude with a state file has ended (a Claude with no state file yet has its tab closed
but is not waited for) and a second count finds no new commit, no more uncommitted
changes and no newly started Claude; otherwise nothing is deleted. It refuses up
front, stopping nothing, for the main worktree, a directory git does not list as a
worktree, a locked worktree, one with another worktree inside it, one with an editor
open in it, and the worktree this window has open as its repository. A worktree on the default branch is deleted but the
branch is kept.

### Keybindings

Press **?** in the app for the full list; **Ctrl+p** (or **:** outside a
terminal) opens the fuzzy-searchable command palette, which reaches every
command by name.

| | |
|---|---|
| **Tab / Shift+Tab** | cycle panel focus (non-terminal panels) |
| **Alt+h / Alt+l** | cycle panel focus from anywhere, terminals included |
| **Alt+1** (or **⌘+1**) | Sessions — every Claude on the machine |
| **Alt+2…6** (or **⌘+2…6**) | focus a specific panel (**Ctrl+w** the worktree strip) |
| **Ctrl+Alt+Z** / **Ctrl+Alt+Arrow** | maximize / resize the focused panel |
| **Ctrl+Tab / Ctrl+Shift+Tab** (or **Alt+] / Alt+[**) | switch worktree |
| **Ctrl+n** / **Ctrl+t** | new Claude Code session / new shell (in the Sessions list, Ctrl+n opens its own menu and Ctrl+t the selected worktree's shell under the list) |
| **j/k** · **h/l** · **g/G** | up/down · collapse/expand · top/bottom (in the Sessions list, g opens the pull request) |
| **/** · **Ctrl+g** | search · full-text (grep) search (in the Sessions list, Ctrl+g shows Git Changes under the list) |
| **Esc** · **Ctrl+q** | back / close overlay · quit |

To drive Chrome from Claude, start the session with *Terminal ▸ New Claude Code
Session (Chrome)* (or "New Claude Code (Chrome)" in the palette), pick
"Claude Code + Chrome" from the `[+]` on the Claude Code tab row, or choose 2 in the
Sessions list's new-session menu (Ctrl+n, or the `[+]` on a worktree). It launches
`claude --chrome`. Every other start runs without it: Ctrl+n outside the Sessions list, a double click on
an empty pane, a smart session, the startup auto-resume and the resume after
grabbing a branch. To add Chrome to a session that ran without it, close its tab
and reopen it from *Resume Claude Session*, choosing "Resume + Chrome".

Quitting (Ctrl+q) does not stop the Claude Code and shell sessions: a background
`conductor pty-host` holds them, so a turn that is mid-task keeps working and the
next window you open on the repository shows each one where it has got to. To end
them as you leave, use *Repo ▸ Quit and Stop Sessions*, or **D** on one line of
Sessions. Deleting a worktree stops the sessions in it; a `$EDITOR` pane still
closes with the window.

## Reviewing a Pull Request

PR review runs inside the normal Explorer/Viewer/Terminal accordion — there is
no separate mode to enter or leave, so every navigation and terminal keybinding
keeps working while you review.

*Review ▸ Review Pull Request…* checks the PR's branch out of `origin` into a
worktree (via `gh`; an existing one is pulled up to date) and focuses the
Explorer on its review comment list. *Review: Publish Comments to GitHub* posts your inline
comments back to the PR. For an AI review of the branch, run the `/revidere` skill
in a Claude Code session there: it publishes the review as an Artifact page.

Full walkthrough, including the symbol-jump keys:
[docs/reviewing-a-pr.md](docs/reviewing-a-pr.md).
## MCP Server

The MCP server that exposes the review database to the Claude Code sessions
running inside the terminal is the `conductor` binary itself, via
`conductor mcp-serve` — there is no separate package to keep in sync with the
TUI's version. Installing the plugin configures it, but **`conductor` must be on
`$PATH`**: the plugin's `.mcp.json` invokes it by name, not by absolute path.

```sh
conductor mcp-serve --db /path/to/repo/.conductor/conductor.db   # run it by hand
```

Besides the review-comment tools, the server reads the code index conductor
builds (`.conductor/index.<lang>.<key>.scip`): `search_symbols` lists definitions
by name, kind, or file (one line each: `path:line kind Type::name [first-last]`
plus the signature), and `read_symbol` returns one definition's doc comment,
attributes and body without reading the whole file. A file edited since the
index was built gets no line numbers — the tools say so instead of guessing.
Markdown, TOML and other non-indexed files stay invisible to them; use grep.

## Configuration

Config file: `~/.config/conductor/config.toml`

All fields are optional with sensible defaults, so an absent config file is a
valid one. The full set of keys — with every field commented out at its default
— is in [docs/configuration.md](docs/configuration.md).

The sections are `[general]` (repo, shell, worktree directory,
Claude Code auto-resume), `[terminal]` (scrollback limits), `[viewer]` (syntax
theme, tab width), `[diff]` (unified / side-by-side, word diff), `[ui]` (colour
theme, contrast, icons, startup animation), `[layout]` (column and split
proportions), `[updates]` (the GitHub release check), `[keybinds]` (per-chord
overrides layered over the defaults, globally or per panel), `[api]` (the
LLM provider shared by every AI feature), and `[review]` (the repositories where a
PR that requests your review is checked out into a worktree on its own).

## Development

```sh
cargo build               # build
cargo test --workspace    # test — bare `cargo test` skips the crates/ members
cargo clippy --workspace  # lint
make fmt                  # cargo fmt --all
```

The root package is a five-line `main.rs`; the code lives in `crates/` —
`conductor-core` (domain), `conductor-svc` (threads, PTYs, watchers),
`conductor-tui` (the screen), `conductor-mcp` (the MCP server), plus `sheaf-core`
(the code index).

### Working in several worktrees at once

Cargo puts a crate's version into the hash that names its build artifacts, so two
worktrees holding the same version of `conductor-tui` write the *same* file into a
shared `target-dir` — and the one that builds second silently replaces the other.
`cargo test` then runs a binary from the wrong worktree and reports `ok`, so the
green tells you nothing.

Give each worktree its own versions right after you create it:

```sh
make wt-stamp   # crates/*/Cargo.toml -> <version>-wt.<worktree dir name>
make wt-reset   # strip it again, before you open the pull request
```

Commit the stamp as the first commit on the branch rather than leaving it
uncommitted — `git rebase` refuses to run with *any* unstaged change, so a
permanently dirty `Cargo.toml` blocks every rebase. The root `conductor` version is
left alone, which keeps `conductor -V` honest; a dependency's version already
propagates into everything downstream of it. Third-party crates keep their versions
and so stay shared in the cache — only the seven workspace crates rebuild.

`make install` from a linked worktree installs as `conductor-<worktree dir name>`
rather than `conductor`, so several finished worktrees can sit side by side in
`~/.cargo/bin` and you can tell which build you are running. The main worktree
still installs as plain `conductor`, and builds keep using the shared `target-dir`,
so splitting the name costs no build time. Those extra binaries are outside cargo's
own bookkeeping: remove one with `rm ~/.cargo/bin/conductor-<name>`, not
`cargo uninstall`.

One conductor window shows the sessions of every repository, so only one may be open
at a time; a second one is refused before it touches the terminal. When it starts it
attaches to every repository's PTY host that is not held elsewhere, and you can answer
any of those Claude sessions from the session list. A window of an older version still
holds its repository's host; close it and reopen the new window to pick that host up.
What lives under `.conductor/` — the review database, the cc-notify socket, the
refresh FIFO — likewise assumes a single owner per repository. To try a build without closing
the window you already have, start the second one with `--second-window`. It takes no lock and
*declines* those per-repository channels rather than taking them over, so the first
window keeps working. The trade is that the second window has no MCP refresh and no
Claude Code session hooks — its monitor strip stays blank and a panel's `/clear` is
not followed — and its sessions end with it, while the review database and the code
index stay shared as usual.

CI checks formatting and clippy on every pull request. `.githooks/pre-commit`
runs the same `cargo fmt --all -- --check` locally if you want to catch it before
pushing — wiring it up is left to you, since hook setups vary. Pointing git at it
directly is `git config core.hooksPath .githooks`; if you already run your own
hook manager, call the script from there instead.

## Data Paths

| Path | Description |
|---|---|
| `~/.config/conductor/config.toml` | User configuration |
| `<repo-root>/.conductor/conductor.db` | Per-repo review database (gitignored) |
| `<repo-parent>/<repo-name>-worktrees/` | Default worktree directory |

## License

MIT
