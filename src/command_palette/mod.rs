//! コマンドパレット — あいまい検索できるコマンド索引。
//!
//! VSCode 風のコマンドパレット (Ctrl+P / :) を提供し、アプリの全コマンドを
//! 発見・実行できるようにする。各コマンドは対応する keymap の [Action] を
//! 持つ (ある場合)。これにより表示するショートカットとスコープ (グローバルか
//! フォーカス中パネルのレイヤーか) は keymap から都度導出され、古くなること
//! がない。パレット専用のコマンド (キーバインドなし) は action: None を持つ。
//!
//! types (CommandId/CommandCategory/CommandScope/PaletteCommand/
//! ScoredCommand のモデル)、commands (静的な COMMANDS テーブル)、search
//! (あいまいフィルタリング/スコアリング) に分割してある。呼び出し側が
//! 引き続き crate::command_palette::X を使えるよう、ここで re-export する。

mod commands;
mod search;
#[cfg(test)]
mod tests;
mod types;

pub use commands::COMMANDS;
pub use search::filter_commands;
// CommandCategory/PaletteCommand/ScoredCommand は今のところこのモジュールの
// 外から参照されていないが、分割前の公開パス (crate::command_palette::X) の
// 一部なので、それを保つために re-export してある。
#[allow(unused_imports)]
pub use types::{CommandCategory, CommandId, CommandScope, PaletteCommand, ScoredCommand};
