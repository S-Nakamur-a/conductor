# Configuration

Config file: `~/.config/conductor/config.toml`. Conductor writes this file on
first run, with every key commented out at its default.

All fields are optional with sensible defaults, so an absent file is a valid one.
Appearance settings (`[ui]`, `[viewer]`, `[diff]`, `[layout]`) take effect
immediately on save; everything else needs a restart.

```toml
[general]
# repo = "/path/to/default/repo"       # default repository to open on startup
# main_branch = "main"                  # main/trunk branch name (default: "main")
# shell = "/bin/zsh"                    # shell for PTY sessions (default: $SHELL)
# repos = ["/path/to/repo1", "/path/to/repo2"]  # additional repos for multi-repo support
# worktree_dir = "~/worktrees"          # custom worktree base directory
                                        #   (default: <repo-parent>/<repo-name>-worktrees/)
# auto_resume = true                    # automatically resume Claude Code sessions on startup
# auto_resume_main = false              # also resume the session on the main worktree
                                        #   (grabbed sessions are always resumed regardless)

[terminal]
# inactive_scrollback = 1000            # scrollback lines for background sessions
# active_scrollback = 10000             # scrollback lines for foreground session

[viewer]
# theme = "catppuccin-mocha"            # syntax highlighting theme; [ui] theme overrides it
# syntax_theme_file = "~/.config/conductor/custom.tmTheme"  # custom .tmTheme file path
# tab_width = 2                         # spaces per tab stop

[diff]
# default_view = "unified"              # unified | side-by-side
# word_diff = true                      # highlight intra-line word changes

[ui]
# theme = "catppuccin-mocha"            # UI color theme (overrides [viewer] theme when set)
                                        #   dark:  catppuccin-mocha | dracula | nord | solarized-dark
                                        #          tokyo-night | gruvbox | rose-pine | kanagawa
                                        #   light: catppuccin-latte | solarized-light | github-light
                                        # When unset, conductor probes the terminal background via
                                        # OSC 11 and switches to a light theme for that session only.
# high_contrast = false                 # boost the active theme's contrast; toggle from the palette
# icons = "unicode"                     # glyph set for file icons: nerd | unicode
                                        # "nerd" needs a Nerd Font. Terminals cannot report which
                                        # font is in use, so on first run conductor guesses from
                                        # $TERM_PROGRAM and writes the result here. An explicit
                                        # value always wins.
# startup_animation = true              # panels assemble themselves on the first frames.
                                        # Turn it off over SSH or on slow-drawing terminals.

[layout]
# explorer_width_pct = 24               # explorer column width % (terminal gets the remainder)
# viewer_width_pct = 38                 # viewer column width %
# explorer_split_pct = 50               # file-tree height % within the explorer column
# terminal_split_pct = 80               # Claude Code height % within the terminal column
                                        # These are the initial proportions. Resizing panels
                                        # tmux-style with Ctrl+Alt+Arrow writes the new ratios
                                        # back here automatically. Maximizing a panel
                                        # (Ctrl+Alt+Z) overrides them temporarily.

[updates]
# check_on_startup = true               # look for a newer release on GitHub at startup
# check_interval_secs = 3600            # minimum interval between checks; also the cache lifetime

[keybinds]
# Key-bind overrides, in key->action form. Each entry maps a key chord to an
# action name and is MERGED OVER the built-in defaults per-chord: a chord you
# bind here overrides that exact default, and every default you do not touch
# keeps working. To remove a default outright rather than shadow it, set the
# chord to `false` — a tombstone.
#
# [keybinds.keys] is the global layer. Each [keybinds.layers.<context>] table is
# a per-panel layer; the context names are listed below.
#
# Key grammar: modifiers ctrl/alt/shift/super joined with '+', then the key. A
# single char is verbatim and case-sensitive ("G" is Shift+g). Back-tab is
# "shift+tab". Named keys: enter, esc, tab, space, up/down/left/right, home,
# end, pageup, pagedown, delete, f1..f24.
#
# [keybinds.keys]
# "ctrl+q" = "quit"
# "ctrl+r" = false          # remove a default binding entirely
#
# [keybinds.layers.worktree]
# "j" = "navigate_down"
# "w" = "create_worktree"

[api]
# Which AI answers Conductor's own prompts (smart worktree naming, the revidere
# review). Conductor never runs a CLI of its own — you name the tool here. The
# two providers stand alone: a failure surfaces rather than falling back.
#
# provider = "gemini"                   # "gemini" (Gemini API) | "command" (any CLI)
# model = "gemini-2.5-flash"            # model for the "gemini" provider
# command_timeout_secs = 60             # wall-clock timeout (0 = none)
#
# `command` is the AI tool itself, as argv — no shell. Two placeholders are
# substituted:
#   {prompt}   the assembled prompt. Leave it out for tools that read stdin,
#              which is then used.
#   {workdir}  the directory the task is about. The command also runs there.
#
# command = ["claude", "-p", "{prompt}"]                 # Claude Code, one-shot
# command = ["my-cli", "-w", "{workdir}", "{prompt}"]    # prompt as a positional argument
# command = ["ollama", "run", "llama3"]                  # reads stdin, no placeholder needed
#
# You do not need a wrapper script: what a task requires — that the model must
# not use tools, the output format, which directory to read — is part of the
# prompt Conductor builds for that task.
```

## Key-bind action names

`[keybinds]` entries map a key chord to an *action name*. The authoritative list
of action names is `crates/conductor-core/src/keymap/action.rs`, and the built-in
bindings they are layered over are in `default_keybinds.toml` beside it.

Accepted `[keybinds.layers.<context>]` names: `worktree`, `explorer`,
`explorer_diff_list`, `explorer_comment_list`, `viewer`, `viewer_diff_mode`,
`terminal`, `editor`, `revidere`, `overlay`.
