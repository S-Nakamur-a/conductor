use super::*;

#[test]
fn 補色は色相を180度回して往復する() {
    // 純粋な赤(h=0) → 同じ彩度/明度のシアン(h=0.5)。
    assert_eq!(
        Theme::complement(Color::Rgb(255, 0, 0)),
        Color::Rgb(0, 255, 255)
    );
    // complement を2回適用すると(近似的に)元の色相に戻る。
    let original = Color::Rgb(203, 166, 247); // Catppuccin Mauve accent
    let twice = Theme::complement(Theme::complement(original));
    let (Color::Rgb(r, g, b), Color::Rgb(r2, g2, b2)) = (original, twice) else {
        unreachable!()
    };
    // HSL の往復変換による丸め誤差として、各チャンネル ±2 まで許容する。
    assert!((r as i16 - r2 as i16).abs() <= 2);
    assert!((g as i16 - g2 as i16).abs() <= 2);
    assert!((b as i16 - b2 as i16).abs() <= 2);
}

#[test]
fn 補色はrgb以外を変えない() {
    assert_eq!(Theme::complement(Color::Reset), Color::Reset);
}

#[test]
fn lightenの両端と中点() {
    let c = Color::Rgb(100, 100, 100);
    assert_eq!(Theme::lighten(c, 0.0), c);
    assert_eq!(Theme::lighten(c, 1.0), Color::Rgb(255, 255, 255));
    // 100 と 255 の中間は約178。
    assert_eq!(Theme::lighten(c, 0.5), Color::Rgb(178, 178, 178));
}

#[test]
fn lightenはrgb以外を変えない() {
    assert_eq!(Theme::lighten(Color::Reset, 0.5), Color::Reset);
}

/// ヘルパー: RGB カラーの知覚輝度(Rec. 601)、0〜255。
fn luma(c: Color) -> f64 {
    match c {
        Color::Rgb(r, g, b) => 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64,
        _ => 0.0,
    }
}

#[test]
fn 高コントラストはdarkテーマの薄いグレーを明るくする() {
    // ダークテーマでは、くすんだグレー(border、hint、muted)は明るくならな
    // ければならない — それがハイコントラスト変換の目的そのものである。
    for &name in Theme::all_names() {
        let base = Theme::from_name(name);
        if base.light {
            continue;
        }
        let hc = base.clone().high_contrast();
        assert!(
            luma(hc.border_unfocused) > luma(base.border_unfocused),
            "{name}: border_unfocused must brighten in high contrast"
        );
        assert!(
            luma(hc.hint) > luma(base.hint),
            "{name}: hint must brighten in high contrast"
        );
        assert!(
            luma(hc.fg) >= luma(base.fg),
            "{name}: fg must not dim in high contrast"
        );
    }
}

#[test]
fn 高コントラストはlightテーマの薄いグレーを濃くする() {
    // ライトテーマでは、白に近い背景に対するコントラストという意味で、
    // くすんだグレーは明るくではなく暗くならなければならない。
    for &name in Theme::all_names() {
        let base = Theme::from_name(name);
        if !base.light {
            continue;
        }
        let hc = base.clone().high_contrast();
        assert!(
            luma(hc.border_unfocused) < luma(base.border_unfocused),
            "{name}: border_unfocused must darken in high contrast"
        );
        assert!(
            luma(hc.hint) < luma(base.hint),
            "{name}: hint must darken in high contrast"
        );
        assert!(
            luma(hc.fg) <= luma(base.fg),
            "{name}: fg must not lighten in high contrast"
        );
    }
}

#[test]
fn lerpの両端と中点() {
    let a = Color::Rgb(0, 0, 0);
    let b = Color::Rgb(100, 200, 50);
    assert_eq!(Theme::lerp(a, b, 0.0), a);
    assert_eq!(Theme::lerp(a, b, 1.0), b);
    assert_eq!(Theme::lerp(a, b, 0.5), Color::Rgb(50, 100, 25));
}

#[test]
fn lerpは範囲外のtをクランプする() {
    let a = Color::Rgb(10, 20, 30);
    let b = Color::Rgb(200, 200, 200);
    assert_eq!(Theme::lerp(a, b, -1.0), a);
    assert_eq!(Theme::lerp(a, b, 2.0), b);
}

/// リストの出どころを1箇所に保つ。テーマを足しても書き足す先は
/// [Theme::all_names] だけで、light フラグの取り違えは依然ここで捕まる。
#[test]
fn lightテーマはリスト末尾の3つちょうど() {
    let light: Vec<&str> = Theme::all_names()
        .iter()
        .copied()
        .filter(|n| Theme::from_name(n).light)
        .collect();
    assert_eq!(
        light,
        ["catppuccin-latte", "solarized-light", "github-light"]
    );
}

#[test]
fn all_namesはdarkがlightより先() {
    let names = Theme::all_names();
    let last_dark = names
        .iter()
        .rposition(|n| !Theme::from_name(n).light)
        .expect("at least one dark theme");
    let first_light = names
        .iter()
        .position(|n| Theme::from_name(n).light)
        .expect("at least one light theme");
    assert!(
        last_dark < first_light,
        "dark themes must precede light themes"
    );
}

#[test]
fn 知らない名前は既定へ落ちる() {
    // 未知の名前はデフォルト(catppuccin-mocha、ダーク)を返す。
    let theme = Theme::from_name("does-not-exist");
    assert!(!theme.light);
}

/// 不一致は、あるテーマが片方のリストには登録されているのにもう片方では欠落しているか、
/// 名前が違っていることを意味する。
#[test]
fn all_namesの全部がfrom_nameを往復する() {
    for &n in Theme::all_names() {
        let theme = Theme::from_name(n);
        assert_eq!(
            theme.name, n,
            "Theme::from_name(\"{n}\").name == \"{}\", expected \"{n}\" — \
             check that from_name has a match arm for this theme",
            theme.name
        );
    }
}
