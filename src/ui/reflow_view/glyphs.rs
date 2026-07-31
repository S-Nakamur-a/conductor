//! Marker-gutter layout constants shared by [`build`](super::build) (which
//! prefixes each transcript line with one of these) and
//! [`helpers`](super::helpers) (which pads/positions them).

/// Display columns reserved for the left-hand marker gutter.
pub(crate) const MARKER_COLS: usize = 2;

// These are Claude Code's own glyphs. `⏺`, `✻` and `⎿` measure one column in
// `unicode-width` but many terminals/fonts draw them two columns wide (`⏺`
// even carries the Emoji property), which used to push every following
// character one column right and spill the line past the panel edge — the
// scrollback "bleed". They were replaced with ASCII for exactly that reason.
//
// The fix is the one Claude Code itself uses: don't rely on the glyph's width
// at all. It emits an absolute column (CHA) right after the glyph, so whatever
// the terminal did with it, the body still starts where it should. The
// equivalent here is to leave the cell after the glyph unwritten — ratatui's
// diff then sees a discontinuity and the crossterm backend emits an absolute
// `MoveTo`. See `WIDTH_AMBIGUOUS_GLYPHS`, `super::build::width_risk_hole`, and
// the byte-level proof in `super::render_tests`.

/// Bullet/record marker for assistant messages and tool invocations
/// (Claude Code's `⏺`, U+23FA).
pub(crate) const ASSISTANT_MARKER: &str = "\u{23fa}";

/// Prompt marker for user turns (measured: Claude Code shows `❯`, U+276F).
/// Unlike `⏺`/`✻`/`⎿`, `❯` is a Dingbats-block character with no
/// Emoji_Presentation default, so it is not width-ambiguous and needs no
/// unwritten cell after it — which matters, because it sits inside a
/// full-width background block where a hole would read as a notch.
pub(crate) const USER_MARKER: &str = "\u{276f}";

/// Corner glyph for tool result lines (Claude Code's `⎿`, U+23BF).
pub(crate) const TOOL_RESULT_GLYPH: &str = "\u{23bf}";

/// Marker for thinking blocks (Claude Code's `✻`, U+273B).
pub(crate) const THINKING_GLYPH: &str = "\u{273b}";

/// Glyphs that `unicode-width` measures as one column but a terminal may draw
/// two columns wide — `⏺` even carries the Emoji property. A line containing
/// one of these gets the cell right after it left unwritten, which forces an
/// absolute cursor move and pins the text that follows to its intended
/// column no matter how wide the glyph actually rendered (see
/// `super::build::width_risk_hole`).
///
/// `❯` (U+276F) and `›` (U+203A) are deliberately **not** listed: both are
/// plain punctuation with no Emoji presentation, and the user turn's `❯` sits
/// inside a full-width background block where an unwritten cell would show as
/// a notch in the background.
const WIDTH_AMBIGUOUS_GLYPHS: [char; 3] = ['\u{23fa}', '\u{23bf}', '\u{273b}'];

/// Whether `ch` is one of [`WIDTH_AMBIGUOUS_GLYPHS`].
pub(crate) fn is_width_ambiguous(ch: char) -> bool {
    WIDTH_AMBIGUOUS_GLYPHS.contains(&ch)
}

/// Marker for a collapsed `<teammate-message>` block (S4) — Conductor's own
/// multi-agent construct, not something Claude Code itself renders. `›`
/// (U+203A, General Punctuation) is not an Emoji_Presentation character and
/// measures 1 column under `unicode_width` (verified against this crate's
/// own version), so — like `USER_MARKER` above — it needs no unwritten cell.
pub(crate) const TEAMMATE_MESSAGE_GLYPH: &str = "\u{203a}";
