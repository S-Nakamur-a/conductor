use super::*;

#[test]
fn complement_rotates_hue_180_and_round_trips() {
    // Pure red (h=0) → cyan (h=0.5) at the same saturation/lightness.
    assert_eq!(Theme::complement(Color::Rgb(255, 0, 0)), Color::Rgb(0, 255, 255));
    // Applying complement twice returns (approximately) the original hue.
    let original = Color::Rgb(203, 166, 247); // Catppuccin Mauve accent
    let twice = Theme::complement(Theme::complement(original));
    let (Color::Rgb(r, g, b), Color::Rgb(r2, g2, b2)) = (original, twice) else {
        unreachable!()
    };
    // Allow ±2 per channel for HSL round-trip rounding.
    assert!((r as i16 - r2 as i16).abs() <= 2);
    assert!((g as i16 - g2 as i16).abs() <= 2);
    assert!((b as i16 - b2 as i16).abs() <= 2);
}

#[test]
fn complement_leaves_non_rgb_unchanged() {
    assert_eq!(Theme::complement(Color::Reset), Color::Reset);
}

#[test]
fn lighten_endpoints_and_midpoint() {
    let c = Color::Rgb(100, 100, 100);
    assert_eq!(Theme::lighten(c, 0.0), c);
    assert_eq!(Theme::lighten(c, 1.0), Color::Rgb(255, 255, 255));
    // Halfway between 100 and 255 is ~178.
    assert_eq!(Theme::lighten(c, 0.5), Color::Rgb(178, 178, 178));
}

#[test]
fn lighten_leaves_non_rgb_unchanged() {
    assert_eq!(Theme::lighten(Color::Reset, 0.5), Color::Reset);
}

/// Helper: perceptual luminance (Rec. 601) of an RGB color, 0–255.
fn luma(c: Color) -> f64 {
    match c {
        Color::Rgb(r, g, b) => 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64,
        _ => 0.0,
    }
}

#[test]
fn high_contrast_brightens_dim_greys_on_dark_themes() {
    // On a dark theme, the dim greys (borders, hints, muted) must get lighter
    // — that is the whole point of the high-contrast transform.
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
fn high_contrast_darkens_dim_greys_on_light_themes() {
    // On a light theme, contrast against a near-white background means the
    // dim greys must get darker, not lighter.
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
fn lerp_endpoints_and_midpoint() {
    let a = Color::Rgb(0, 0, 0);
    let b = Color::Rgb(100, 200, 50);
    assert_eq!(Theme::lerp(a, b, 0.0), a);
    assert_eq!(Theme::lerp(a, b, 1.0), b);
    assert_eq!(Theme::lerp(a, b, 0.5), Color::Rgb(50, 100, 25));
}

#[test]
fn lerp_clamps_out_of_range_t() {
    let a = Color::Rgb(10, 20, 30);
    let b = Color::Rgb(200, 200, 200);
    assert_eq!(Theme::lerp(a, b, -1.0), a);
    assert_eq!(Theme::lerp(a, b, 2.0), b);
}

#[test]
fn from_name_light_themes_have_light_true() {
    assert!(Theme::from_name("catppuccin-latte").light);
    assert!(Theme::from_name("solarized-light").light);
    assert!(Theme::from_name("github-light").light);
}

#[test]
fn from_name_dark_themes_have_light_false() {
    assert!(!Theme::from_name("catppuccin-mocha").light);
    assert!(!Theme::from_name("dracula").light);
    assert!(!Theme::from_name("nord").light);
    assert!(!Theme::from_name("solarized-dark").light);
    assert!(!Theme::from_name("tokyo-night").light);
    assert!(!Theme::from_name("gruvbox").light);
    assert!(!Theme::from_name("rose-pine").light);
    assert!(!Theme::from_name("kanagawa").light);
}

#[test]
fn all_names_contains_all_eleven_themes() {
    let names = Theme::all_names();
    assert_eq!(names.len(), 11);
    assert!(names.contains(&"catppuccin-mocha"));
    assert!(names.contains(&"dracula"));
    assert!(names.contains(&"nord"));
    assert!(names.contains(&"solarized-dark"));
    assert!(names.contains(&"tokyo-night"));
    assert!(names.contains(&"gruvbox"));
    assert!(names.contains(&"rose-pine"));
    assert!(names.contains(&"kanagawa"));
    assert!(names.contains(&"catppuccin-latte"));
    assert!(names.contains(&"solarized-light"));
    assert!(names.contains(&"github-light"));
}

#[test]
fn all_names_dark_before_light() {
    let names = Theme::all_names();
    let last_dark = names
        .iter()
        .rposition(|n| !Theme::from_name(n).light)
        .expect("at least one dark theme");
    let first_light = names
        .iter()
        .position(|n| Theme::from_name(n).light)
        .expect("at least one light theme");
    assert!(last_dark < first_light, "dark themes must precede light themes");
}

#[test]
fn unknown_name_falls_back_to_default() {
    // Unknown names return the default (catppuccin-mocha), which is dark.
    let theme = Theme::from_name("does-not-exist");
    assert!(!theme.light);
}

/// Every name in `all_names()` must round-trip through `from_name` and
/// return the same canonical `name` field. A mismatch means a theme was
/// registered in one list but omitted or renamed in the other.
#[test]
fn all_names_round_trip_through_from_name() {
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
