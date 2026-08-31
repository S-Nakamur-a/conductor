//! メニューバー — ポインタと矢印キーであらゆるコマンドにたどり着ける経路。
//!
//! Conductor にはすでに2つのコマンド実行手段があった。default_keybinds.toml
//! によるチョードと、あいまい検索のコマンドパレットだ。どちらも何を探しているか
//! 事前に知っている必要がある。メニューバーはブラウズ可能な第三の経路であり、
//! タイトルバー直下に常時表示される帯で、ドロップダウンが操作対象ごとに
//! コマンドを分類して並べる。
//!
//! メニューバー自体は独自の振る舞いを持たない。各行は
//! [CommandId](crate::command_palette::CommandId) を持ち、実行時には
//! [App::execute_palette_command](crate::app::App::execute_palette_command) を
//! 呼ぶ。行を実行できるかの判定も同じ側にあり
//! ([App::command_enabled](crate::app::App::command_enabled))、
//! これはパレットが使うのと同じエントリポイントであり、event::global の
//! キーボード操作が呼ぶメソッドとも同じである。行の右側に表示されるショートカットは
//! キーマップからその都度読み取るので、ユーザ設定でのリバインドはこのモジュールに
//! 手を入れずとも反映される。
//!
//! ナビゲーションのヘルパーは [state] に純粋関数として置き、キーボードと
//! マウスの両ハンドラが共有する。

pub mod input;
pub mod model;
pub mod mouse;
pub mod render;
pub mod state;

#[cfg(test)]
mod tests;

pub use model::MenuItem;
pub use state::{MenuFocus, MenuState};
