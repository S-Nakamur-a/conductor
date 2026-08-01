//! Config file path resolution, default-file generation, and comment-preserving
//! in-place persistence of individual settings (theme, high-contrast, layout).

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Return the canonical path to the configuration file.
pub fn config_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("conductor")
        .join("config.toml")
}

/// Expand a leading `~` to the user's home directory.
pub(super) fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with('~')
        && let Some(home) = dirs::home_dir()
    {
        return PathBuf::from(s.replacen('~', &home.to_string_lossy(), 1));
    }
    path.to_path_buf()
}

/// Generate a default configuration file with all settings commented out.
pub fn generate_default_config() -> String {
    String::from(
        r#"# Conductor configuration file
# All fields are optional with sensible defaults.
# Appearance settings (theme, viewer, diff, decoration, layout) take effect
# immediately on file save — no restart required. All other settings need a restart.

[general]
# repo = "/path/to/default/repo"       # default repository to open on startup
# main_branch = "main"                  # main/trunk branch name (default: "main")
# shell = "/bin/zsh"                    # shell for PTY sessions (default: $SHELL)
# repos = ["/path/to/repo1", "/path/to/repo2"]  # additional repos for multi-repo support
# worktree_dir = "~/worktrees"          # custom worktree base directory
#                                       #   (default: <repo-parent>/<repo-name>-worktrees/)
# decoration = "aquarium"               # worktree panel decoration
#                                       #   aquarium | space | garden | city | none
# auto_resume = true                    # automatically resume Claude Code sessions on startup
# auto_resume_main = false              # also resume on the main worktree (grabbed sessions
#                                       #   are always resumed regardless of this setting)

[terminal]
# inactive_scrollback = 1000            # scrollback lines for background sessions
# active_scrollback = 10000             # scrollback lines for foreground session

[viewer]
# theme = "catppuccin-mocha"            # syntax highlighting theme
#                                       #   catppuccin-mocha | dracula | nord | solarized-dark
# syntax_theme_file = "~/.config/conductor/custom.tmTheme"  # custom .tmTheme file path
# tab_width = 2                         # spaces per tab stop
# word_wrap = false                     # soft-wrap long lines (未実装)

[diff]
# default_view = "unified"              # unified | side-by-side
# word_diff = true                      # highlight intra-line word changes

[review]
# レビューコメントの受け渡しはMCPプラグイン (conductor plugin) 経由です。
# walkthrough_language = "日本語"        # language the walkthrough is written in
#                                       # (unset = model's choice)
#                                       # which MODEL writes it is [api] below — walkthrough
#                                       # generation goes through the same configurable seam

[keybinds]
# Key-bind overrides, in key→action form. Each entry maps a key chord to an
# action name. Your bindings are MERGED OVER the built-in defaults per-chord: a
# chord you bind here overrides the default for that exact chord, and every
# default you do not touch keeps working. To fully remove a default key (not
# just shadow it), set the chord to `false` — a tombstone, e.g.
# `"ctrl+q" = false`.
#
# [keybinds.keys] is the global layer (active everywhere). Each
# [keybinds.layers.<context>] table is a per-panel layer. Context names:
# worktree, explorer, explorer_diff_list, explorer_comment_list, viewer,
# viewer_diff_mode, terminal, overlay.
#
# Key grammar: modifiers ctrl/alt/shift/super joined with '+', then the key.
# A single char is verbatim and case-sensitive (e.g. "G" is Shift+g). Back-tab
# is "shift+tab". Named keys: enter, esc, tab, space, up/down/left/right, home,
# end, pageup, pagedown, delete, f1..f24.
#
# [keybinds.keys]
# "ctrl+q" = "quit"
# "ctrl+r" = false          # remove a default binding entirely
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

[rich]
# mode = "auto"                         # rich mode (gradient borders, pixel-quality images)
#                                       #   auto  - detect terminal capabilities (default)
#                                       #   off   - plain rendering everywhere
#                                       #   force - enable even when detection fails

[api]
# Which AI answers Conductor's own prompts (smart worktree naming, walkthrough
# generation). Conductor never runs a CLI of its own — you name the tool here.
#
# provider = "gemini"                   # "gemini" (Gemini API) or "command" (any CLI) — no fallback between them
# model = "gemini-2.5-flash"            # model for the "gemini" provider
# command_timeout_secs = 60             # wall-clock timeout (0 = none). Long tasks such as
#                                       #   walkthrough generation set their own instead.
#
# `command` is the AI tool itself, as argv. Two placeholders are substituted:
#   {prompt}   the assembled prompt. Put it where the tool expects its prompt;
#              LEAVE IT OUT for tools that read stdin, which is then used.
#   {workdir}  the directory the task is about (the reviewed worktree). The
#              command also *runs* in that directory, so "." works too.
#
# command = ["claude", "-p", "{prompt}"]                 # Claude Code, one-shot
# command = ["my-cli", "-w", "{workdir}", "{prompt}"]    # prompt as a positional argument
# command = ["ollama", "run", "llama3"]                  # reads stdin, no placeholder needed
#
# You do not need a wrapper script: what a task requires — that the model must
# not use tools, the output format, which directory to read — is part of the
# prompt Conductor builds for that task.

[ui]
# theme = "catppuccin-mocha"            # UI color theme (overrides [viewer] theme when set)
#                                       #   dark:  catppuccin-mocha | dracula | nord | solarized-dark
#                                       #          tokyo-night | gruvbox | rose-pine | kanagawa
#                                       #   light: catppuccin-latte | solarized-light | github-light
#                                       # Light themes work best on terminals with a light/white background.
#                                       # When unset, conductor auto-detects a light background via OSC 11
#                                       # and switches to catppuccin-latte for that session (no file write).
# high_contrast = false                 # boost the active theme's contrast (brighter/deeper text, borders,
#                                       # accents). Works with any theme; toggle live from the command palette.

[layout]
# explorer_width_pct = 24               # explorer column width % (default: 24)
# viewer_width_pct = 38                 # viewer column width % (default: 38)
#                                       # terminal column gets the remaining width
# explorer_split_pct = 50              # file-tree height % within explorer column (default: 50)
#                                       # changed-files list receives the remainder
# terminal_split_pct = 80              # Claude Code area height % within terminal column (default: 80)
#                                       # shell area receives the remainder. These three values are the
#                                       # initial proportions; resize panels live, tmux-style, with
#                                       # Ctrl+Alt+Arrow (grows the focused panel toward the arrow) and
#                                       # conductor writes the new ratios back here automatically.
#                                       # These proportions apply in the default layout only;
#                                       # maximizing a panel (Ctrl+Alt+Z) overrides them temporarily.
"#,
    )
}

/// Persist a theme selection to `~/.config/conductor/config.toml`.
///
/// Uses text-based minimal editing to preserve existing comments and structure:
/// - If the `[ui]` section already has an uncommented `theme = ...` line, it is
///   replaced in place.
/// - If the `[ui]` section exists but has no uncommented `theme` line (e.g. only
///   comments), the line is inserted immediately after the `[ui]` header.
/// - If no `[ui]` section exists, `\n[ui]\ntheme = "..."\n` is appended.
pub fn persist_ui_theme(name: &str) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        // Config file doesn't exist yet; generate defaults and proceed.
        generate_default_config()
    };
    let updated = upsert_ui_theme(&contents, name);
    write_atomic(&path, &updated)?;
    Ok(())
}

/// Write `contents` to `path` atomically: write to a sibling temp file, fsync,
/// then rename over the target. A crash, kill, or full disk mid-write can no
/// longer leave the user's hand-edited config truncated or half-written —
/// `std::fs::write` truncates in place, so the old direct writes could destroy
/// the whole file on a mistimed failure.
pub(super) fn write_atomic(path: &std::path::Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("toml.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Return `true` when `line` is a bare `[section]` TOML header, possibly
/// followed by whitespace and/or an inline comment (e.g. `[ui]  # colors`).
/// Sub-sections like `[ui.fonts]` are deliberately excluded.
pub(super) fn is_section_header(line: &str, section: &str) -> bool {
    let trimmed = line.trim();
    let bracket = format!("[{section}]");
    if !trimmed.starts_with(&bracket) {
        return false;
    }
    // After `[section]` only whitespace and/or a `#` comment may follow.
    let after = trimmed[bracket.len()..].trim_start();
    after.is_empty() || after.starts_with('#')
}

/// Pure function: upsert `<key> = <value>` in the `[section]` table of a config
/// file string, preserving all comments and surrounding content.
///
/// - If the section has an uncommented `key = ...` line, it is replaced in place.
/// - If the section exists but has no uncommented `key` line (e.g. only the
///   commented default), the line is inserted right after the section header.
/// - If the section does not exist, `\n[section]\n<key> = <value>\n` is appended.
///
/// `value` must already be formatted as valid TOML (quote strings yourself).
/// Extracted as a testable helper so file I/O stays separable from the edit.
pub(super) fn upsert_section_kv(contents: &str, section: &str, key: &str, value: &str) -> String {
    let kv_line = format!("{key} = {value}");
    let lines: Vec<&str> = contents.lines().collect();

    let sec_start = lines.iter().position(|l| is_section_header(l, section));

    if let Some(sec_idx) = sec_start {
        // End of section = next bare `[...]` header (not a comment).
        let sec_end = lines[sec_idx + 1..]
            .iter()
            .position(|l| {
                let t = l.trim();
                t.starts_with('[') && !t.starts_with('#')
            })
            .map(|i| sec_idx + 1 + i)
            .unwrap_or(lines.len());

        // Look for an existing uncommented `key = ...` line within the section.
        let key_eq = format!("{key} =");
        let key_eq_tight = format!("{key}=");
        let existing_idx = lines[sec_idx + 1..sec_end]
            .iter()
            .position(|l| {
                let t = l.trim();
                !t.starts_with('#') && (t.starts_with(&key_eq) || t.starts_with(&key_eq_tight))
            })
            .map(|i| sec_idx + 1 + i);

        let mut result_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        if let Some(idx) = existing_idx {
            result_lines[idx] = kv_line;
        } else {
            // No uncommented key line in the section — insert right after the header.
            result_lines.insert(sec_idx + 1, kv_line);
        }

        let mut result = result_lines.join("\n");
        if contents.ends_with('\n') {
            result.push('\n');
        }
        result
    } else {
        // No such section at all — append one.
        let mut result = contents.to_string();
        if !result.ends_with('\n') && !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("\n[{section}]\n{kv_line}\n"));
        result
    }
}

/// Pure function: upsert `theme = "<name>"` in the `[ui]` section. Thin wrapper
/// over [`upsert_section_kv`] kept for call-site clarity and its dedicated tests.
pub(super) fn upsert_ui_theme(contents: &str, name: &str) -> String {
    upsert_section_kv(contents, "ui", "theme", &format!("\"{name}\""))
}

/// Persist the high-contrast toggle to the `[ui]` section of the config file,
/// preserving comments and structure. Mirrors [`persist_ui_theme`]; called by
/// the in-app "Toggle High Contrast" command so the choice survives restarts.
pub fn persist_ui_high_contrast(enabled: bool) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        generate_default_config()
    };
    let updated = upsert_section_kv(&contents, "ui", "high_contrast", &enabled.to_string());
    write_atomic(&path, &updated)?;
    Ok(())
}

/// Persist the runtime panel proportions to the `[layout]` section of
/// `~/.config/conductor/config.toml`, preserving comments and structure.
///
/// Called after a tmux-style pane resize so the chosen ratios survive restarts.
/// The three values are the explorer/viewer column width percentages and the
/// Claude-area height percentage within the terminal column. Writing this file
/// trips the config watcher, but `reload_appearance_config` no-ops because the
/// running config already holds these values (the appearance snapshot matches).
pub fn persist_layout_proportions(
    explorer_width_pct: u16,
    viewer_width_pct: u16,
    terminal_split_pct: u16,
    explorer_split_pct: u16,
) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        generate_default_config()
    };
    let updated = upsert_section_kv(
        &contents,
        "layout",
        "explorer_width_pct",
        &explorer_width_pct.to_string(),
    );
    let updated = upsert_section_kv(&updated, "layout", "viewer_width_pct", &viewer_width_pct.to_string());
    let updated = upsert_section_kv(
        &updated,
        "layout",
        "terminal_split_pct",
        &terminal_split_pct.to_string(),
    );
    let updated = upsert_section_kv(
        &updated,
        "layout",
        "explorer_split_pct",
        &explorer_split_pct.to_string(),
    );
    write_atomic(&path, &updated)?;
    Ok(())
}
