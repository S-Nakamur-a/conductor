//! sheaf-core が要求する [`SyntacticLayer`] の実装。
//!
//! sheaf-core 自身は構文を知らないので、意味索引が答えられない位置 — コメント、文字列
//! リテラル、索引に無い語 — をこの層が埋める。判定は [`CodeMask`] と [`SymbolIndex`] への
//! 委譲で、新しい解析はしない。

use std::path::{Path, PathBuf};

use sheaf_core::{Location, Span, SyntacticAnswer, SyntacticLayer, Token};

use crate::symbol_index::{CodeMask, SymbolIndex, identifier_occurrences, same_language};

/// sheaf に構文層として渡すアダプタ。viewer が開いているファイル 1 個ぶんだけを答える。
pub struct Bridge<'a> {
    /// `source` を読んだ絶対パス。
    pub abs_path: &'a Path,
    /// そのファイルの元ソース (タブ展開前)。
    pub source: &'a str,
    pub mask: &'a CodeMask,
    pub index: &'a SymbolIndex,
}

impl<'a> Bridge<'a> {
    /// 完全一致に限る。末尾一致だと別のツリーの同じ相対パスも通ってしまう。
    fn is_target_file(&self, path: &Path) -> bool {
        path == self.abs_path
    }

    fn locate_word(&self, line: u32, col: u32) -> Option<(Span, &'a str)> {
        let source_line = self.source.lines().nth(line as usize)?;
        let line_1 = line as usize + 1;
        for (k, (start, end, word)) in identifier_occurrences(source_line).enumerate() {
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
            return Some((
                Span {
                    start_line: line,
                    start_col: start_col as u32,
                    end_line: line,
                    end_col: end as u32,
                },
                word,
            ));
        }
        None
    }

    /// 索引の 1 始まりの行を、sheaf-core の 0 始まりの位置に直す。
    fn at_line(path: String, line: usize) -> Location {
        Location {
            path: PathBuf::from(path),
            line: line.saturating_sub(1) as u32,
            col: 0,
        }
    }
}

impl<'a> SyntacticLayer for Bridge<'a> {
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token {
        if !self.is_target_file(path) {
            return Token::Unknown;
        }
        match self.locate_word(line, col) {
            Some((span, _)) => Token::Word(span),
            None => Token::NotWord,
        }
    }

    fn definition_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        if !self.is_target_file(path) {
            return SyntacticAnswer::NotCode;
        }
        let Some((_, word)) = self.locate_word(line, col) else {
            return SyntacticAnswer::NotCode;
        };
        SyntacticAnswer::Found(
            self.index
                .find_definitions(word, self.abs_path)
                .into_iter()
                .map(|s| Self::at_line(s.file_path, s.line))
                .collect(),
        )
    }

    fn references_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        if !self.is_target_file(path) {
            return SyntacticAnswer::NotCode;
        }
        let Some((_, word)) = self.locate_word(line, col) else {
            return SyntacticAnswer::NotCode;
        };
        // ツリー全体を歩くので重い (ありふれた名前で実測 200 ファイル約 157ms)。描画のたびに
        // 走る経路からは呼ばない。
        let root = self.index.root();
        SyntacticAnswer::Found(
            self.index
                .find_references(word, &root)
                .into_iter()
                .filter(|r| same_language(self.abs_path, Path::new(&r.file_path)))
                .map(|r| Self::at_line(r.file_path, r.line))
                .collect(),
        )
    }
}
