//! 拡張子から言語と tree-sitter 文法を引く唯一の場所。
//!
//! 索引の構築、コードマスク、折りたたみ、言語の一致判定が別々に拡張子を
//! 見ていると、対応言語が片方だけ増えたときに、同じファイルがジャンプは
//! できるのに畳めない (あるいはその逆) という状態になる。

use std::path::Path;

/// 名前の一致を絞る単位。TypeScript と JavaScript は互いに引き合うので同じ言語。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Go,
    TypeScript,
}

impl Language {
    /// 分類できない拡張子は `None`。
    pub fn of_path(path: &Path) -> Option<Self> {
        Grammar::of_path(path).map(Grammar::language)
    }
}

/// 名前しか根拠が無い答えを、問い合わせ元と同じ言語のファイルに限る判定。
///
/// 索引は名前でしか引けないので、`.go` の `rollbar` が `.tsx` の `const rollbar` に
/// 当たる。分類できない拡張子は通す。落とすと、いま答えているものまで黙って消える。
pub fn same_language(asking: &Path, candidate: &Path) -> bool {
    match (Language::of_path(asking), Language::of_path(candidate)) {
        (Some(here), Some(there)) => here == there,
        _ => true,
    }
}

/// 拡張子に対応する tree-sitter の文法。
pub fn language_for_ext(ext: &str) -> Option<tree_sitter::Language> {
    Grammar::for_ext(ext).map(Grammar::tree_sitter)
}

/// tree-sitter の文法の単位。TSX は TypeScript と別の文法で解析する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Grammar {
    Rust,
    Go,
    TypeScript,
    Tsx,
}

impl Grammar {
    pub(super) fn for_ext(ext: &str) -> Option<Self> {
        Some(match ext {
            "rs" => Grammar::Rust,
            "go" => Grammar::Go,
            "ts" | "mts" | "cts" | "js" | "mjs" | "cjs" => Grammar::TypeScript,
            "tsx" | "jsx" => Grammar::Tsx,
            _ => return None,
        })
    }

    pub(super) fn of_path(path: &Path) -> Option<Self> {
        Self::for_ext(path.extension()?.to_str()?)
    }

    pub(super) fn language(self) -> Language {
        match self {
            Grammar::Rust => Language::Rust,
            Grammar::Go => Language::Go,
            Grammar::TypeScript | Grammar::Tsx => Language::TypeScript,
        }
    }

    pub(super) fn tree_sitter(self) -> tree_sitter::Language {
        match self {
            Grammar::Rust => tree_sitter_rust::LANGUAGE.into(),
            Grammar::Go => tree_sitter_go::LANGUAGE.into(),
            Grammar::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Grammar::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    pub(super) fn parse(self, source: &str) -> Option<tree_sitter::Tree> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&self.tree_sitter()).ok()?;
        parser.parse(source, None)
    }
}
