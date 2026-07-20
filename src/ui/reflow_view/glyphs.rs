//! Marker-gutter layout constants shared by [`build`](super::build) (which
//! prefixes each transcript line with one of these) and
//! [`helpers`](super::helpers) (which pads/positions them).

/// Display columns reserved for the left-hand marker gutter.
pub(crate) const MARKER_COLS: usize = 2;

// Gutter markers MUST measure 1 column in `unicode-width` AND render 1 column in
// the terminal, or every transcript line ends up one short and its last char
// spills past the panel edge. Claude Code's glyphs (⏺ ✻ ⎿) are width-Narrow but
// many terminals/fonts render them 2 columns wide (⏺ even carries the Emoji
// property), so the count and the render disagree — the source of the scrollback
// "bleed". We use only glyphs this terminal provably renders at the width we
// count: ASCII for the bullet/prompt (always 1 col) and a box-drawing corner for
// tool results (the panel borders use the same block, so it renders narrow too).
// The host terminal's line-wrap is also disabled (see `enter_tui`) so even a
// wide glyph in message *content* can't wrap into a neighbouring panel.

/// Bullet/record marker for assistant messages and tool invocations.
/// ASCII so it can't be widened by emoji presentation (Claude Code uses ⏺).
pub(crate) const ASSISTANT_MARKER: &str = "*";

/// Prompt marker for user turns (Claude Code shows `>` before user input).
pub(crate) const USER_MARKER: &str = ">";

/// Corner glyph for tool result lines (box-drawing `└`, same block as the panel
/// borders so it renders one column wide; Claude Code uses ⎿).
pub(crate) const TOOL_RESULT_GLYPH: &str = "\u{2514}";

/// Marker for thinking blocks (Claude Code uses ✻; ASCII keeps the width honest).
pub(crate) const THINKING_GLYPH: &str = "*";
