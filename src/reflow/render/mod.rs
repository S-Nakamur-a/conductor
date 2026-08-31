//! リフロー版トランスクリプトビュー。Claude PTY パネル内で Claude Code のセッションログを
//! 読み取り専用・折り返し表示する。
//!
//! render は terminal::render::claude::render から app.reflow.active が true のときに呼ばれる。
//! app.reflow 内に cached_lines ベクタを保持し、パネル幅が変わったときだけ再構築するため、
//! .jsonl ファイルの再パースや Markdown レンダラの再実行は毎フレーム発生しない。
//!
//! レイアウトの規則
//!
//! 各会話ブロックは2カラムのガター配置で描画される。
//!
//! ⏺ assistant text line 1
//!   continuation line 2
//! ⏺ Bash(cargo build)
//!   ⎿  12 lines
//! ❯ user text line 1 (full-width background block)
//!   continuation line 2
//!
//! ガター（MARKER_COLS = 2）は常に表示幅2カラムで、先頭行はマーカーグリフを2カラムに
//! パディングし、継続行はスペース2つにする。Markdown コンテンツは width - MARKER_COLS の
//! 幅で描画するため、合計幅はちょうど width になり「論理行1つ = 表示行1つ」の不変条件が
//! 保たれる。ユーザーターンだけは例外で、Markdown を経由せず全幅の背景ブロックを描画する
//! （[user_text] を参照）。

mod block_render;
mod build;
mod frame;
mod glyphs;
mod helpers;
mod palette;
mod tool_lines;
mod user_text;

pub(crate) use build::LineMeta;
pub use frame::render;

#[cfg(test)]
mod build_tests;
#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod user_text_tests;
