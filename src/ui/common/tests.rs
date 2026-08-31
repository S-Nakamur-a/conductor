//! バッジ色計算の WCAG コントラスト保証。

use ratatui::style::Color;

use crate::ui::common::color::{hsl_to_rgb, readable_fg_on, relative_luminance};

/// WCAG 2.1 に基づく、2つの相対輝度値間のコントラスト比。
fn contrast_ratio(l1: f64, l2: f64) -> f64 {
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

#[test]
fn relative_luminance_endpoints() {
    assert!(relative_luminance(0, 0, 0).abs() < 1e-9);
    assert!((relative_luminance(255, 255, 255) - 1.0).abs() < 1e-9);
    // 緑は最大強度のとき青よりはるかに大きく輝度に寄与する。
    assert!(relative_luminance(0, 255, 0) > relative_luminance(0, 0, 255));
}

#[test]
fn readable_fg_matches_higher_contrast_choice() {
    // 明るい背景 → 黒文字が勝つ。
    assert_eq!(readable_fg_on(255, 255, 0), Color::Rgb(0, 0, 0));
    // 暗い背景 → 白文字が勝つ。
    assert_eq!(readable_fg_on(20, 20, 120), Color::Rgb(255, 255, 255));
}

/// バッジが取り得るすべての色相について、選ばれた文字色は選ばれなかった方より
/// 優れていなければならない — これにより、バッジが明るくても暗くても、
/// プロジェクト名が背景と衝突しないことを保証する。
#[test]
fn badge_fg_is_always_the_more_readable_choice() {
    for hue in 0..360 {
        let (r, g, b) = hsl_to_rgb(hue as f64, 0.6, 0.45);
        let bg = relative_luminance(r, g, b);
        let fg = readable_fg_on(r, g, b);
        let (chosen, rejected) = match fg {
            Color::Rgb(0, 0, 0) => (0.0, 1.0),
            Color::Rgb(255, 255, 255) => (1.0, 0.0),
            other => panic!("unexpected fg {other:?} at hue {hue}"),
        };
        assert!(
            contrast_ratio(bg, chosen) >= contrast_ratio(bg, rejected),
            "hue {hue}: chosen fg has worse contrast than the alternative",
        );
        // 健全性チェック: すべての色相でバッジは、大きい文字/UIコンポーネントの
        // 下限である 3:1 を余裕を持って上回る。
        assert!(
            contrast_ratio(bg, chosen) >= 3.0,
            "hue {hue}: contrast {:.2} fell below 3:1",
            contrast_ratio(bg, chosen),
        );
    }
}
