//! Reflow transcript view — read-only, word-wrapped rendering of a Claude Code
//! session log inside the Claude PTY panel.
//!
//! `render` is called from `terminal_claude::render` whenever `app.reflow.active`
//! is true.  It maintains a `cached_lines` vector inside `app.reflow` and
//! rebuilds it only when the panel width changes, so there is no per-frame
//! re-parse of the `.jsonl` file or re-invocation of the Markdown renderer.
//!
//! ## Layout grammar
//!
//! Each conversation block is rendered in a two-column gutter layout:
//!
//! ```text
//! ⏺ assistant text line 1
//!   continuation line 2
//! ⏺ Bash(cargo build)
//!   ⎿  12 lines
//! ❯ user text line 1 (full-width background block)
//!   continuation line 2
//! ```
//!
//! The gutter (`MARKER_COLS = 2`) is always 2 display columns: marker glyph
//! padded to 2 cols for the first line, two spaces for continuations.
//! Markdown content is rendered at `width - MARKER_COLS` so the combined width
//! is exactly `width`, preserving the "1 logical line = 1 visual row"
//! invariant. User turns are the one exception: they bypass Markdown and
//! paint a full-width background block instead (see [`user_text`]).
//!
//! Split by responsibility: [`glyphs`] holds the gutter-width constants,
//! [`palette`] the fixed Claude Code color scheme, [`helpers`] the pure
//! marker/truncation functions, [`build`] the per-width line cache builder,
//! [`user_text`] the user-turn background-block renderer, and [`render`] the
//! public entry point that blits the cache each frame.

mod block_render;
mod build;
mod glyphs;
mod helpers;
mod palette;
mod render;
mod tool_lines;
mod user_text;

pub(crate) use build::LineMeta;
pub use render::render;

#[cfg(test)]
mod build_tests;
#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod user_text_tests;
