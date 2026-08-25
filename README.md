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
pulling a PR in and publishing review comments, and **ccusage** (via npx) for the
title bar's token/cost display (`ccusage.enabled = true`).

## Installation

```sh
git clone https://github.com/S-Nakamur-a/conductor.git
cd conductor
make install
```

`make install` puts the `conductor` binary in `~/.cargo/bin/`. The MCP server and
the revidere review analyser ship inside that same binary, so there is nothing
else to install for them.

Then, in a Claude Code session:

```
/plugin marketplace add S-Nakamur-a/conductor
/plugin install conductor@conductor-marketplace
```

That sets up the MCP server (review comment DB and change summaries), the hooks
that detect a waiting Claude Code session, and the `/address-conductor-comment`
and `/explain-comment` skills.

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

The worktree list is not a column. Every worktree — branch, dirty count,
ahead/behind, and Claude Code waiting/active state — sits in the full-width
monitor strip along the top, so parallel sessions can be watched at a glance.
Click one to jump to it, or `[+]` to create one.

### Keybindings

Press **?** in the app for the full list; **Ctrl+p** (or **:** outside a
terminal) opens the fuzzy-searchable command palette, which reaches every
command by name.

| | |
|---|---|
| **Tab / Shift+Tab** | cycle panel focus (non-terminal panels) |
| **Alt+h / Alt+l** | cycle panel focus from anywhere, terminals included |
| **Alt+1…6** (or **⌘+1…6**) | focus a specific panel |
| **Ctrl+Alt+Z** / **Ctrl+Alt+Arrow** | maximize / resize the focused panel |
| **Ctrl+Tab / Ctrl+Shift+Tab** (or **Alt+] / Alt+[**) | switch worktree |
| **Ctrl+n** / **Ctrl+t** | new Claude Code session / new shell |
| **j/k** · **h/l** · **g/G** | up/down · collapse/expand · top/bottom |
| **/** · **Ctrl+g** | search · full-text (grep) search |
| **Esc** · **Ctrl+q** | back / close overlay · quit |

## Reviewing a Pull Request

PR review runs inside the normal Explorer/Viewer/Terminal accordion — there is
no separate mode to enter or leave, so every navigation and terminal keybinding
keeps working while you review.

*Review ▸ Review Pull Request…* fetches a PR (via `gh`) into a worktree and
focuses the changed-files list. `W` then runs **revidere**, the AI reviewer that
ships inside the binary: it sorts every changed line into sections by importance
and checks that no changed line is left unexplained. `w` opens the result as a
two-column reading-order view, and *Review: Publish Comments to GitHub* posts
your inline comments back to the PR.

Full walkthrough, including the symbol-jump keys and revidere's `[api]`
requirements: [docs/reviewing-a-pr.md](docs/reviewing-a-pr.md).
## MCP Server

The MCP server that exposes the review database to the Claude Code sessions
running inside the terminal is the `conductor` binary itself, via
`conductor mcp-serve` — there is no separate package to keep in sync with the
TUI's version. Installing the plugin configures it, but **`conductor` must be on
`$PATH`**: the plugin's `.mcp.json` invokes it by name, not by absolute path.

```sh
conductor mcp-serve --db /path/to/repo/.conductor/conductor.db   # run it by hand
```

## Configuration

Config file: `~/.config/conductor/config.toml`

All fields are optional with sensible defaults, so an absent config file is a
valid one. The full set of keys — with every field commented out at its default
— is in [docs/configuration.md](docs/configuration.md).

The sections are `[general]` (repo, main branch, shell, worktree directory,
Claude Code auto-resume), `[terminal]` (scrollback limits), `[viewer]` (syntax
theme, tab width), `[diff]` (unified / side-by-side, word diff), `[keybinds]`
(per-chord overrides layered over the defaults, globally or per panel),
`[ccusage]`, `[updates]`, and `[api]` (the LLM provider shared by every AI
feature).

## Development

```sh
cargo build               # build
cargo test --workspace    # test — bare `cargo test` skips the crates/ members
cargo clippy --workspace  # lint
make fmt                  # cargo fmt --all
```

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
