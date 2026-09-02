use super::*;

#[test]
fn 自動テーマ選択は設定済みか暗い背景ならnone明るいときだけlatte() {
    let cases = [
        // (輝度, 設定済みテーマ, 期待値)
        (0.9, Some("dracula"), None),
        (0.1, Some("github-light"), None),
        (0.9, None, Some("catppuccin-latte")),
        (0.501, None, Some("catppuccin-latte")),
        (0.5, None, None), // しきい値ちょうどは暗い扱い。
        (0.1, None, None),
        (0.0, None, None),
    ];
    for (lum, configured, want) in cases {
        assert_eq!(
            auto_theme_for_background(lum, configured),
            want,
            "lum={lum} configured={configured:?}"
        );
    }
}

/// OSC11 の応答から背景輝度を読む。終端が BEL の端末と、8bit (2 桁) チャネルで
/// 応答する端末があるので、どちらも同じ輝度になること。
#[test]
fn osc11の応答はどの綴りでも輝度を読める() {
    let cases = [
        ("\x1b]11;rgb:0000/0000/0000\x1b\\", Some(0.0)),
        ("\x1b]11;rgb:ffff/ffff/ffff\x1b\\", Some(1.0)),
        ("\x1b]11;rgb:ffff/ffff/ffff\x07", Some(1.0)),
        ("\x1b]11;rgb:ff/ff/ff\x1b\\", Some(1.0)),
        ("garbage", None),
        ("\x1b]11;rgb:ZZ/GG/HH\x1b\\", None),
        ("\x1b]11;rgb:ffff/ffff\x1b\\", None),
    ];
    for (response, want) in cases {
        match (parse_osc11_luminance(response), want) {
            (Some(got), Some(w)) => assert!((got - w).abs() < 0.01, "{response:?} -> {got}"),
            (got, w) => assert_eq!(got.is_none(), w.is_none(), "{response:?}"),
        }
    }

    // Catppuccin Mocha の base (#1e1e2e) は暗い背景として読めること。
    let v = parse_osc11_luminance("\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\").unwrap();
    assert!(v < 0.2, "expected dark bg, got {v}");
}

#[test]
fn シンボルを同梱する端末だけnerdと判定する() {
    for name in ["ghostty", "Ghostty", "WezTerm", "wezterm"] {
        assert_eq!(
            icon_set_for_term_program(name),
            Some(IconSet::Nerd),
            "{name} は Nerd Font のシンボルを同梱している"
        );
    }
}

/// フォントを同梱しない端末と、tmux 越しで内側が見えない場合は判定しないこと。
/// ここで推測すると、Nerd Font を入れていないユーザの画面が tofu で埋まる。
#[test]
fn 判らない端末では推測しない() {
    for name in [
        "kitty",
        "Alacritty",
        "iTerm.app",
        "Apple_Terminal",
        "tmux",
        "",
    ] {
        assert_eq!(
            icon_set_for_term_program(name),
            None,
            "{name} からはフォントの有無が判らない"
        );
    }
}
