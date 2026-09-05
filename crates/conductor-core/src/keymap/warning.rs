//! ユーザのキーバインディング読み込み中に見つかった、致命的ではない問題。

/// keymap_suite::Warning は #[non_exhaustive] で、conductor が使わないシーケンスの
/// 概念を抱えているので、公開面には自前の型を置く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindWarning {
    /// アクション名が認識されず、そのバインディングは読み飛ばした。
    UnknownAction { key: String, action: String },
    /// 1 つのレイヤー内で 2 つのキーが同じチョードに解決した。後に書いた方が勝つ。
    Conflict { chord: String },
    /// [keybinds.layers.<name>] がどのコンテキストにも一致せず、無視した。
    UnknownLayer { layer: String },
    /// [keybinds] 全体がパースできず、ユーザの上書きを全て無視して既定を使った。
    InvalidConfig { detail: String },
}

impl std::fmt::Display for KeybindWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeybindWarning::UnknownAction { key, action } => {
                write!(f, "unknown keybind action {action:?} for key {key:?}")
            }
            KeybindWarning::Conflict { chord } => {
                write!(
                    f,
                    "keybind chord {chord:?} is bound more than once in one layer"
                )
            }
            KeybindWarning::UnknownLayer { layer } => {
                write!(f, "unknown keybind layer {layer:?}")
            }
            KeybindWarning::InvalidConfig { detail } => {
                write!(f, "could not parse [keybinds] config: {detail}")
            }
        }
    }
}
