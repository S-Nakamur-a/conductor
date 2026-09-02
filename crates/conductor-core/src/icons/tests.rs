use super::*;
use unicode_width::UnicodeWidthStr;

const PRIVATE_USE: std::ops::RangeInclusive<char> = '\u{e000}'..='\u{f8ff}';

/// Nerd Font に依存しない罫線。私用領域の検査から除く。
const BOX_DRAWING: [&str; 2] = ["\u{2502}", "\u{2570}"];

/// 各分岐に 1 つ以上当てるためのファイル名。網羅ではない。
const SAMPLE_FILE_NAMES: [&str; 44] = [
    "main.rs",
    "app.py",
    "index.js",
    "types.ts",
    "App.tsx",
    "main.go",
    "app.rb",
    "Main.java",
    "core.c",
    "core.cpp",
    "Program.cs",
    "App.swift",
    "Main.kt",
    "index.php",
    "init.lua",
    "run.sh",
    "index.html",
    "style.css",
    "data.json",
    "conf.yaml",
    "Cargo.toml",
    "pom.xml",
    "README.md",
    "notes.txt",
    "schema.sql",
    "api.graphql",
    "msg.proto",
    "logo.png",
    "logo.svg",
    "clip.mp4",
    "song.mp3",
    "dist.zip",
    "paper.pdf",
    "yarn.lock",
    ".env",
    "run.log",
    "mod.wasm",
    "foo.test",
    "unknown.xyzzy",
    "package.json",
    "Dockerfile",
    "Makefile",
    ".gitignore",
    "LICENSE",
];

const UI_GLYPHS: [(&str, Glyph); 21] = [
    ("COMMENT", COMMENT),
    ("COMMENT_SPAN", COMMENT_SPAN),
    ("ADD_COMMENT", ADD_COMMENT),
    ("RUN_TEST", RUN_TEST),
    ("RANGE_END", RANGE_END),
    ("KIND_SUGGEST", KIND_SUGGEST),
    ("KIND_QUESTION", KIND_QUESTION),
    ("PANEL_EXPLORER", PANEL_EXPLORER),
    ("PANEL_CHANGED", PANEL_CHANGED),
    ("PANEL_COMMENTS", PANEL_COMMENTS),
    ("PANEL_TERMINAL", PANEL_TERMINAL),
    ("PANEL_REVIEW", PANEL_REVIEW),
    ("LOCKED", LOCKED),
    ("MENU_REPO", MENU_REPO),
    ("MENU_WORKTREE", MENU_WORKTREE),
    ("MENU_REVIEW", MENU_REVIEW),
    ("MENU_VIEW", MENU_VIEW),
    ("MENU_PANEL", MENU_PANEL),
    ("MENU_SEARCH", MENU_SEARCH),
    ("MENU_TERMINAL", MENU_TERMINAL),
    ("MENU_HELP", MENU_HELP),
];

struct Case {
    label: String,
    nerd: &'static str,
    unicode: &'static str,
}

/// 情報を持つアイコン。フォールバック側でも必ず描く。
fn informative_icons() -> Vec<Case> {
    let files = SAMPLE_FILE_NAMES
        .iter()
        .map(|name| (name.to_string(), file_icon(name)))
        .chain([
            ("dir(open)".to_string(), dir_icon(true)),
            ("dir(closed)".to_string(), dir_icon(false)),
        ])
        .map(|(label, icon)| Case {
            label,
            nerd: icon.glyph(IconSet::Nerd),
            unicode: icon.glyph(IconSet::Unicode),
        });
    let arrows = [true, false].into_iter().map(|expanded| Case {
        label: format!("expand_arrow({expanded})"),
        nerd: expand_arrow(expanded, IconSet::Nerd),
        unicode: expand_arrow(expanded, IconSet::Unicode),
    });
    files.chain(arrows).collect()
}

fn every_glyph() -> Vec<Case> {
    let ui = UI_GLYPHS.iter().map(|(name, glyph)| Case {
        label: name.to_string(),
        nerd: glyph.get(IconSet::Nerd),
        unicode: glyph.get(IconSet::Unicode),
    });
    informative_icons().into_iter().chain(ui).collect()
}

/// ここが崩れるとアイコンより右の内容が 1 列ずれ、行がパネル端からはみ出す。
#[test]
fn 全グリフは1カラム幅() {
    for case in every_glyph() {
        assert_eq!(
            case.nerd.width(),
            1,
            "{} の nerd 側が 1 カラムでない",
            case.label
        );
        assert!(
            case.unicode.is_empty() || case.unicode.width() == 1,
            "{} のフォールバックが 1 カラムでない",
            case.label
        );
    }
}

#[test]
fn 情報を持つアイコンはフォールバック側でも描く() {
    for case in informative_icons() {
        assert!(
            !case.unicode.is_empty(),
            "{} にフォールバックの字形がない",
            case.label
        );
    }
}

/// 私用領域に入っていると Nerd Font の無い端末で tofu になる。
#[test]
fn フォールバックは私用領域を避ける() {
    for case in every_glyph() {
        let Some(ch) = case.unicode.chars().next() else {
            continue;
        };
        assert!(
            !PRIVATE_USE.contains(&ch),
            "{} のフォールバック U+{:04X} は私用領域にある",
            case.label,
            ch as u32
        );
    }
}

/// Plane 15 以降は端末による幅の扱いが読めないので使わない。
#[test]
fn nerdのグリフはbmpの私用領域に収まる() {
    for case in every_glyph() {
        if BOX_DRAWING.contains(&case.nerd) {
            continue;
        }
        let ch = case.nerd.chars().next().expect("字形が空");
        assert!(
            PRIVATE_USE.contains(&ch),
            "{} の nerd 側 U+{:04X} は BMP 私用領域の外にある",
            case.label,
            ch as u32
        );
    }
}

#[test]
fn ファイル名の一致は拡張子より優先し大文字小文字を区別しない() {
    let cases = [
        ("Cargo.toml", IconRole::Code),
        ("settings.toml", IconRole::Data),
        ("README.MD", IconRole::Doc),
        ("notes.md", IconRole::Doc),
        ("Dockerfile", IconRole::Data),
        (".ENV", IconRole::Sensitive),
        ("noext", IconRole::Doc),
    ];
    for (name, role) in cases {
        assert_eq!(file_icon(name).role, role, "{name}");
    }
    assert_ne!(
        file_icon("Cargo.toml").glyph(IconSet::Nerd),
        file_icon("settings.toml").glyph(IconSet::Nerd)
    );
}
