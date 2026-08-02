//! レイアウト描画 — UI全体の統括とオーバーレイ用ヘルパー。
//!
//! 全パネルとオーバーレイを組み立てる render_ui 関数と、
//! カラムの比率計算を行う accordion_widths ヘルパーを含む。

mod cache;
mod overlays;
mod render;

#[cfg(test)]
mod tests;

pub use cache::LayoutCache;
// 現状 tests (cfg(test)) からしか使われていないが、分割前のモジュール構成と揃えるため
// pub(crate) のまま残している。他の crate::ui サブモジュールが必要とする可能性への備え。
#[allow(unused_imports)]
pub(crate) use cache::accordion_widths;
pub(crate) use render::render_ui;
