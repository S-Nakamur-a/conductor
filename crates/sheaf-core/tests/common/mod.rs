//! 統合テストが共有する足回り。索引の組み立てと投入、構文層の代役。

#![allow(dead_code)]

use protobuf::{EnumOrUnknown, Message, MessageField};
use scip::types::{
    Document, Index, Metadata, Occurrence, PositionEncoding, SymbolInformation, TextEncoding,
    ToolInfo,
};
use sheaf_core::{IndexSource, Span, Store, SyntacticAnswer, SyntacticLayer, Token, blob_hash};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 語の切り出しをわざと素朴にした構文層。英数字・下線・非 ASCII が続く範囲を 1 語とし、
/// コメントも文字列も予約語も区別しない。この雑な層で通ることが、索引側の規則が
/// 構文層の賢さに依存していないことの検査になる。
pub struct Rough {
    calls: RefCell<Vec<(PathBuf, u32, u32)>>,
    answer: SyntacticAnswer,
}

impl Rough {
    pub fn new(answer: SyntacticAnswer) -> Self {
        Rough {
            calls: RefCell::new(Vec::new()),
            answer,
        }
    }

    pub fn calls(&self) -> Vec<(PathBuf, u32, u32)> {
        self.calls.borrow().clone()
    }
}

/// 索引だけで何が引けるかを見るときの構文層。
pub fn silent() -> Rough {
    Rough::new(SyntacticAnswer::NotCode)
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

impl SyntacticLayer for Rough {
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token {
        let Ok(src) = std::fs::read(path) else {
            return Token::Unknown;
        };
        let Some(text) = src.split(|b| *b == b'\n').nth(line as usize) else {
            return Token::NotWord;
        };
        let col = col as usize;
        if col >= text.len() || !is_word_byte(text[col]) {
            return Token::NotWord;
        }
        let mut start = col;
        while start > 0 && is_word_byte(text[start - 1]) {
            start -= 1;
        }
        let mut end = col + 1;
        while end < text.len() && is_word_byte(text[end]) {
            end += 1;
        }
        Token::Word(Span {
            start_line: line,
            start_col: start as u32,
            end_line: line,
            end_col: end as u32,
        })
    }

    fn definition_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        self.calls
            .borrow_mut()
            .push((path.to_path_buf(), line, col));
        self.answer.clone()
    }

    fn references_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer {
        self.calls
            .borrow_mut()
            .push((path.to_path_buf(), line, col));
        self.answer.clone()
    }
}

/// タグとスレッドごとに別の場所になる作業ディレクトリ。
pub fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-test-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn workdir_with_src(tag: &str) -> PathBuf {
    let dir = workdir(tag);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

pub fn write_and_hash(root: &Path, rel: &str, body: &str) -> String {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    blob_hash(body.as_bytes())
}

pub struct DocBuilder(Document);

/// `rel` は索引ルートからの相対。
pub fn doc(rel: &str) -> DocBuilder {
    DocBuilder(Document {
        relative_path: rel.to_string(),
        ..Default::default()
    })
}

impl DocBuilder {
    pub fn lang(mut self, language: &str) -> Self {
        self.0.language = language.to_string();
        self
    }

    pub fn utf16_positions(mut self) -> Self {
        self.0.position_encoding =
            EnumOrUnknown::from_i32(PositionEncoding::UTF16CodeUnitOffsetFromLineStart as i32);
        self
    }

    pub fn def(self, range: [i32; 3], symbol: &str) -> Self {
        self.occurrence(range, symbol, 1)
    }

    pub fn reference(self, range: [i32; 3], symbol: &str) -> Self {
        self.occurrence(range, symbol, 0)
    }

    pub fn occurrence(mut self, range: [i32; 3], symbol: &str, roles: i32) -> Self {
        self.0.occurrences.push(Occurrence {
            range: range.to_vec(),
            symbol: symbol.to_string(),
            symbol_roles: roles,
            ..Default::default()
        });
        self
    }

    pub fn info(mut self, info: SymbolInformation) -> Self {
        self.0.symbols.push(info);
        self
    }
}

pub struct IndexBuilder {
    metadata: Metadata,
    documents: Vec<Document>,
}

pub fn index() -> IndexBuilder {
    IndexBuilder {
        metadata: Metadata::default(),
        documents: Vec::new(),
    }
}

impl IndexBuilder {
    pub fn rooted_at(mut self, root: &Path) -> Self {
        self.metadata.project_root = format!("file://{}", root.display());
        self
    }

    pub fn utf8(self) -> Self {
        self.encoding(TextEncoding::UTF8 as i32)
    }

    pub fn utf16(self) -> Self {
        self.encoding(TextEncoding::UTF16 as i32)
    }

    pub fn encoding(mut self, encoding: i32) -> Self {
        self.metadata.text_document_encoding = EnumOrUnknown::from_i32(encoding);
        self
    }

    pub fn tool(mut self, name: &str) -> Self {
        self.metadata.tool_info = MessageField::some(ToolInfo {
            name: name.to_string(),
            ..Default::default()
        });
        self
    }

    pub fn unnamed_tool(mut self) -> Self {
        self.metadata.tool_info = MessageField::some(ToolInfo::default());
        self
    }

    pub fn add(mut self, doc: DocBuilder) -> Self {
        self.documents.push(doc.0);
        self
    }

    pub fn write(self, at: &Path) -> PathBuf {
        let index = Index {
            metadata: MessageField::some(self.metadata),
            documents: self.documents,
            ..Default::default()
        };
        std::fs::write(at, index.write_to_bytes().unwrap()).unwrap();
        at.to_path_buf()
    }
}

pub fn source(index: &Path, subroot: &str, expected: HashMap<PathBuf, String>) -> IndexSource {
    IndexSource {
        index: index.to_path_buf(),
        subroot: PathBuf::from(subroot),
        expected,
    }
}

/// 出自の表を「このファイルはこの内容だった」という申告から組む。鍵は索引ルートからの相対。
pub fn provenance(entries: &[(&str, &str)]) -> HashMap<PathBuf, String> {
    entries
        .iter()
        .map(|(rel, body)| (PathBuf::from(rel), blob_hash(body.as_bytes())))
        .collect()
}

/// いまツリーにある内容から組んだ出自の表。
pub fn hashes_of(root: &Path, rels: &[&str]) -> HashMap<PathBuf, String> {
    rels.iter()
        .map(|rel| {
            let bytes = std::fs::read(root.join(rel)).unwrap();
            (PathBuf::from(rel), blob_hash(&bytes))
        })
        .collect()
}

/// 索引 1 本を、索引ルート = リポジトリルート = `root` として投入する。
pub fn load_one(
    index: &Path,
    root: &Path,
    expected: HashMap<PathBuf, String>,
) -> sheaf_core::Result<Store> {
    Store::load(&[source(index, "", expected)], root)
}

/// 「いま root にあるツリーが、索引を生成したツリーそのものである」と申告して投入する。
/// 索引を書き出した直後に読むテストのためのもので、実運用の申告元は git になる。
pub fn load_as_indexed_tree(index: &Path, root: &Path) -> sheaf_core::Result<Store> {
    let mut expected = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                // 実索引のツリーでは target が数十 GB あり、歩くと検査が分単位になる。
                if name != "target" && name != ".git" {
                    stack.push(path);
                }
            } else if entry.file_type().is_ok_and(|t| t.is_file())
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(rel) = path.strip_prefix(root)
            {
                expected.insert(rel.to_path_buf(), blob_hash(&bytes));
            }
        }
    }
    load_one(index, root, expected)
}

/// 実索引の場所。`SHEAF_TEST_INDEX` に `.scip`、`SHEAF_TEST_ROOT` にツリーのルート。
pub fn real_index() -> (PathBuf, PathBuf) {
    let index =
        std::env::var("SHEAF_TEST_INDEX").expect("SHEAF_TEST_INDEX に .scip のパスを渡すこと");
    let root =
        std::env::var("SHEAF_TEST_ROOT").expect("SHEAF_TEST_ROOT にソースツリーのルートを渡すこと");
    (PathBuf::from(index), PathBuf::from(root))
}

/// 実索引を、生成時に書かれた出自の表とともに投入する。
pub fn load_real_index() -> (Store, PathBuf) {
    let (index, root) = real_index();
    let expected = sheaf_core::read_provenance(
        &index.with_file_name("index.hashes"),
        &sheaf_core::RustAnalyzer,
    )
    .expect("SHEAF_TEST_INDEX には rust-analyzer が作った索引を渡すこと");
    let store = Store::load(&[source(&index, "", expected)], &root).unwrap();
    (store, root)
}
