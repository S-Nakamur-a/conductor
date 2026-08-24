# Configuration

Config file: `~/.config/conductor/config.toml`

All fields are optional with sensible defaults. This is the full set, with
every optional field commented out at its default.

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

## Key-bind action names

`[keybinds]` entries map a key chord to an *action name*. The authoritative list
of action names is `src/keymap/action.rs`; the layer names accepted under
`[keybinds.layers.<name>]` are listed in the `[keybinds]` comment above.
