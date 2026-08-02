//! KeybindWarning — キーマップ構築中に見つかった、致命的ではない問題。

// KeybindWarning — キーマップ構築中に見つかった、致命的ではない問題

/// ユーザのキーバインディング読み込み中に見つかった、致命的でない問題。
/// Conductor 独自の型なので、公開表面は keymap_suite::Warning
/// （#[non_exhaustive] であり、Conductor が使わないシーケンスの概念を
/// 抱えている）に依存しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindWarning {
    /// 設定内のアクション名が認識されなかった。バインディングはスキップされた。
    UnknownAction { key: String, action: String },
    /// 2つのキーが1つのレイヤー内で同じチョードに解決された。後に指定した方が勝つ。
    Conflict { chord: String },
    /// [keybinds.layers.<name>] テーブルが、どのコンテキストにも一致しない
    /// レイヤー名を使っていた。そのバインディングは無視された。
    UnknownLayer { layer: String },
    /// [keybinds] 設定が全くパースできなかった（不正な形式、または 0.x 以前の
    /// [keybinds.<context>] action→key 形式）。ユーザのオーバーライドは
    /// 無視され、組み込みのデフォルトが使われる。
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
