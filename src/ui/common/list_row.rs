//! Shared hover/selection state and styling for the Explorer's two row-based
//! lists (file tree, Changed files). Centralized here so the
//! selection/focus/hover priority rules live in one place instead of being
//! re-derived (and potentially drifting) in every panel that renders rows —
//! see ADR D3 in `docs/plans/2026-07-30-ui-affordance-feedback.md`.

use ratatui::style::{Modifier, Style};
use std::time::{Duration, Instant};

/// How long a row keeps fading out after the pointer leaves it, in
/// milliseconds. Only the *leaving* row animates (D2): entering a row lights
/// it up instantly so pointer tracking never feels laggy, while the row left
/// behind eases out so a fast sweep across rows still reads as smooth motion.
const HOVER_FADE_MS: u64 = 120;

/// Hover state for a single row-based list: which row is currently under the
/// pointer, plus the row that was hovered most recently (with the instant it
/// was left) so its highlight can fade out instead of vanishing abruptly.
#[derive(Debug, Default)]
pub struct HoverRow {
    row: Option<usize>,
    left: Option<(usize, Instant)>,
}

/// The hover state of a single row, as returned by [`HoverRow::phase`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HoverPhase {
    /// The pointer is currently over this row.
    On,
    /// The pointer just left this row; `f64` is the remaining strength of the
    /// highlight in `[0.0, 1.0]` (`1.0` right after leaving, `0.0` once the
    /// fade completes).
    FadingOut(f64),
}

impl HoverRow {
    /// Update the currently hovered row. Passing `None` means the pointer is
    /// over no row in this list (e.g. it moved to a different panel).
    ///
    /// The previous row is only recorded into `left` (starting its fade) when
    /// the hover actually moves to a *different* row. Re-setting the same row
    /// on every mouse-move event (the common case while the pointer sits
    /// still) must not restart the fade animation each time.
    pub fn set(&mut self, row: Option<usize>) {
        if self.row == row {
            return;
        }
        if let Some(prev) = self.row {
            self.left = Some((prev, Instant::now()));
        }
        self.row = row;
    }

    /// The hover phase of `row`, or `None` if it is neither hovered nor
    /// fading out.
    pub fn phase(&self, row: usize) -> Option<HoverPhase> {
        if self.row == Some(row) {
            return Some(HoverPhase::On);
        }
        if let Some((left_row, left_at)) = self.left
            && left_row == row
        {
            let remaining = 1.0 - crate::anim::eased_progress(left_at.elapsed(), HOVER_FADE_MS);
            if remaining > 0.0 {
                return Some(HoverPhase::FadingOut(remaining));
            }
        }
        None
    }

    /// Whether any row in this list is currently fading out. Used by the main
    /// loop to decide whether a redraw pump is still needed (see
    /// `App::has_active_transition` and `src/anim.rs`).
    pub fn is_animating(&self) -> bool {
        self.left.is_some_and(|(row, left_at)| {
            self.row != Some(row) && left_at.elapsed() < Duration::from_millis(HOVER_FADE_MS)
        })
    }

    /// Test-only constructor that lets a test seed `left` with an already-
    /// elapsed instant, so fade-completion behavior can be asserted without
    /// sleeping the test thread.
    #[cfg(test)]
    fn with_left_at(row: usize, left_at: Instant) -> Self {
        Self {
            row: None,
            left: Some((row, left_at)),
        }
    }
}

/// Build the [`Style`] for a single list row from selection, panel focus, and
/// hover state.
///
/// Priority is selection over hover: a selected row keeps its selection
/// colors regardless of hover, since selection is the more consequential
/// state and diluting it with a hover tint would make it harder to keep track
/// of which row is selected while sweeping the pointer across the list.
///
/// Per ADR D1 (revised), hover for these row-based lists is expressed purely
/// through the foreground color (no background), matching the existing
/// precedent in the Viewer's line hover (`src/ui/viewer_panel/code_line.rs`).
/// Backing this with a background color instead was tried and rejected: on 7
/// of 11 themes it was indistinguishable from `selected_bg_inactive`, which is
/// exactly the state a hovered-but-unfocused row is in.
pub fn row_style(
    theme: &crate::theme::Theme,
    base_fg: ratatui::style::Color,
    selected: bool,
    panel_focused: bool,
    hover: Option<HoverPhase>,
) -> Style {
    if selected {
        return if panel_focused {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.selected_fg_inactive)
                .bg(theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD)
        };
    }

    let target = hover_emphasis(theme, base_fg);
    let fg = match hover {
        Some(HoverPhase::On) => target,
        Some(HoverPhase::FadingOut(t)) => crate::theme::Theme::lerp(base_fg, target, t),
        None => base_fg,
    };
    let style = Style::default().fg(fg);
    // Second, non-colour channel for the hover (D1 revised): colour alone has
    // to work across 11 palettes and every base colour a row can take, and it
    // lands weakest on `theme.fg` — the colour of the most common row by far.
    // An underline is palette-independent and survives colour-vision
    // deficiency, and it is already this codebase's hover vocabulary: the
    // Viewer underlines a jumpable symbol under the pointer
    // (`src/ui/viewer_panel/code_line.rs`). It also can't be confused with
    // selection, which is expressed as a background plus BOLD.
    //
    // Deliberately not carried through `FadingOut`: the underline says "the
    // pointer is *here*", which stops being true the moment it leaves, and a
    // modifier can't be interpolated so it would have to pop off at some
    // arbitrary point in the fade anyway. The colour fade alone smooths the
    // exit.
    if matches!(hover, Some(HoverPhase::On)) {
        style.add_modifier(Modifier::UNDERLINED)
    } else {
        style
    }
}

/// How far a hovered row's colour has to sit from its resting colour, in the
/// units of [`Theme::perceptual_distance`] (`0` identical, ~`765` black vs
/// white).
///
/// This floor exists because the obvious transform doesn't hold one. Pushing
/// the row's colour toward white (dark themes) or black (light ones) has no
/// headroom left when the colour already sits near that extreme — which
/// `theme.fg`, the colour of the overwhelming majority of rows, does by
/// design. Measured against the previous `lighten(base, 0.45)`, a `theme.fg`
/// row moved by ~53 while a `theme.hint` (untracked) row moved by ~237: hover
/// was four times weaker exactly where it fires most often, which is what made
/// it read as unreliable rather than subtle.
const HOVER_MIN_DISTANCE: f64 = 120.0;

/// Lightnesses a hovered colour may be pulled toward, in preference order, per
/// theme polarity. Two of them because one is not always reachable: a row
/// colour that already sits at the bright target (catppuccin's `info`, a
/// near-fully-saturated cyan at L 0.73) has nowhere to go toward it, exactly as
/// `theme.fg` has nowhere to go toward white. The second target is on the
/// other side, so some direction always has room.
///
/// Both entries in a pair stay on the far side of the theme's background —
/// bright on a dark theme, deep on a light one — so whichever the search picks
/// is still comfortably legible.
const HOVER_TARGET_L_DARK: [f64; 2] = [0.85, 0.52];
const HOVER_TARGET_L_LIGHT: [f64; 2] = [0.34, 0.14];

/// Number of steps the search below walks; the granularity of the emphasis,
/// nothing more.
const HOVER_STEPS: u32 = 20;

/// The colour a hovered row's text moves to: the row's own colour intensified
/// toward the vivid end of its own hue, by the smallest amount that clears
/// [`HOVER_MIN_DISTANCE`].
///
/// Two properties are doing the work here.
///
/// *Derived from `base_fg`*, never a fixed token. An earlier revision used
/// `theme.accent`, which destroyed the information the row's colour was
/// carrying: in the Changed files list the foreground encodes git stage state
/// (D6), and `accent == warning` (the staged colour) in both solarized-dark
/// and gruvbox — so hovering an *unstaged* row repainted it the exact colour
/// of a *staged* one. The hover told the user something false about their
/// working tree. `accent == info` on github-light had the same shape, turning
/// a hovered file into a directory. Transforming the row's own colour while
/// holding its hue cannot collide with another token by construction.
///
/// *Smallest sufficient push*, rather than a fixed amount. A fixed amount
/// makes the size of the change depend on where the row's colour happens to
/// start; searching for the floor instead gives every row a hover of roughly
/// the same strength, so the feedback reads as one consistent behaviour.
///
/// If no candidate clears the floor — no built-in theme is in that position,
/// but a custom palette could be — the most distant one found is used, so the
/// hover degrades to "as visible as this colour allows" rather than vanishing.
fn hover_emphasis(
    theme: &crate::theme::Theme,
    base_fg: ratatui::style::Color,
) -> ratatui::style::Color {
    let targets = if theme.light {
        HOVER_TARGET_L_LIGHT
    } else {
        HOVER_TARGET_L_DARK
    };
    let mut best = base_fg;
    let mut best_distance = 0.0;
    for step in 1..=HOVER_STEPS {
        let amount = f64::from(step) / f64::from(HOVER_STEPS);
        for target_l in targets {
            let candidate = crate::theme::Theme::vivify(base_fg, theme.accent, amount, target_l);
            let distance = crate::theme::Theme::perceptual_distance(base_fg, candidate);
            if distance >= HOVER_MIN_DISTANCE {
                return candidate;
            }
            if distance > best_distance {
                best = candidate;
                best_distance = distance;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::style::Color;

    fn test_theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn row_style_cases_are_pairwise_distinct() {
        let theme = test_theme();
        let base_fg = theme.fg;

        // Representative cases spanning the selected x panel_focused x hover
        // matrix. Comparing every pair (rather than asserting each against a
        // hand-picked expected `Style`) is what makes this test meaningful:
        // asserting fixed expected values would just restate the
        // implementation and pass trivially.
        let cases = [
            ("normal", row_style(&theme, base_fg, false, true, None)),
            (
                "hover_on",
                row_style(&theme, base_fg, false, true, Some(HoverPhase::On)),
            ),
            (
                "selected_focused",
                row_style(&theme, base_fg, true, true, None),
            ),
            (
                "selected_unfocused",
                row_style(&theme, base_fg, true, false, None),
            ),
        ];

        for i in 0..cases.len() {
            for j in (i + 1)..cases.len() {
                assert_ne!(
                    cases[i].1, cases[j].1,
                    "expected `{}` and `{}` to differ",
                    cases[i].0, cases[j].0
                );
            }
        }
    }

    #[test]
    fn selected_row_ignores_hover() {
        let theme = test_theme();
        let base_fg = theme.fg;

        let no_hover = row_style(&theme, base_fg, true, true, None);
        let with_hover = row_style(&theme, base_fg, true, true, Some(HoverPhase::On));
        assert_eq!(no_hover, with_hover);

        let no_hover_inactive = row_style(&theme, base_fg, true, false, None);
        let with_hover_inactive = row_style(&theme, base_fg, true, false, Some(HoverPhase::On));
        assert_eq!(no_hover_inactive, with_hover_inactive);
    }

    #[test]
    fn hover_row_phase_reflects_current_row() {
        let mut hover = HoverRow::default();
        hover.set(Some(3));
        assert_eq!(hover.phase(3), Some(HoverPhase::On));
        assert_eq!(hover.phase(4), None);
    }

    #[test]
    fn moving_hover_starts_fade_on_previous_row() {
        let mut hover = HoverRow::default();
        hover.set(Some(3));
        hover.set(Some(4));

        assert_eq!(hover.phase(4), Some(HoverPhase::On));
        match hover.phase(3) {
            Some(HoverPhase::FadingOut(t)) => assert!(t >= 0.9, "expected t >= 0.9, got {t}"),
            other => panic!("expected FadingOut close to 1.0, got {other:?}"),
        }
    }

    #[test]
    fn fade_completes_after_duration_elapses() {
        let hover = HoverRow::with_left_at(3, Instant::now() - Duration::from_millis(200));
        assert_eq!(hover.phase(3), None);
    }

    #[test]
    fn resetting_same_row_does_not_restart_fade() {
        let mut hover = HoverRow::default();
        hover.set(Some(3));
        hover.set(Some(4));
        let first_left = hover.left;

        // Re-setting the already-current row must be a no-op for `left`.
        hover.set(Some(4));
        assert_eq!(hover.left, first_left);
    }

    #[test]
    fn is_animating_reflects_active_fade() {
        let mut hover = HoverRow::default();
        assert!(!hover.is_animating());

        hover.set(Some(3));
        assert!(!hover.is_animating(), "no previous row to fade yet");

        hover.set(Some(4));
        assert!(hover.is_animating());

        let done = HoverRow::with_left_at(3, Instant::now() - Duration::from_millis(200));
        assert!(!done.is_animating());
    }

    /// The fade must run *from* the hovered colour back *to* the base, not the
    /// other way round. Swapping `lerp`'s two colour arguments still compiles
    /// and still animates — it just plays the row lighting up after the
    /// pointer has left. Only checking the direction catches that.
    #[test]
    fn fading_out_starts_lit_and_ends_at_base() {
        let theme = test_theme();
        let base = theme.fg;
        let lit = row_style(&theme, base, false, true, Some(HoverPhase::On)).fg;

        assert_eq!(
            row_style(&theme, base, false, true, Some(HoverPhase::FadingOut(1.0))).fg,
            lit,
            "full strength must match the hovered colour, or the row jumps when the pointer leaves"
        );
        assert_eq!(
            row_style(&theme, base, false, true, Some(HoverPhase::FadingOut(0.0))).fg,
            Some(base),
            "zero strength must be back at the row's own colour"
        );
        let mid = row_style(&theme, base, false, true, Some(HoverPhase::FadingOut(0.5))).fg;
        assert_ne!(mid, lit);
        assert_ne!(mid, Some(base));
    }

    /// Regression guard for the defect that made hover *lie*: `theme.accent`
    /// as the hover colour equals `theme.warning` (the staged colour) on
    /// solarized-dark and gruvbox, so hovering an unstaged file made it look
    /// staged. Emphasis is now derived from the row's own colour, so this holds
    /// for every theme and every base colour that carries meaning.
    #[test]
    fn hover_never_repaints_a_row_as_another_meaningful_token() {
        for &name in crate::theme::Theme::all_names() {
            let theme = crate::theme::Theme::from_name(name);
            // The tokens whose meaning a hovered row must not impersonate:
            // the four D6 stage colours plus the tree's directory colour.
            let meaningful = [
                ("error/unstaged", theme.error),
                ("warning/staged", theme.warning),
                ("success/committed", theme.success),
                ("hint/untracked", theme.hint),
                ("info/directory", theme.info),
            ];
            for (base_name, base) in meaningful {
                let hovered = row_style(&theme, base, false, true, Some(HoverPhase::On))
                    .fg
                    .expect("row_style always sets a foreground");
                assert_ne!(
                    hovered, base,
                    "{name}: hovering {base_name} produced no visible change"
                );
                for (other_name, other) in meaningful {
                    if other == base {
                        continue;
                    }
                    assert_ne!(
                        hovered, other,
                        "{name}: hovering {base_name} makes it look like {other_name}"
                    );
                }
            }
        }
    }

    /// The point of the emphasis rework: the hover must be *equally* visible
    /// no matter which colour the row starts from. The old transform cleared
    /// this floor on dim rows and missed it by 2-3x on ordinary `theme.fg`
    /// ones, so the same gesture produced obviously different feedback
    /// depending on the file's git state.
    #[test]
    fn hover_clears_the_visibility_floor_on_every_theme_and_base_colour() {
        for &name in crate::theme::Theme::all_names() {
            let theme = crate::theme::Theme::from_name(name);
            let bases = [
                ("fg/tracked file", theme.fg),
                ("hint/untracked", theme.hint),
                ("info/directory", theme.info),
                ("error/unstaged", theme.error),
                ("warning/staged", theme.warning),
                ("success/committed", theme.success),
                ("accent/summary", theme.accent),
            ];
            for (base_name, base) in bases {
                let hovered = row_style(&theme, base, false, true, Some(HoverPhase::On))
                    .fg
                    .expect("row_style always sets a foreground");
                let distance = Theme::perceptual_distance(base, hovered);
                assert!(
                    distance >= HOVER_MIN_DISTANCE,
                    "{name}: hovering {base_name} only moves it by {distance:.0}, \
                     below the {HOVER_MIN_DISTANCE:.0} floor"
                );
            }
        }
    }

    /// Hover carries a second, non-colour channel so it survives palettes and
    /// colour-vision deficiency. Selection must not borrow it: the two states
    /// have to stay tellable apart when a hovered row sits next to the
    /// selected one.
    #[test]
    fn only_the_hovered_row_is_underlined() {
        let theme = test_theme();
        let base = theme.fg;
        let underlined =
            |style: Style| style.add_modifier.contains(Modifier::UNDERLINED);

        assert!(underlined(row_style(
            &theme,
            base,
            false,
            true,
            Some(HoverPhase::On)
        )));
        assert!(!underlined(row_style(&theme, base, false, true, None)));
        assert!(!underlined(row_style(&theme, base, true, true, None)));
        assert!(!underlined(row_style(
            &theme,
            base,
            true,
            true,
            Some(HoverPhase::On)
        )));
        assert!(
            !underlined(row_style(
                &theme,
                base,
                false,
                true,
                Some(HoverPhase::FadingOut(1.0))
            )),
            "the underline marks where the pointer *is*, so it must not linger \
             into the fade-out"
        );
    }

    #[test]
    fn lerp_dummy_color_is_used_directly_when_hover_is_none_or_non_rgb() {
        // Sanity check that row_style doesn't accidentally invoke lerp when
        // there's no hover, by using a non-RGB base_fg (lerp would leave it
        // untouched per `Theme::lerp`'s contract either way, but this keeps
        // the branch's shape explicit for future readers).
        let theme = test_theme();
        let style = row_style(&theme, Color::Reset, false, true, None);
        assert_eq!(style.fg, Some(Color::Reset));
    }
}
