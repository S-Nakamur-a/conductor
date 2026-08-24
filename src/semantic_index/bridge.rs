//! sheaf-core が要求する SyntacticLayer の実装。
//!
//! sheaf-core 自身は構文を知らないので、意味索引が答えられない位置――コメント、
//! 文字列リテラル、索引に無い語――をこの層が埋める。判定は既存の CodeMask と
//! SymbolIndex への委譲で、新しい解析はしない。

use std::path::{Path, PathBuf};

use sheaf_core::{Location, Span, SyntacticAnswer, SyntacticLayer, Token};

use crate::symbol_index::{CodeMask, SymbolIndex, identifier_occurrences};

/// sheaf に構文層として渡すアダプタ。viewer が開いているファイル1個ぶんだけを答える。
pub struct Bridge<'a> {
    /// `source` を読んだ絶対パス。
    pub abs_path: &'a Path,
    /// そのファイルの元ソース(タブ展開前)。
    pub source: &'a str,
    pub mask: &'a CodeMask,
    pub index: &'a SymbolIndex,
}

impl<'a> Bridge<'a> {
    /// 末尾一致ではなく完全一致で見る。sheaf は索引が向いているツリーのルートを
    /// 繋いだパスを渡してくるので、こちらが読んだツリーと違えばここで食い違う。
    /// 末尾一致だと別のツリーの同じ相対パスも通ってしまい、そのずれが消える。
    fn is_target_file(&self, path: &Path) -> bool {
        path == self.abs_path
    }

    /// line・col の位置がコード上の識別子の出現なら、その範囲と識別子の
    /// テキストを返す。範囲はライフタイムの `'` を含む形に広げてある。
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
            // ライフタイムは識別子の前に ' が付くが、tree-sitter 側の語には
            // 含まれない。sheaf に渡す範囲だけをここで1バイト広げておく。
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
}

impl<'a> SyntacticLayer for Bridge<'a> {
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token {
        if !self.is_target_file(path) {
            return Token::Unknown;
        }
        match self.locate_word(line, col) {
            Some((span, _word)) => Token::Word(span),
            None => Token::NotWord,
        }
    }

    fn definition_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        if !self.is_target_file(path) {
            return SyntacticAnswer::NotCode;
        }
        let Some((_span, word)) = self.locate_word(line, col) else {
            return SyntacticAnswer::NotCode;
        };
        let locations = self
            .index
            .find_definitions(word, self.abs_path)
            .into_iter()
            .map(|s| Location {
                path: PathBuf::from(s.file_path),
                // Symbol::line は1始まり、sheaf-core の Location::line は0始まり。
                line: s.line.saturating_sub(1) as u32,
                col: 0,
            })
            .collect();
        SyntacticAnswer::Found(locations)
    }

    fn references_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        if !self.is_target_file(path) {
            return SyntacticAnswer::NotCode;
        }
        let Some((_span, word)) = self.locate_word(line, col) else {
            return SyntacticAnswer::NotCode;
        };
        // ツリー全体を歩くので重い (ありふれた名前で実測 200 ファイル約 157ms)。
        // 描画のたびに走る経路からは呼ばない。
        let root = self.index.root();
        let locations = self
            .index
            .find_references(word, &root)
            .into_iter()
            .filter(|r| {
                crate::semantic_index::same_language(self.abs_path, Path::new(&r.file_path))
            })
            .map(|r| Location {
                path: PathBuf::from(r.file_path),
                // Reference::line は 1 始まり、sheaf-core の Location::line は 0 始まり。
                line: r.line.saturating_sub(1) as u32,
                col: 0,
            })
            .collect();
        SyntacticAnswer::Found(locations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "\
fn value() {}

struct Holder<'a> {
    value: &'a str,
}

fn caller() {
    let x = value();
    // value mentioned only in a comment
    let s = \"value\";
    let c = 'x';
}
";

    /// SOURCE を1ファイルに書き出してビルドした SymbolIndex と、対応する
    /// CodeMask を返す。テストごとに tempdir を作り直す。
    fn build_fixture() -> (tempfile::TempDir, SymbolIndex, CodeMask) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), SOURCE).unwrap();

        let index = SymbolIndex::new(dir.path().to_path_buf());
        index.build().unwrap();
        let mask = CodeMask::compute(SOURCE, "lib.rs");
        (dir, index, mask)
    }

    fn line_col(line_idx: usize, needle: &str) -> (u32, u32) {
        let line = SOURCE.lines().nth(line_idx).unwrap();
        let col = line.find(needle).unwrap();
        (line_idx as u32, col as u32)
    }

    #[test]
    fn token_at_code_position_returns_word_with_exact_span() {
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(7, "value"); // let x = value();
        assert_eq!(
            bridge.token_at(&path, line, col),
            Token::Word(Span {
                start_line: line,
                start_col: col,
                end_line: line,
                end_col: col + "value".len() as u32,
            })
        );
    }

    #[test]
    fn token_at_comment_word_is_not_word() {
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(8, "value"); // // value mentioned only in a comment
        assert_eq!(bridge.token_at(&path, line, col), Token::NotWord);
    }

    #[test]
    fn token_at_string_literal_word_is_not_word() {
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(9, "value"); // let s = "value";
        assert_eq!(bridge.token_at(&path, line, col), Token::NotWord);
    }

    #[test]
    fn 別の言語の同名の定義には落とさない() {
        // tree-sitter の索引は名前でしか引けないので、Go の rollbar が
        // TypeScript の const rollbar に当たる。実際に踏んだ症状がこれ。
        let dir = tempfile::tempdir().unwrap();
        let go = "package main\n\nfunc use() { rollbar.SetToken(\"x\") }\n";
        std::fs::write(dir.path().join("main.go"), go).unwrap();
        std::fs::write(
            dir.path().join("page.tsx"),
            "const rollbar = useRollbar();\n",
        )
        .unwrap();
        let index = SymbolIndex::new(dir.path().to_path_buf());
        index.build().unwrap();

        let path = dir.path().join("main.go");
        let mask = CodeMask::compute(go, "main.go");
        let bridge = Bridge {
            abs_path: &path,
            source: go,
            mask: &mask,
            index: &index,
        };
        let line = go.lines().position(|l| l.contains("rollbar")).unwrap() as u32;
        let col = go
            .lines()
            .nth(line as usize)
            .unwrap()
            .find("rollbar")
            .unwrap() as u32;

        let SyntacticAnswer::Found(locations) = bridge.definition_at(&path, line, col) else {
            panic!("識別子として認識されていない");
        };
        assert!(
            locations
                .iter()
                .all(|l| l.path.extension().is_none_or(|e| e != "tsx")),
            "Go のファイルから TypeScript の定義に落ちた: {locations:?}"
        );
    }

    #[test]
    fn token_at_different_file_is_unknown() {
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(7, "value");
        let other = dir.path().join("other.rs");
        assert_eq!(bridge.token_at(&other, line, col), Token::Unknown);
    }

    #[test]
    fn token_at_lifetime_includes_apostrophe_in_span() {
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(3, "'a"); // value: &'a str,
        let col = col + 1; // 'a' の文字そのもの(アポストロフィの次)を指す
        match bridge.token_at(&path, line, col) {
            Token::Word(span) => {
                let text =
                    &SOURCE.lines().nth(3).unwrap()[span.start_col as usize..span.end_col as usize];
                assert_eq!(text, "'a", "span should include the leading apostrophe");
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn token_at_char_literal_does_not_falsely_widen() {
        // char_literal は CodeMask::compute で丸ごと NonCode としてマスクされる
        // ため、'x' の x は NotWord になる。もしマスクがコードだと判定していれば
        // 誤って範囲を広げるだけで、誤答(間違った定義へのジャンプ)にはならない。
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(10, "'x'");
        let col = col + 1; // 'x' の文字そのもの
        assert_eq!(bridge.token_at(&path, line, col), Token::NotWord);
    }

    #[test]
    fn definition_at_converts_one_based_symbol_line_to_zero_based_location() {
        let (dir, index, mask) = build_fixture();
        let path = dir.path().join("lib.rs");
        let bridge = Bridge {
            abs_path: &path,
            source: SOURCE,
            mask: &mask,
            index: &index,
        };
        let (line, col) = line_col(7, "value"); // let x = value();
        let path = dir.path().join("lib.rs");

        let symbol = index
            .find_definitions("value", Path::new("lib.rs"))
            .into_iter()
            .next()
            .expect("value の定義が索引に無い");
        assert_eq!(
            symbol.line, 1,
            "fixture の前提: fn value() {{}} は1始まりで1行目"
        );

        match bridge.definition_at(&path, line, col) {
            SyntacticAnswer::Found(locations) => {
                assert_eq!(locations.len(), 1);
                assert_eq!(locations[0].line, symbol.line as u32 - 1);
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// リポジトリに同梱された実際の索引 (.conductor/index.scip, .conductor/index.hashes) を
    /// 使い、経路全体(索引ロード→期待ハッシュ照合→位置クエリ)が実際に Exact を返すことを
    /// 確かめる。索引を作ったあとに対象ファイルを編集していると内容ハッシュが一致せず
    /// Exact にならない。
    #[test]
    #[ignore = "リポジトリ本体の実インデックスに依存するため、作業ツリーがそのファイルについて \
                クリーンな状態でのみ通る"]
    fn real_index_resolves_its_own_definition_to_itself() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let store = crate::semantic_index::load(repo_root, repo_root)
            .expect(".conductor/index.scip と index.hashes がリポジトリに無い");

        let rel = "src/repo_path.rs";
        let abs = repo_root.join(rel);
        let source = std::fs::read_to_string(&abs).unwrap();
        let mask = CodeMask::compute(&source, "repo_path.rs");
        // token_at はこの索引を使わない(索引を経由しない位置だけが index に落ちる)ので、
        // ビルドせず空のまま渡してよい。
        let index = SymbolIndex::new(repo_root.to_path_buf());

        let (line, col) = source
            .lines()
            .enumerate()
            .find_map(|(i, l)| {
                l.contains("pub fn normalize")
                    .then(|| (i as u32, l.find("normalize").unwrap() as u32))
            })
            .expect("pub fn normalize が見つからない");

        let bridge = Bridge {
            abs_path: &abs,
            source: &source,
            mask: &mask,
            index: &index,
        };

        match sheaf_core::definition_at(&store, &bridge, Path::new(rel), line, col) {
            sheaf_core::Definition::Exact(locations) => {
                assert!(!locations.is_empty(), "自分自身の定義を含むはず");
            }
            other => panic!("expected Exact, got {other:?}"),
        }
    }
}
