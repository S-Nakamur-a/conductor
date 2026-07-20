//! Claude Code's fixed palette (dark theme), lifted verbatim from the CLI's
//! hardcoded dark theme so the transcript reads like the real thing
//! regardless of the user's conductor theme. (Conductor's own theme drives
//! every other panel; only this overlay pins the Claude palette.)

use ratatui::style::Color;

use crate::theme::Theme;

/// Claude's signature coral/orange accent — `claude` token.
pub(crate) const CLAUDE: Color = Color::Rgb(215, 119, 87);
/// Primary text — follows the terminal's default foreground (`Color::Reset`),
/// exactly like the live PTY view (`vt100::Color::Default → Color::Reset`).
/// A hardcoded pure white here read as a harsh, brighter-than-live white when
/// scrolling from the live panel into the transcript; deferring to the
/// terminal default keeps the two views pixel-identical for body text.
pub(crate) const TEXT: Color = Color::Reset;
/// Tool-invocation bullet — `success` token (green).
pub(crate) const SUCCESS: Color = Color::Rgb(78, 186, 101);
/// Error connector — `error` token (coral red).
pub(crate) const ERROR: Color = Color::Rgb(255, 107, 128);
/// Dimmed/secondary text — `inactive` token (grey).
pub(crate) const INACTIVE: Color = Color::Rgb(153, 153, 153);
/// Accent for headings/links — `permission` token (periwinkle).
pub(crate) const PERMISSION: Color = Color::Rgb(177, 185, 249);
/// Inline-code / very dim — `subtle` token.
pub(crate) const SUBTLE: Color = Color::Rgb(80, 80, 80);

/// Build a Claude-flavored [`Theme`] for the Markdown renderer so prose,
/// headings, links and code in the transcript adopt Claude Code's palette
/// instead of the active conductor theme. Only the fields the Markdown
/// renderer consults are overridden; the rest are inherited from `base`.
pub(crate) fn claude_markdown_theme(base: &Theme) -> Theme {
    let mut t = base.clone();
    t.fg = TEXT;
    t.muted = INACTIVE;
    t.hint = INACTIVE;
    t.accent = CLAUDE;
    t.info = PERMISSION;
    t.success = SUCCESS;
    t.error = ERROR;
    t.warning = Color::Rgb(255, 193, 7);
    t.border_secondary = SUBTLE;
    t.code_fg = TEXT;
    t.code_bg = Color::Rgb(43, 43, 43);
    t
}
