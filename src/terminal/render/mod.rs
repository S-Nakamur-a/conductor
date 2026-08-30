//! terminal パネル (Claude Code / Shell / 埋め込みエディタ) の描画。
//!
//! 3 つの PTY ペインはそれぞれ独立した描画エントリポイントを持ち、
//! [crate::ui::layout] から個別に呼ばれる。vt100 → ratatui の変換とその
//! キャッシュは [pty] にまとまっている。

pub mod claude;
pub mod editor;
pub mod pty;
pub mod shell;
