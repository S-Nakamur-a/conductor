//! キーチョードをアクションに解決する、カスタマイズ可能なキーバインディング。
//!
//! エンジンは keymap-suite。語彙 (Action) は actions! マクロで一度だけ宣言し、
//! 既定のチョードは default_keybinds.toml に置いてコンパイル時に埋め込む。
//! ユーザの [keybinds] はその上に重ねるオーバーレイで、"<chord>" = false と
//! 書けば既定を外せる。レイヤーの積み方 (コンテキスト → グローバル) は
//! KeyMap が組む。suite はこちらのフォーカスを追跡しない。

mod action;
mod context;
mod map;
mod warning;

pub use action::Action;
pub use context::KeyContext;
pub use keymap_suite::ActionName;
pub use map::KeyMap;
pub use warning::KeybindWarning;

#[cfg(test)]
mod tests;
