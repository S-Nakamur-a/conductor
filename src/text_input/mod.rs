//! カーソル移動と編集ができる、再利用可能なテキスト入力バッファ。
//!
//! カーソルナビゲーション、カーソル位置への挿入、前方/後方の削除、
//! 単語単位の移動、クリップボードからの貼り付けをサポートする TextInput 構造体を提供する。

mod boundary;
mod construct;
mod edit;
mod key_handler;
mod movement;
#[cfg(test)]
mod tests;

use std::fmt;
use std::ops::Deref;

/// カーソル位置を追跡するテキスト入力バッファ。
///
/// 単一行モードと複数行モード、カーソル移動、カーソル位置への挿入/削除、
/// 単語単位のナビゲーションをサポートする。
#[derive(Clone, Debug)]
pub struct TextInput {
    buffer: String,
    /// カーソル位置。buffer 内へのバイトオフセットとして表す。
    cursor: usize,
    /// この入力が複数行編集をサポートするかどうか。
    multiline: bool,
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.buffer)
    }
}

impl Deref for TextInput {
    type Target = str;
    fn deref(&self) -> &str {
        &self.buffer
    }
}
