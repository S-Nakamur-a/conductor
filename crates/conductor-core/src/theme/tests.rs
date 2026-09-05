use super::*;

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Rec. 601 の輝度。明暗の向きを比べるだけなので厳密さは要らない。
fn luma(c: Color) -> f64 {
    match c {
        Color::Rgb(r, g, b) => 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b),
        _ => 0.0,
    }
}

fn builtin_themes() -> impl Iterator<Item = Theme> {
    Theme::all_names().iter().map(|name| Theme::from_name(name))
}

#[test]
fn 補色は彩度と明度を保って色相を180度回す() {
    assert_eq!(Theme::complement(rgb(255, 0, 0)), rgb(0, 255, 255));

    let mauve = rgb(203, 166, 247);
    let Color::Rgb(r, g, b) = Theme::complement(Theme::complement(mauve)) else {
        unreachable!()
    };
    // HSL 往復の丸め誤差として各チャンネル 2 まで許す
    assert!((i16::from(r) - 203).abs() <= 2);
    assert!((i16::from(g) - 166).abs() <= 2);
    assert!((i16::from(b) - 247).abs() <= 2);
}

#[test]
fn rgbでない色はどの演算もそのまま返す() {
    let red = rgb(255, 0, 0);
    for c in [Color::Reset, Color::Red, Color::Indexed(3)] {
        assert_eq!(Theme::complement(c), c);
        assert_eq!(Theme::lighten(c, 0.5), c);
        assert_eq!(Theme::darken(c, 0.5), c);
        assert_eq!(Theme::vivify(c, red, 0.5, 0.5), c);
        assert_eq!(Theme::lerp(c, red, 0.5), c);
        assert_eq!(Theme::lerp(red, c, 0.5), red);
        assert_eq!(Theme::perceptual_distance(c, red), 0.0);
    }
}

#[test]
fn lightenとdarkenの両端と中点() {
    let grey = rgb(100, 100, 100);
    let cases = [
        ("lighten 0", Theme::lighten(grey, 0.0), grey),
        ("lighten 1", Theme::lighten(grey, 1.0), rgb(255, 255, 255)),
        ("lighten 0.5", Theme::lighten(grey, 0.5), rgb(178, 178, 178)),
        ("darken 1", Theme::darken(grey, 1.0), grey),
        ("darken 0", Theme::darken(grey, 0.0), rgb(0, 0, 0)),
        ("darken 0.5", Theme::darken(grey, 0.5), rgb(50, 50, 50)),
    ];
    for (label, actual, expected) in cases {
        assert_eq!(actual, expected, "{label}");
    }
}

#[test]
fn lerpは両端と中点を通り範囲外のtをクランプする() {
    let a = rgb(0, 0, 0);
    let b = rgb(100, 200, 50);
    let cases = [
        (0.0, a),
        (1.0, b),
        (0.5, rgb(50, 100, 25)),
        (-1.0, a),
        (2.0, b),
    ];
    for (t, expected) in cases {
        assert_eq!(Theme::lerp(a, b, t), expected, "t={t}");
    }
}

#[test]
fn 知覚距離は同一で0で黒と白がおよそ765() {
    assert_eq!(Theme::perceptual_distance(rgb(9, 9, 9), rgb(9, 9, 9)), 0.0);
    let extremes = Theme::perceptual_distance(rgb(0, 0, 0), rgb(255, 255, 255));
    assert!((extremes - 765.0).abs() < 1.0, "{extremes}");
}

#[test]
fn vivifyは色相を保ち無彩色だけfallbackの色相を借りる() {
    let red = rgb(255, 0, 0);
    let blue = rgb(0, 0, 255);
    let grey = rgb(128, 128, 128);
    let cases = [
        ("amount 0 は不変", Theme::vivify(blue, red, 0.0, 0.5), blue),
        (
            "有彩色は自分の色相",
            Theme::vivify(blue, red, 1.0, 0.5),
            blue,
        ),
        (
            "無彩色は fallback の色相",
            Theme::vivify(grey, red, 1.0, 0.5),
            red,
        ),
    ];
    for (label, actual, expected) in cases {
        assert_eq!(actual, expected, "{label}");
    }
}

#[test]
fn 高コントラストは薄いグレーと本文を背景から遠ざける() {
    for base in builtin_themes() {
        let name = base.name;
        let hc = base.clone().high_contrast();
        // ダークは明るく、ライトは暗く。輝度の差に極性を掛けて 1 本の条件にする
        let away = |after: Color, before: Color| {
            let delta = luma(after) - luma(before);
            if base.light { -delta } else { delta }
        };
        assert!(
            away(hc.border_unfocused, base.border_unfocused) > 0.0,
            "{name}: border_unfocused"
        );
        assert!(away(hc.hint, base.hint) > 0.0, "{name}: hint");
        assert!(away(hc.fg, base.fg) >= 0.0, "{name}: fg");
    }
}

#[test]
fn 組み込みはダーク8つの後にライト3つが並ぶ() {
    let names = Theme::all_names();
    let (dark, light) = names.split_at(8);
    assert!(dark.iter().all(|n| !Theme::from_name(n).light), "{dark:?}");
    assert_eq!(
        light,
        ["catppuccin-latte", "solarized-light", "github-light"]
    );
    assert!(light.iter().all(|n| Theme::from_name(n).light));
}

#[test]
fn 全組み込み名はfrom_nameを往復する() {
    for name in Theme::all_names() {
        assert_eq!(Theme::from_name(name).name, *name);
    }
}

#[test]
fn 知らない名前は既定のcatppuccin_mochaへ落ちる() {
    let fallback = Theme::from_name("does-not-exist");
    assert_eq!(fallback.name, "catppuccin-mocha");
    assert_eq!(fallback.name, Theme::default().name);
    assert!(!fallback.light);
}

#[test]
fn コメントの書き手ごとの背景は全テーマで異なる() {
    for theme in builtin_themes() {
        assert_ne!(
            theme.comment_preview_bg, theme.comment_user_bg,
            "{}",
            theme.name
        );
    }
}
