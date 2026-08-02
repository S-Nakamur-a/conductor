//! keymap のテストスイートが共有するフィクスチャ。関心ごとに [resolution]
//! （各コンテキストにおけるデフォルトキーマップの解決）、[overrides]
//! （ユーザの [keybinds] オーバーレイ/トゥームストーン/警告の挙動）、
//! [edge_cases]（チョード正規化とその他の細かなエッジケース）に分割している。

use super::*;

mod edge_cases;
mod overrides;
mod resolution;

fn default_keymap() -> KeyMap {
    KeyMap::new(&toml::Table::new())
}
