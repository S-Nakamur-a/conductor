//! sheaf-core が要求する [`SyntacticLayer`] の実装。
//!
//! sheaf-core 自身は構文を知らないので、画面のどこに語があるかをこの層が答える。
//! 判定は [`CodeMask`] への委譲で、新しい解析はしない。答えるのは語の範囲だけで、
//! 定義も参照も出さない — 索引が黙っている位置は黙ったままにする。

use std::path::Path;

use sheaf_core::{Span, SyntacticLayer, Token};

use crate::syntax::{CodeMask, identifier_occurrences};

/// sheaf に構文層として渡すアダプタ。viewer が開いているファイル 1 個ぶんだけを答える。
pub struct Bridge<'a> {
    /// `source` を読んだ絶対パス。
    pub abs_path: &'a Path,
    /// そのファイルの元ソース (タブ展開前)。
    pub source: &'a str,
    pub mask: &'a CodeMask,
}

impl<'a> Bridge<'a> {
    /// 完全一致に限る。末尾一致だと別のツリーの同じ相対パスも通ってしまう。
    fn is_target_file(&self, path: &Path) -> bool {
        path == self.abs_path
    }

    fn locate_word(&self, line: u32, col: u32) -> Option<Span> {
        let source_line = self.source.lines().nth(line as usize)?;
        let line_1 = line as usize + 1;
        for (k, (start, end, _)) in identifier_occurrences(source_line).enumerate() {
            let col = col as usize;
            if col < start || col >= end {
                continue;
            }
            if !self.mask.is_code(line_1, k) {
                return None;
            }
            // ライフタイムは識別子の前に ' が付くが、tree-sitter 側の語には含まれない。
            // sheaf に渡す範囲だけをここで 1 バイト広げておく。
            let start_col = if start > 0 && source_line.as_bytes()[start - 1] == b'\'' {
                start - 1
            } else {
                start
            };
            return Some(Span {
                start_line: line,
                start_col: start_col as u32,
                end_line: line,
                end_col: end as u32,
            });
        }
        None
    }
}

impl<'a> SyntacticLayer for Bridge<'a> {
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token {
        if !self.is_target_file(path) {
            return Token::Unknown;
        }
        match self.locate_word(line, col) {
            Some(span) => Token::Word(span),
            None => Token::NotWord,
        }
    }
}
