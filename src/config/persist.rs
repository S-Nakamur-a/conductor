//! 設定ファイルのパス解決、デフォルトファイルの生成、および個別設定
//! (テーマ、high-contrast、layout)のコメントを保持したままの in-place 永続化。

use std::path::{Path, PathBuf};

use anyhow::Result;

/// 設定ファイルの正規パスを返す。
pub fn config_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("conductor")
        .join("config.toml")
}

/// 先頭の ~ をユーザのホームディレクトリに展開する。
pub(super) fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with('~')
        && let Some(home) = dirs::home_dir()
    {
        return PathBuf::from(s.replacen('~', &home.to_string_lossy(), 1));
    }
    path.to_path_buf()
}

/// すべての設定をコメントアウトした状態のデフォルト設定ファイルを生成する。
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
# 差分の解析 (どの AI が、どの言語で書くか) は revidere 側の設定です:
#   <repo>/.revidere/config.toml → ~/.config/revidere/config.toml

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
"#,
    )
}

/// テーマ選択を ~/.config/conductor/config.toml に永続化する。
///
/// 既存のコメントと構造を保つため、テキストベースの最小限の編集を行う:
/// - [ui] セクションに既にコメントアウトされていない theme = ... 行が
///   あれば、その場で置き換える。
/// - [ui] セクションはあるがコメントアウトされていない theme 行がない
///   場合(コメントのみなど)は、[ui] ヘッダの直後に行を挿入する。
/// - [ui] セクション自体が存在しない場合は \n[ui]\ntheme = "..."\n を追記する。
pub fn persist_ui_theme(name: &str) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        // 設定ファイルがまだ存在しない場合はデフォルトを生成して進める。
        generate_default_config()
    };
    let updated = upsert_ui_theme(&contents, name);
    write_atomic(&path, &updated)?;
    Ok(())
}

/// contents を path へアトミックに書き込む: 隣に一時ファイルを作って
/// 書き込み、fsync してから対象へ rename する。クラッシュ・kill・書き込み中の
/// ディスクフルが起きても、ユーザが手編集した設定ファイルが途中で切れたり
/// 壊れたりしなくなる — std::fs::write はその場で切り詰めるため、旧来の
/// 直接書き込みはタイミングによっては失敗時にファイル全体を破壊しかねなかった。
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

/// line が素の [section] TOML ヘッダ(後ろに空白やインラインコメントが
/// 続く場合を含む、例: [ui]  # colors)であれば true を返す。
/// [ui.fonts] のようなサブセクションは意図的に除外する。
pub(super) fn is_section_header(line: &str, section: &str) -> bool {
    let trimmed = line.trim();
    let bracket = format!("[{section}]");
    if !trimmed.starts_with(&bracket) {
        return false;
    }
    // [section] の後には空白か # コメントしか続かないはず。
    let after = trimmed[bracket.len()..].trim_start();
    after.is_empty() || after.starts_with('#')
}

/// 純粋関数: 設定ファイル文字列の [section] テーブル内で <key> = <value>
/// を upsert する。すべてのコメントと周辺の内容は保持する。
///
/// - セクションにコメントアウトされていない key = ... 行があれば、その場で
///   置き換える。
/// - セクションはあるがコメントアウトされていない key 行がない場合
///   (コメントアウトされたデフォルトのみなど)は、セクションヘッダの直後に
///   行を挿入する。
/// - セクションが存在しない場合は \n[section]\n<key> = <value>\n を追記する。
///
/// value はあらかじめ有効な TOML として整形しておくこと(文字列は自分で
/// クォートする)。ファイル I/O と編集ロジックを分離できるよう、テスト可能な
/// ヘルパーとして切り出している。
pub(super) fn upsert_section_kv(contents: &str, section: &str, key: &str, value: &str) -> String {
    let kv_line = format!("{key} = {value}");
    let lines: Vec<&str> = contents.lines().collect();

    let sec_start = lines.iter().position(|l| is_section_header(l, section));

    if let Some(sec_idx) = sec_start {
        // セクションの終わりは次の素の [...] ヘッダ(コメントでないもの)。
        let sec_end = lines[sec_idx + 1..]
            .iter()
            .position(|l| {
                let t = l.trim();
                t.starts_with('[') && !t.starts_with('#')
            })
            .map(|i| sec_idx + 1 + i)
            .unwrap_or(lines.len());

        // セクション内にコメントアウトされていない key = ... 行がないか探す。
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
            // セクション内にコメントアウトされていない key 行がない — ヘッダの直後に挿入する。
            result_lines.insert(sec_idx + 1, kv_line);
        }

        let mut result = result_lines.join("\n");
        if contents.ends_with('\n') {
            result.push('\n');
        }
        result
    } else {
        // そのセクション自体が存在しない — 新しく追記する。
        let mut result = contents.to_string();
        if !result.ends_with('\n') && !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("\n[{section}]\n{kv_line}\n"));
        result
    }
}

/// 純粋関数: [ui] セクションで theme = "<name>" を upsert する。呼び出し
/// 側の分かりやすさと専用テストのために [upsert_section_kv] を薄くラップ
/// したもの。
pub(super) fn upsert_ui_theme(contents: &str, name: &str) -> String {
    upsert_section_kv(contents, "ui", "theme", &format!("\"{name}\""))
}

/// high-contrast のトグルを、コメントと構造を保ったまま設定ファイルの
/// [ui] セクションに永続化する。[persist_ui_theme] と対をなす関数で、
/// 選択が再起動後も残るようアプリ内の "Toggle High Contrast" コマンドから
/// 呼ばれる。
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

/// 自動判定したアイコンの文字セットを [ui] セクションに書き込む。
///
/// テーマの自動判定 (こちらはセッション限りで永続化しない) と違って書き戻すのは、
/// 判定材料が TERM_PROGRAM しかないためである。tmux 越しや未知の端末では判定が
/// 効かず、同じ端末でも起動経路によって結果が変わりうる。一度ファイルに書いて
/// しまえば以降は環境に左右されず、ユーザが直せば恒久的にそれが優先される。
pub fn persist_ui_icons(set: crate::icons::IconSet) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        generate_default_config()
    };
    let value = match set {
        crate::icons::IconSet::Nerd => "\"nerd\"",
        crate::icons::IconSet::Unicode => "\"unicode\"",
    };
    let updated = upsert_section_kv(&contents, "ui", "icons", value);
    write_atomic(&path, &updated)?;
    Ok(())
}

/// 実行時のパネル比率を、コメントと構造を保ったまま
/// ~/.config/conductor/config.toml の [layout] セクションに永続化する。
///
/// tmux 式のペインリサイズの後に呼ばれ、選んだ比率が再起動後も残るようにする。
/// 3つの値は explorer/viewer カラムの幅パーセントと、terminal カラム内での
/// Claude エリアの高さパーセント。このファイルへの書き込みは config
/// watcher を発火させるが、実行中の config は既にこれらの値を持っている
/// (appearance snapshot が一致する)ため reload_appearance_config は
/// no-op になる。
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
    let updated = upsert_section_kv(
        &updated,
        "layout",
        "viewer_width_pct",
        &viewer_width_pct.to_string(),
    );
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
