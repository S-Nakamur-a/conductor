//! Layout rendering — top-level UI orchestration and overlay helpers.
//!
//! Contains the main `render_ui` function that composes all panels and overlays,
//! plus the `accordion_widths` helper for calculating column proportions.

mod cache;
mod overlays;
mod render;

#[cfg(test)]
mod tests;

pub use cache::LayoutCache;
// Only consumed by `tests` (cfg(test)) today; kept pub(crate) for parity with
// the pre-split module surface in case another crate::ui submodule needs it.
#[allow(unused_imports)]
pub(crate) use cache::accordion_widths;
pub(crate) use render::render_ui;
