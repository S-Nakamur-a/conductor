//! カスタマイズ可能なキーバインディング — キーチョードをセマンティックな
//! アクションにマッピングする。
//!
//! 与えられた KeyContext について KeyEvent → Action を解決する KeyMap を
//! 提供する。ユーザによるオーバーライドは config.toml から読む。
//!
//! エンジンは keymap-suite（keymap_suite） — keymap-core/keymap-config/
//! keymap-seq をまとめたワンインポートの facade である。その設計にそのまま従う:
//!
//! * アクションの語彙は一度だけ宣言する。 Action enum、その安定した設定名、
//!   Action::ALL は、1つの actions!（keymap_suite::actions）ブロックから
//!   生成される。生成される ActionName（keymap_suite::ActionName）実装
//!   （from_name / name）が、手書きの from_str / as_str の名前テーブルを
//!   置き換え、そのまま suite のローダに差し込まれる。
//! * 一度ロードしたら、丸ごと所有する。 KeyMap は1つの Loaded<Action>
//!   （facade の TOML ビルド結果）を保持し、その layers マップは名前を
//!   キーにしている。各 KeyContext は1つのレイヤーを指し、Global は素の
//!   [keys] テーブル（keymap_suite::GLOBAL_LAYER）である。
//! * アクティブなチェーンは呼び出し側が組み立てる。 キーイベントごとに
//!   resolve_layered([context_layer, global], …) をライブラリに渡す —
//!   コンテキストのレイヤーが優先し、外れればグローバルへ落ち、完全な
//!   不一致なら None（「PTY へそのまま通す」）を返す。ライブラリはこちらの
//!   フォーカス/モードを一切追跡しない。そのスタックはこちら側の責任で
//!   あり、それこそが suite が意図するところである。
//! * デフォルト ⊕ ユーザは merge（keymap_suite::merge）で。 デフォルトは
//!   default_keybinds.toml に書かれており（コンパイル時に埋め込まれる）、
//!   [keybinds] からのユーザバインディングはその上に重ねるオーバーレイで
//!   ある。ユーザのチョードは、そのチョードに対するデフォルトを上書きする。
//!   "<chord>" = false はデフォルトを取り除くトゥームストーンである。
//!   本当に問題のあるものだけを KeybindWarning として表面化させる —
//!   override/unbind の注記は情報であって警告ではない。
//! * ヘルプは解決の逆写像である。 KeyMap::keys_for_action は facade の
//!   keys_for_action（keymap_suite::keys_for_action）を使うので、表示される
//!   ショートカットが実際の解決結果からずれることはない。

mod action;
mod context;
mod map;
mod warning;

pub use action::Action;
pub use context::KeyContext;
pub use map::KeyMap;
// crate::keymap::KeybindWarning のパスのため再エクスポートしている。クレート内
// では現状これを直接名指しするものがない（呼び出し側は KeyMap::with_warnings
// のタプルを Vec の要素型を明記せずに分解している）ので、rustc はこのエイリアス
// 経由で使われていることを検出できない。
#[allow(unused_imports)]
pub use warning::KeybindWarning;

#[cfg(test)]
mod tests;
