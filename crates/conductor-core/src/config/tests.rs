use std::path::{Path, PathBuf};

use super::persist::{is_section_header, persist_at, upsert_section_kv};
use super::*;
use crate::diff_state::DiffView;
use crate::icons::IconSet;

/// 過去の版が生成していた config.toml に残っている、もう読まないセクションと鍵。
const LEGACY_CONFIG: &str = r#"
[general]
main_branch = "develop"

[viewer]
word_wrap = true

[review]
prompt_template = "…{comments}"
prompt_action = "send_to_session"
walkthrough_language = "日本語"

[rich]
mode = "force"

[ccusage]
enabled = true
poll_interval_secs = 60
"#;

fn parse(toml: &str) -> Config {
    toml::from_str(toml).unwrap()
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap()
}

fn non_default() -> Config {
    Config {
        general: GeneralConfig {
            repo: Some(PathBuf::from("/r")),
            main_branch: String::from("master"),
            shell: String::from("/bin/fish"),
            repos: vec![PathBuf::from("/a")],
            worktree_dir: Some(PathBuf::from("/wt")),
            auto_resume: false,
            auto_resume_main: true,
        },
        terminal: TerminalConfig {
            inactive_scrollback: 1,
            active_scrollback: 2,
        },
        viewer: ViewerConfig {
            theme: String::from("nord"),
            syntax_theme_file: Some(String::from("/t.tmTheme")),
            tab_width: 4,
        },
        diff: DiffConfig {
            default_view: DiffView::SideBySide,
            word_diff: false,
        },
        keybinds: toml::from_str("[keys]\n\"ctrl+q\" = \"quit\"\n").unwrap(),
        api: ApiConfig {
            provider: String::from("command"),
            model: String::from("m"),
            command: vec![String::from("claude")],
            command_timeout_secs: 0,
        },
        ui: UiConfig {
            theme: Some(String::from("dracula")),
            high_contrast: true,
            icons: Some(IconSet::Nerd),
            startup_animation: false,
        },
        layout: LayoutConfig {
            explorer_width_pct: 30,
            viewer_width_pct: 40,
            terminal_split_pct: 70,
            explorer_split_pct: 60,
        },
        updates: UpdatesConfig {
            check_on_startup: false,
            check_interval_secs: 60,
        },
    }
}

#[test]
fn tomlを往復して一致する() {
    for cfg in [Config::default(), non_default()] {
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(parse(&serialized), cfg, "{serialized}");
    }
}

#[test]
fn 空のtomlは既定値になる() {
    assert_eq!(parse(""), Config::default());
}

#[test]
fn 既定ファイルは読むと既定値になる() {
    assert_eq!(parse(DEFAULT_CONFIG), Config::default());
}

#[test]
fn セクションごとに鍵を読む() {
    let d = Config::default();
    let cases = [
        (
            "[general]\nmain_branch = \"develop\"",
            Config {
                general: GeneralConfig {
                    main_branch: String::from("develop"),
                    ..d.general.clone()
                },
                ..d.clone()
            },
        ),
        (
            "[terminal]\nactive_scrollback = 5",
            Config {
                terminal: TerminalConfig {
                    active_scrollback: 5,
                    ..d.terminal.clone()
                },
                ..d.clone()
            },
        ),
        (
            "[viewer]\ntab_width = 8",
            Config {
                viewer: ViewerConfig {
                    tab_width: 8,
                    ..d.viewer.clone()
                },
                ..d.clone()
            },
        ),
        (
            "[diff]\ndefault_view = \"side-by-side\"",
            Config {
                diff: DiffConfig {
                    default_view: DiffView::SideBySide,
                    ..d.diff.clone()
                },
                ..d.clone()
            },
        ),
        (
            "[api]\ncommand = [\"claude\", \"-p\", \"{prompt}\"]",
            Config {
                api: ApiConfig {
                    command: vec!["claude".into(), "-p".into(), "{prompt}".into()],
                    ..d.api.clone()
                },
                ..d.clone()
            },
        ),
        (
            "[ui]\ntheme = \"catppuccin-latte\"\nhigh_contrast = true\nicons = \"nerd\"\nstartup_animation = false",
            Config {
                ui: UiConfig {
                    theme: Some(String::from("catppuccin-latte")),
                    high_contrast: true,
                    icons: Some(IconSet::Nerd),
                    startup_animation: false,
                },
                ..d.clone()
            },
        ),
        (
            "[updates]\ncheck_on_startup = false\ncheck_interval_secs = 60",
            Config {
                updates: UpdatesConfig {
                    check_on_startup: false,
                    check_interval_secs: 60,
                },
                ..d.clone()
            },
        ),
        (
            "[layout]\nexplorer_width_pct = 30\nviewer_width_pct = 40\nterminal_split_pct = 75",
            Config {
                layout: LayoutConfig {
                    explorer_width_pct: 30,
                    viewer_width_pct: 40,
                    terminal_split_pct: 75,
                    ..d.layout.clone()
                },
                ..d.clone()
            },
        ),
    ];
    for (toml, expected) in cases {
        assert_eq!(parse(toml), expected, "{toml}");
    }
}

#[test]
fn keybindsは生のテーブルのまま持つ() {
    let cfg = parse(
        r#"
[keybinds.keys]
"ctrl+q" = "quit"

[keybinds.layers.worktree]
"j" = "navigate_down"
"#,
    );
    let keys = cfg.keybinds["keys"].as_table().unwrap();
    assert_eq!(keys["ctrl+q"].as_str(), Some("quit"));
    let worktree = cfg.keybinds["layers"]["worktree"].as_table().unwrap();
    assert_eq!(worktree["j"].as_str(), Some("navigate_down"));
}

#[test]
fn 捨てた設定と知らない鍵は無視される() {
    let expected = Config {
        general: GeneralConfig {
            main_branch: String::from("develop"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(parse(LEGACY_CONFIG), expected);
}

#[test]
fn theme_nameはui_themeを優先する() {
    let cases = [
        (Some("github-light"), "catppuccin-mocha", "github-light"),
        (None, "github-light", "github-light"),
    ];
    for (ui, viewer, expected) in cases {
        let cfg = Config {
            ui: UiConfig {
                theme: ui.map(String::from),
                ..Default::default()
            },
            viewer: ViewerConfig {
                theme: String::from(viewer),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.theme_name(), expected);
    }
}

#[test]
fn チルダの展開() {
    let cases = [
        ("~/dev/project", home().join("dev/project")),
        ("/abs/path", PathBuf::from("/abs/path")),
        ("relative", PathBuf::from("relative")),
    ];
    for (input, expected) in cases {
        assert_eq!(expand_tilde(Path::new(input)), expected, "{input}");
    }
}

#[test]
fn 無ければ既定ファイルを書いて既定値を返す() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conductor").join("config.toml");
    assert_eq!(Config::load_from(&path).unwrap(), Config::default());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
}

#[test]
fn 読み込み時にパスのチルダを展開する() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[general]\nrepo = \"~/r\"\nrepos = [\"~/a\"]\nworktree_dir = \"~/w\"\n\
         [viewer]\nsyntax_theme_file = \"~/t.tmTheme\"\n",
    )
    .unwrap();
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.general.repo, Some(home().join("r")));
    assert_eq!(cfg.general.repos, vec![home().join("a")]);
    assert_eq!(cfg.general.worktree_dir, Some(home().join("w")));
    assert_eq!(
        cfg.viewer.syntax_theme_file.as_deref(),
        Some(home().join("t.tmTheme").to_str().unwrap())
    );
}

#[test]
fn セクションヘッダの判定() {
    let cases = [
        ("[ui]", "ui", true),
        ("[ui]  ", "ui", true),
        ("[ui]  # comment", "ui", true),
        ("  [ui]", "ui", true),
        ("[ui.sub]", "ui", false),
        ("[viewer]", "ui", false),
        ("[layout]", "layout", true),
        ("[layout]", "ui", false),
    ];
    for (line, section, expected) in cases {
        assert_eq!(is_section_header(line, section), expected, "{line:?}");
    }
}

#[test]
fn セクション内の鍵のupsert() {
    let cases = [
        (
            "既存の行を置き換える",
            "[layout]\nexplorer_width_pct = 24\nviewer_width_pct = 38\n",
            ("layout", "viewer_width_pct", "42"),
            "[layout]\nexplorer_width_pct = 24\nviewer_width_pct = 42\n",
        ),
        (
            "空白なしの代入も置き換える",
            "[ui]\ntheme=\"dracula\"\n",
            ("ui", "theme", "\"nord\""),
            "[ui]\ntheme = \"nord\"\n",
        ),
        (
            "コメントアウトされた既定の上に挿す",
            "[layout]\n# explorer_width_pct = 24    # default\n",
            ("layout", "explorer_width_pct", "30"),
            "[layout]\nexplorer_width_pct = 30\n# explorer_width_pct = 24    # default\n",
        ),
        (
            "セクションが無ければ末尾に追記する",
            "[general]\nmain_branch = \"main\"\n",
            ("ui", "theme", "\"nord\""),
            "[general]\nmain_branch = \"main\"\n\n[ui]\ntheme = \"nord\"\n",
        ),
        (
            "改行で終わらないファイルにも追記できる",
            "[general]\nmain_branch = \"main\"",
            ("ui", "theme", "\"nord\""),
            "[general]\nmain_branch = \"main\"\n\n[ui]\ntheme = \"nord\"\n",
        ),
        (
            "末尾に改行が無ければ足さない",
            "[ui]\ntheme = \"dracula\"",
            ("ui", "theme", "\"nord\""),
            "[ui]\ntheme = \"nord\"",
        ),
        (
            "行末コメント付きのヘッダも同じセクション",
            "[general]\n\n[ui]  # color settings\ntheme = \"dracula\"\n",
            ("ui", "theme", "\"nord\""),
            "[general]\n\n[ui]  # color settings\ntheme = \"nord\"\n",
        ),
        (
            "サブセクションには当たらない",
            "[ui.colors]\nfoo = \"bar\"\n",
            ("ui", "theme", "\"nord\""),
            "[ui.colors]\nfoo = \"bar\"\n\n[ui]\ntheme = \"nord\"\n",
        ),
        (
            "後ろのセクションにある同名の鍵は触らない",
            "[ui]\n# theme = \"mocha\"\n\n[viewer]\ntheme = \"dracula\"\n",
            ("ui", "theme", "\"nord\""),
            "[ui]\ntheme = \"nord\"\n# theme = \"mocha\"\n\n[viewer]\ntheme = \"dracula\"\n",
        ),
        (
            "前のセクションにある同名の鍵は触らない",
            "[viewer]\ntheme = \"dracula\"\n\n[ui]\n# placeholder\n",
            ("ui", "theme", "\"nord\""),
            "[viewer]\ntheme = \"dracula\"\n\n[ui]\ntheme = \"nord\"\n# placeholder\n",
        ),
        (
            "前方一致する別の鍵は置き換えない",
            "[ui]\ntheme_file = \"a\"\n",
            ("ui", "theme", "\"nord\""),
            "[ui]\ntheme = \"nord\"\ntheme_file = \"a\"\n",
        ),
    ];
    for (name, contents, (section, key, value), expected) in cases {
        assert_eq!(
            upsert_section_kv(contents, section, key, value),
            expected,
            "{name}"
        );
    }
}

#[test]
fn 無い設定ファイルには既定を生成してから書く() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    persist_at(&path, "ui", &[("theme", String::from("\"nord\""))]).unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# high_contrast = false"), "{contents}");
    assert_eq!(
        parse(&contents),
        Config {
            ui: UiConfig {
                theme: Some(String::from("nord")),
                ..Default::default()
            },
            ..Default::default()
        }
    );
    assert!(!path.with_extension("toml.tmp").exists());
}

#[test]
fn ある設定ファイルはコメントを保って書き換える() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[layout]\n# explorer_width_pct = 24\n# viewer_width_pct = 38\n\n[ui]\ntheme = \"nord\"\n",
    )
    .unwrap();
    let layout = non_default().layout;
    persist_at(
        &path,
        "layout",
        &[
            ("explorer_width_pct", layout.explorer_width_pct.to_string()),
            ("viewer_width_pct", layout.viewer_width_pct.to_string()),
            ("terminal_split_pct", layout.terminal_split_pct.to_string()),
            ("explorer_split_pct", layout.explorer_split_pct.to_string()),
        ],
    )
    .unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# explorer_width_pct = 24"), "{contents}");
    let cfg = parse(&contents);
    assert_eq!(cfg.layout, layout);
    assert_eq!(cfg.ui.theme.as_deref(), Some("nord"));
}

#[test]
fn 文字セットはserdeの綴りでtomlの文字列になる() {
    let value = toml::Value::try_from(IconSet::Nerd).unwrap().to_string();
    assert_eq!(value, "\"nerd\"");
    let cfg = parse(&upsert_section_kv("", "ui", "icons", &value));
    assert_eq!(cfg.ui.icons, Some(IconSet::Nerd));
}

#[derive(Debug, PartialEq)]
enum Kind {
    Live,
    Restart,
}
use Kind::*;

type Change = fn(&mut Config);

#[test]
fn 全フィールドはliveかrestartのどちらか一方に属する() {
    let cases: &[(&str, Change, Kind)] = &[
        (
            "general.repo",
            |c| c.general.repo = Some("/p".into()),
            Restart,
        ),
        (
            "general.main_branch",
            |c| c.general.main_branch = "master".into(),
            Restart,
        ),
        (
            "general.shell",
            |c| c.general.shell = "/bin/fish".into(),
            Restart,
        ),
        (
            "general.repos",
            |c| c.general.repos = vec!["/p".into()],
            Restart,
        ),
        (
            "general.worktree_dir",
            |c| c.general.worktree_dir = Some("/wt".into()),
            Restart,
        ),
        (
            "general.auto_resume",
            |c| c.general.auto_resume = false,
            Restart,
        ),
        (
            "general.auto_resume_main",
            |c| c.general.auto_resume_main = true,
            Restart,
        ),
        (
            "terminal.inactive_scrollback",
            |c| c.terminal.inactive_scrollback = 1,
            Restart,
        ),
        (
            "terminal.active_scrollback",
            |c| c.terminal.active_scrollback = 1,
            Restart,
        ),
        ("viewer.theme", |c| c.viewer.theme = "nord".into(), Live),
        (
            "viewer.syntax_theme_file",
            |c| c.viewer.syntax_theme_file = Some("/t".into()),
            Live,
        ),
        ("viewer.tab_width", |c| c.viewer.tab_width = 4, Live),
        (
            "diff.default_view",
            |c| c.diff.default_view = DiffView::SideBySide,
            Live,
        ),
        ("diff.word_diff", |c| c.diff.word_diff = false, Live),
        ("keybinds", |c| c.keybinds = non_default().keybinds, Restart),
        (
            "api.provider",
            |c| c.api.provider = "command".into(),
            Restart,
        ),
        ("api.model", |c| c.api.model = "m".into(), Restart),
        (
            "api.command",
            |c| c.api.command = vec!["claude".into()],
            Restart,
        ),
        (
            "api.command_timeout_secs",
            |c| c.api.command_timeout_secs = 0,
            Restart,
        ),
        ("ui.theme", |c| c.ui.theme = Some("dracula".into()), Live),
        ("ui.high_contrast", |c| c.ui.high_contrast = true, Live),
        ("ui.icons", |c| c.ui.icons = Some(IconSet::Nerd), Live),
        (
            "ui.startup_animation",
            |c| c.ui.startup_animation = false,
            Live,
        ),
        (
            "updates.check_on_startup",
            |c| c.updates.check_on_startup = false,
            Restart,
        ),
        (
            "updates.check_interval_secs",
            |c| c.updates.check_interval_secs = 60,
            Restart,
        ),
        (
            "layout.explorer_width_pct",
            |c| c.layout.explorer_width_pct = 30,
            Live,
        ),
        (
            "layout.viewer_width_pct",
            |c| c.layout.viewer_width_pct = 42,
            Live,
        ),
        (
            "layout.terminal_split_pct",
            |c| c.layout.terminal_split_pct = 70,
            Live,
        ),
        (
            "layout.explorer_split_pct",
            |c| c.layout.explorer_split_pct = 60,
            Live,
        ),
    ];
    let base = Config::default();
    for (name, change, kind) in cases {
        let mut changed = base.clone();
        change(&mut changed);
        let live = changed.appearance_snapshot() != base.appearance_snapshot();
        let restart = has_restart_changes(&base, &changed);
        let observed = match (live, restart) {
            (true, false) => Some(Live),
            (false, true) => Some(Restart),
            _ => None,
        };
        assert_eq!(
            observed.as_ref(),
            Some(kind),
            "{name}: live={live} restart={restart}"
        );
    }
}

#[test]
fn liveフィールドを全部変えても再起動は要らない() {
    let base = Config::default();
    let mut all_live = base.clone();
    all_live.adopt_appearance(&non_default());
    assert_ne!(all_live.appearance_snapshot(), base.appearance_snapshot());
    assert!(!has_restart_changes(&base, &all_live));
    assert!(has_restart_changes(&base, &non_default()));
}
