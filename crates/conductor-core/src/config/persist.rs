//! 設定ファイルの場所、既定ファイルの生成、コメントを保ったままの個別鍵の書き戻し。

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::LayoutConfig;
use crate::icons::IconSet;

pub fn config_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("conductor")
        .join("config.toml")
}

/// 初回起動時に書き出すファイル。全鍵をコメントアウトしてあるので読むと既定値になる。
pub const DEFAULT_CONFIG: &str = r#"# Conductor configuration file
# All fields are optional with sensible defaults.
# Appearance settings (ui, viewer, diff, layout) take effect
# immediately on file save — no restart required. All other settings need a restart.

[general]
# repo = "/path/to/default/repo"       # default repository to open on startup
# main_branch = "main"                  # main/trunk branch name (default: "main")
# shell = "/bin/zsh"                    # shell for PTY sessions (default: $SHELL)
# repos = ["/path/to/repo1", "/path/to/repo2"]  # additional repos for multi-repo support
# worktree_dir = "~/worktrees"          # custom worktree base directory
#                                       #   (default: <repo-parent>/<repo-name>-worktrees/)
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

[diff]
# default_view = "unified"              # unified | side-by-side
# word_diff = true                      # highlight intra-line word changes

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

[api]
# Which AI answers Conductor's own prompts (smart worktree naming).
# Conductor never runs a CLI of its own — you name the tool here.
#
# provider = "gemini"                   # "gemini" (Gemini API) or "command" (any CLI) — no fallback between them
# model = "gemini-2.5-flash"            # model for the "gemini" provider
# command_timeout_secs = 60             # wall-clock timeout (0 = none). A task that knows
#                                       #   it runs long sets its own instead.
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
# icons = "unicode"                     # glyph set for file icons: nerd | unicode
#                                       # "nerd" needs a Nerd Font (or a terminal that bundles the symbols —
#                                       # Ghostty and WezTerm do). Terminals cannot report which font is in
#                                       # use, so on first run conductor picks a set from $TERM_PROGRAM and
#                                       # writes the result here. Edit it freely; an explicit value always wins.

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
"#;

pub(super) fn write_default(path: &Path) -> Result<()> {
    write_atomic(path, DEFAULT_CONFIG)
}

/// テーマピッカーの選択を [ui] theme に書く。
pub fn persist_ui_theme(name: &str) -> Result<()> {
    persist_at(&config_file_path(), "ui", &[("theme", toml_string(name))])
}

pub fn persist_ui_high_contrast(enabled: bool) -> Result<()> {
    persist_at(
        &config_file_path(),
        "ui",
        &[("high_contrast", enabled.to_string())],
    )
}

/// 自動判定したアイコンの文字セットを [ui] icons に書く。
///
/// テーマの自動判定 (セッション限り) と違って書き戻すのは、判定材料が TERM_PROGRAM しか
/// なく、tmux 越しや起動経路で結果が変わりうるため。一度書けば以降は環境に左右されず、
/// ユーザが直せばそれが優先される。
pub fn persist_ui_icons(set: IconSet) -> Result<()> {
    let value = toml::Value::try_from(set)?.to_string();
    persist_at(&config_file_path(), "ui", &[("icons", value)])
}

/// リサイズ後のパネル比率を [layout] に書く。この書き込みで config watcher が起きるが、
/// 実行中の config は同じ値を既に持つので外観の再適用は no-op になる。
pub fn persist_layout_proportions(layout: &LayoutConfig) -> Result<()> {
    persist_at(
        &config_file_path(),
        "layout",
        &[
            ("explorer_width_pct", layout.explorer_width_pct.to_string()),
            ("viewer_width_pct", layout.viewer_width_pct.to_string()),
            ("terminal_split_pct", layout.terminal_split_pct.to_string()),
            ("explorer_split_pct", layout.explorer_split_pct.to_string()),
        ],
    )
}

fn toml_string(s: &str) -> String {
    toml::Value::from(s).to_string()
}

/// path の [section] に鍵を upsert する。ファイルが無ければ既定のファイルに対して行う。
pub(super) fn persist_at(path: &Path, section: &str, kvs: &[(&str, String)]) -> Result<()> {
    let mut contents = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::from(DEFAULT_CONFIG)
    };
    for (key, value) in kvs {
        contents = upsert_section_kv(&contents, section, key, value);
    }
    write_atomic(path, &contents)
}

/// 隣に一時ファイルを書いて fsync してから rename する。std::fs::write はその場で
/// 切り詰めるので、書き込み中に落ちるとユーザが手で編集したファイルが壊れる。
pub(super) fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
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

/// [section] の行か。後ろの空白や行末コメントは許し、[section.sub] は別物として弾く。
pub(super) fn is_section_header(line: &str, section: &str) -> bool {
    let bracket = format!("[{section}]");
    match line.trim().strip_prefix(&bracket) {
        Some(after) => {
            let after = after.trim_start();
            after.is_empty() || after.starts_with('#')
        }
        None => false,
    }
}

/// [section] の中の `key = value` 行を置き換える。無ければヘッダの直後に挿す (コメント
/// アウトされた既定値はそのまま残る)。セクション自体が無ければ末尾に追記する。
pub(super) fn upsert_section_kv(contents: &str, section: &str, key: &str, value: &str) -> String {
    let kv_line = format!("{key} = {value}");
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();

    let Some(header) = lines.iter().position(|l| is_section_header(l, section)) else {
        let mut result = contents.to_string();
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("\n[{section}]\n{kv_line}\n"));
        return result;
    };

    let body = header + 1;
    let end = lines[body..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| body + i);
    let existing = lines[body..end]
        .iter()
        .position(|l| is_assignment_of(l, key))
        .map(|i| body + i);

    match existing {
        Some(idx) => lines[idx] = kv_line,
        None => lines.insert(body, kv_line),
    }

    let mut result = lines.join("\n");
    if contents.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn is_assignment_of(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with('#')
        && t.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
}
