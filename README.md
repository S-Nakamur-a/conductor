# Conductor

Terminal-based Git workspace and code review TUI written in Rust. Manages multiple git worktrees, launches Claude Code sessions via embedded PTYs, reviews diffs, and provides structured inline review comments — designed for an AI-assisted development workflow.

## Prerequisites

### Required

| Dependency | Version | Notes |
|---|---|---|
| **Rust toolchain** | 1.85+ | Edition 2024. Install via [rustup](https://rustup.rs/) |
| **Git** | 2.x | Used for worktree operations (`git worktree add`, `git fetch`, etc.) |
| **Claude Code** | latest | `claude` CLI must be in `$PATH`. Install via `npm install -g @anthropic-ai/claude-code` |
| **Node.js + npm** | 20+ | Required for Claude Code installation and MCP server build |

### Optional

| Dependency | Purpose | How to enable |
|---|---|---|
| **ccusage** (via npx) | Token usage / cost display in title bar | Set env var `CONDUCTOR_CCUSAGE=1` |
| **terminal-notifier** | macOS notifications when Claude Code is waiting for input | `brew install terminal-notifier` + set `notification.cc_waiting = true` in config |

## Installation

### 1. Install the binary

```sh
git clone https://github.com/S-Nakamur-a/conductor.git
cd conductor
make install
```

`make install` installs the `conductor` binary to `~/.cargo/bin/` (`cargo install --path .`) and installs MCP server dependencies (`npm install`).

### 2. Install the Claude Code plugin

In a Claude Code session, run:

```
/plugin marketplace add S-Nakamur-a/conductor
/plugin install conductor@conductor-marketplace
```

This sets up:
- **MCP server** — review comment DB integration
- **Hooks** — waiting-state detection for Claude Code sessions
- **Commands** — `/address-conductor-comment` for resolving review comments

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
Worktree | Explorer | Viewer | Terminal (Claude Code / Shell)
```

### Keybindings

- **Tab** — cycle focus between panels
- **j/k** — navigate up/down
- **h/l** — collapse/expand
- **g/G** — jump to top/bottom
- **/** — search
- **?** — show help
- **Esc** — back / close overlay

### Command Palette

**Alt+;** (any panel, including terminal) or **:** (Worktree panel) to open the command palette. All available commands are listed and fuzzy-searchable — worktree operations, terminal management, diff toggles, review comments, etc.

## MCP Server

Conductor includes an MCP server (`plugins/conductor/mcp/conductor-comment/`) that exposes the review database to Claude Code sessions running inside the terminal. This enables Claude Code to read and write review comments directly.

The MCP server is automatically configured when you install the Claude Code plugin (see Installation step 2).

For development:

```sh
cd plugins/conductor/mcp/conductor-comment
npm install
npm run build  # compile TypeScript
npm start      # run compiled JS
# or: npm run dev (runs via tsx, no build step needed)
```

## Configuration

Config file: `~/.config/conductor/config.toml`

All fields are optional with sensible defaults. Example:

```toml
[general]
main_branch = "main"
# shell = "/bin/zsh"            # defaults to $SHELL
# worktree_dir = "~/worktrees"  # custom worktree base directory

[viewer]
theme = "catppuccin-mocha"     # catppuccin-mocha | dracula | nord | solarized-dark

[diff]
# default_view = "unified"     # unified | side-by-side
# word_diff = true

[review]
# prompt_action = "clipboard"  # clipboard | send_to_session

[notification]
# cc_waiting = false
```

## Data Paths

| Path | Description |
|---|---|
| `~/.config/conductor/config.toml` | User configuration |
| `<repo-root>/.conductor/conductor.db` | Per-repo review database (gitignored) |
| `<repo-parent>/<repo-name>-worktrees/` | Default worktree directory |

## License

MIT
