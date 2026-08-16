# Conductor

Terminal-based Git workspace and code review TUI written in Rust. Manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Prerequisites

### Required

| Dependency | Version | Notes |
|---|---|---|
| **Rust toolchain** | 1.85+ | Edition 2024. Install via [rustup](https://rustup.rs/) |
| **Git** | 2.x | Used for worktree operations (`git worktree add`, `git fetch`, etc.) |
| **Claude Code** | latest | `claude` CLI must be in `$PATH`. Install via `npm install -g @anthropic-ai/claude-code` |
| **Node.js + npm** | 20+ | Required for Claude Code installation |

### Optional

| Dependency | Purpose | How to enable |
|---|---|---|
| **ccusage** (via npx) | Token usage / cost display in title bar | Set `ccusage.enabled = true` in config |
| **GitHub CLI** (`gh`) | Pulling in a PR for review, and publishing review comments to GitHub | [Install](https://cli.github.com/) and run `gh auth login` |

## Installation

### 1. Install the binary

```sh
git clone https://github.com/S-Nakamur-a/conductor.git
cd conductor
make install
```

`make install` installs the `conductor` binary to `~/.cargo/bin/` (`cargo install --path .`). The MCP server and the revidere review analyser ship inside this same binary, so there's nothing else to install for them.

### 2. Install the Claude Code plugin

In a Claude Code session, run:

```
/plugin marketplace add S-Nakamur-a/conductor
/plugin install conductor@conductor-marketplace
```

This sets up:
- **MCP server** — review comment DB integration and change summaries
- **Hooks** — waiting-state detection for Claude Code sessions
- **Skills** — `/address-conductor-comment` (resolve review comments), `/explain-comment` (annotate hard-to-understand code with inline comments)

## Usage

```sh
# Run against the current directory
conductor

# Run against a specific repo
conductor /path/to/repo

# Or use make for development
make dev
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
with focus-driven widths. The worktree list is no longer a column: every
worktree (branch, dirty count, ahead/behind, and Claude Code waiting/active
state) is shown in the full-width **worktree monitor strip** along the top, so
multiple parallel sessions can be watched at a glance. Click a worktree to jump
to it, `[+]` to create one, or open the switcher modal for the full list/detail
view.

- **Explorer** column is split 50/50: file tree (top) and diff / comment list (bottom).
- **Terminal** column is split 80/20: Claude Code (top) and Shell (bottom).
- Any panel can be maximized (`Ctrl+Alt+Z`) to take the full main area, or resized
  tmux-style with `Ctrl+Alt+Arrow` (the new ratios persist to `config.toml`).

### Keybindings

- **Tab / Shift+Tab** — cycle panel focus (non-terminal panels)
- **Alt+h / Alt+l** — cycle panel focus from anywhere, including terminals
- **Alt+1…6** (or **⌘+1…6**) — focus a specific panel
- **Ctrl+Alt+Z** — maximize / restore (zoom) the focused panel
- **Ctrl+Alt+Arrow** — resize the focused panel tmux-style (ratios persist)
- **Ctrl+Tab / Ctrl+Shift+Tab** (or **Alt+] / Alt+[**) — switch the selected worktree
- **Ctrl+n** — new Claude Code session · **Ctrl+t** — new shell
- **j/k** — navigate up/down · **h/l** — collapse/expand · **g/G** — top/bottom
- **/** — search · **Ctrl+g** — full-text (grep) search
- **?** — show help · **Esc** — back / close overlay · **Ctrl+q** — quit

### Command Palette

**Ctrl+p** (any panel, including terminals) or **:** (non-terminal panels) opens the command palette. All available commands are listed and fuzzy-searchable — worktree operations, terminal management, diff toggles, review comments, etc.

## Reviewing a Pull Request

PR review is built into the normal Explorer/Viewer/Terminal accordion — there's
no separate full-screen mode to enter or exit, so all the usual navigation and
terminal keybindings keep working while you review:

1. **Pull Request → local worktree** — menu: *Review ▸ Review Pull Request…*,
   enter a PR number or URL. Conductor fetches it (via `gh`) into a worktree,
   focuses the Explorer's changed-files list, and offers to run the AI review
   below on it.
2. **Changed files and comments** — the Explorer's bottom pane cycles between
   the changed-files diff list and the review comment list.
3. **Jump into the code** — the diff pane supports the Viewer's `gd`/`gi`/`gr`
   symbol-jump hints (go to definition / implementation / references). **Symbol
   jumps only work for Rust, Go, and TypeScript** — other languages show no
   hints.
4. **AI review** — `W` (menu: *Review ▸ Review current branch*) asks first, then
   runs **revidere** over the worktree's
   diff. It sorts every changed line into sections by importance
   (core / ripple / follow / minor) and checks that **no changed line is left
   unexplained**. `w` then opens the result as a full-screen two-column view:
   the reading order on the left, the diff in that order on the right. `j`/`k`
   scroll, `n`/`N` move between sections, `Enter` opens the section's location
   in the Viewer (where you can leave a comment), `q` closes it.

   revidere lives in this repository (`crates/revidere`) and ships inside the
   `conductor` binary, so there is nothing extra to install. It calls the AI
   through the same `[api]` section as every other AI feature here — but note
   that this one needs `provider = "command"` pointing at an agentic CLI: the
   model is expected to read the repository itself, which a plain HTTP
   completion cannot do. Replies are cached on the diff itself, so re-running on
   an unchanged diff returns instantly — the confirmation offers a re-analysis
   that ignores the cache when a review for this commit already exists, and
   `alt+w` skips both the question and the cache.
5. **Publish comments** — palette: *Review: Publish Comments to GitHub* posts
   your inline review comments (and replies) back to the PR via `gh`.

Requires the `gh` CLI (see Prerequisites) to be installed and authenticated.

> **Breaking change:** diff mode's "jump to top" moved from `g` to `gg` (vim-style),
> since `g` is now a prefix for symbol-jump hints (`gd`/`gi`/`gr`/`gg`).

## MCP Server

The MCP server that exposes the review database to Claude Code sessions running inside the terminal is the `conductor` binary itself, via `conductor mcp-serve` — there's no separate package to build or keep in sync with the TUI's version.

The MCP server is automatically configured when you install the Claude Code plugin (see Installation step 2). **`conductor` must be on `$PATH`** for this to work — the plugin's `.mcp.json` invokes it by name, not by absolute path.

To run it manually (e.g. for development):

```sh
conductor mcp-serve --db /path/to/repo/.conductor/conductor.db
```

## Configuration

Config file: `~/.config/conductor/config.toml`

All fields are optional with sensible defaults. Full example:

```toml
[general]
# repo = "/path/to/default/repo"       # default repository to open on startup
main_branch = "main"                    # main/trunk branch name (default: "main")
# shell = "/bin/zsh"                    # shell for PTY sessions (default: $SHELL)
# repos = ["/path/to/repo1", "/path/to/repo2"]  # additional repos for multi-repo support
# worktree_dir = "~/worktrees"          # custom worktree base directory
                                        #   (default: <repo-parent>/<repo-name>-worktrees/)
decoration = "aquarium"                 # worktree panel decoration
                                        #   aquarium | space | garden | city | none
# auto_resume = true                    # automatically resume Claude Code sessions on startup
# auto_resume_main = false              # also resume the session on the main worktree
                                        #   (grabbed sessions are always resumed regardless)

[terminal]
# inactive_scrollback = 1000            # scrollback lines for background sessions
# active_scrollback = 10000             # scrollback lines for foreground session

[viewer]
theme = "catppuccin-mocha"              # syntax highlighting theme
                                        #   catppuccin-mocha | dracula | nord | solarized-dark
# syntax_theme_file = "~/.config/conductor/custom.tmTheme"  # custom .tmTheme file path
# tab_width = 2                         # spaces per tab stop
# word_wrap = false                     # soft-wrap long lines (未実装)

[diff]
# default_view = "unified"              # unified | side-by-side
# word_diff = true                      # highlight intra-line word changes

[review]
# レビュー機能はMCPプラグイン (conductor plugin) に移行済みです。
# 以下の設定は互換性のため残されていますが、通常は変更不要です。
# prompt_template = "以下のレビューコメントに対応してください。\n\n{comments}"
                                        # template for review prompts ({comments} is replaced)
# prompt_action = "clipboard"           # clipboard | send_to_session

[keybinds]
# Key-bind overrides, in key->action form (powered by keymap-rs). Each entry
# maps a key chord to an action name and LAYERS OVER the built-in defaults
# per-chord. [keybinds.keys] is the global layer; each [keybinds.layers.<name>]
# is a per-panel layer (worktree, explorer, explorer_diff_list,
# explorer_comment_list, viewer, viewer_diff_mode, terminal, editor, overlay).
#
# [keybinds.keys]
# "ctrl+q" = "quit"
#
# [keybinds.layers.worktree]
# "j" = "navigate_down"
# "down" = "navigate_down"
# "w" = "create_worktree"

[ccusage]
# enabled = false                       # token usage display in the title bar (requires ccusage)
# poll_interval_secs = 120              # polling interval in seconds

[updates]
# check_on_startup = true               # check for new versions on startup
# check_interval_secs = 3600            # minimum interval between checks (default: 1h)

[api]
# LLM provider for smart-worktree generation (turn a task description into a
# branch name + initial Claude Code prompt). Each provider stands alone — a
# failure surfaces to the user rather than falling back to another.
# provider = "gemini"                   # "gemini" (Gemini API) | "claude" (claude -p CLI) | "command" (external)
# model = "gemini-2.5-flash"            # model used by the "gemini" provider
# command = ["ollama", "run", "llama3"] # external command for provider = "command"
                                        #   (prompt on stdin, completion on stdout)
# command_timeout_secs = 60             # wall-clock timeout for the command provider (0 = no timeout)
```

## Data Paths

| Path | Description |
|---|---|
| `~/.config/conductor/config.toml` | User configuration |
| `<repo-root>/.conductor/conductor.db` | Per-repo review database (gitignored) |
| `<repo-parent>/<repo-name>-worktrees/` | Default worktree directory |

## License

MIT
