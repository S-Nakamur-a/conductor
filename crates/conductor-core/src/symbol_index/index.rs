//! 索引本体。ツリーを歩いて構築し、名前で定義・実装を引き、参照をテキスト検索する。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::code_mask::CodeMask;
use super::extract::extract;
use super::language::{Grammar, same_language};
use super::model::{Reference, Scope, Symbol, SymbolKind};

struct Inner {
    root: PathBuf,
    /// 構築が済むまで、また root が動いてから次の構築が済むまでは `None`。
    symbols: Option<Vec<Symbol>>,
    /// set_root のたびに進む。構築はこの値を持って始まり、終わるまでに進んでいたら捨てる。
    generation: u64,
}

/// スレッド間で共有できる索引。clone は同じ索引を指す。
#[derive(Clone)]
pub struct SymbolIndex {
    inner: Arc<Mutex<Inner>>,
}

/// 載っているシンボルは数万件あるので、件数だけ名乗る。
impl std::fmt::Debug for SymbolIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock().unwrap();
        f.debug_struct("SymbolIndex")
            .field("root", &inner.root)
            .field("symbols", &inner.symbols.as_ref().map(Vec::len))
            .finish()
    }
}

impl SymbolIndex {
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                root,
                symbols: None,
                generation: 0,
            })),
        }
    }

    /// 索引の向き先を別のツリーに変え、持っている内容を捨てる。同じ root なら何もしない。
    ///
    /// 古いツリーで答え続けると、別のブランチの行番号で正しいファイルへ飛ぶ誤りが
    /// 黙って生まれる。進行中の構築は止められないので、generation を進めて
    /// その結果が届いた時点で拒否する。
    pub fn set_root(&self, root: PathBuf) {
        let mut inner = self.inner.lock().unwrap();
        if inner.root == root {
            return;
        }
        inner.root = root;
        inner.symbols = None;
        inner.generation = inner.generation.wrapping_add(1);
    }

    /// root 以下のソースを解析して索引を作る。載せたシンボル数を返す。
    /// 走査中に root が動いていたら結果は捨てられ、0 を返す。
    pub fn build(&self) -> usize {
        let (root, generation) = {
            let inner = self.inner.lock().unwrap();
            (inner.root.clone(), inner.generation)
        };

        let mut symbols = Vec::new();
        for path in source_files(&root) {
            let Some(grammar) = Grammar::of_path(&path) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(tree) = grammar.parse(&source) else {
                continue;
            };
            let rel_path = relative(&path, &root);
            extract(grammar, tree.root_node(), &source, &rel_path, &mut symbols);
        }

        // 名前でしか引けないので、同名のローカルを区別する手立てが無い。
        // 載せると別のファイルの中の宣言が答えとして出る。
        symbols.retain(|s| s.scope == Scope::Global);

        self.publish(symbols, generation)
    }

    /// 構築が始まった時点の generation。
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.inner.lock().unwrap().generation
    }

    /// generation が刻まれてから root が動いていなければ symbols を設置する。
    /// 載せた件数を返し、捨てたなら 0。
    ///
    /// build と分けてあるのは、捨てるルールをスレッドを競わせずに検証するため。
    pub(super) fn publish(&self, symbols: Vec<Symbol>, generation: u64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return 0;
        }
        let count = symbols.len();
        inner.symbols = Some(symbols);
        count
    }

    /// `from` のファイルから見た name の定義。同じ言語のファイルにあるものだけを返す。
    pub fn find_definitions(&self, name: &str, from: &Path) -> Vec<Symbol> {
        self.symbols(|s| {
            s.name == name
                && !matches!(s.kind, SymbolKind::Field | SymbolKind::EnumVariant)
                && same_language(from, Path::new(&s.file_path))
        })
    }

    /// name という型に対する impl ブロック。
    pub fn find_implementations(&self, name: &str) -> Vec<Symbol> {
        self.symbols(|s| s.kind == SymbolKind::Impl && s.parent.as_deref() == Some(name))
    }

    fn symbols(&self, keep: impl Fn(&Symbol) -> bool) -> Vec<Symbol> {
        let inner = self.inner.lock().unwrap();
        inner
            .symbols
            .iter()
            .flatten()
            .filter(|s| keep(s))
            .cloned()
            .collect()
    }

    /// root 以下のソースから name への参照を探す。コメントや文字列の中の一致は除く。
    ///
    /// 名前が特徴的でないと遅い。`new` は約 200 ファイルに出現し、全部を解析すると
    /// 約 157ms かかる。フレームの中で呼ぶ側は [Self::count_references_upto] を使う。
    pub fn find_references(&self, name: &str, root: &Path) -> Vec<Reference> {
        self.collect_references(name, root, usize::MAX)
    }

    /// 参照を数え、cap 件で打ち切る。(件数, 打ち切ったか) を返す。
    ///
    /// ホバーのポップアップが UI スレッドの 16ms の予算の中で呼ぶ。正確な件数は
    /// 要らないので早めに切り上げ、呼び出し側は上限を「他多数」として描く。
    pub fn count_references_upto(&self, name: &str, root: &Path, cap: usize) -> (usize, bool) {
        let found = self.collect_references(name, root, cap);
        (found.len(), found.len() >= cap)
    }

    /// 正規表現で name に触れているファイルだけを構文解析する。ほとんどのファイルは
    /// 名前を含まないので、高価な解析が共通経路から外れる。
    fn collect_references(&self, name: &str, root: &Path, cap: usize) -> Vec<Reference> {
        let Ok(pattern) = regex::Regex::new(&format!(r"\b{}\b", regex::escape(name))) else {
            return Vec::new();
        };

        let mut refs = Vec::new();
        for path in source_files(root) {
            if !is_source_extension(&path) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let hits: Vec<(usize, &str)> = content
                .lines()
                .enumerate()
                .filter(|(_, line)| pattern.is_match(line))
                .collect();
            if hits.is_empty() {
                continue;
            }

            // 文法の無い言語の一致は捨てずに残す。参照の一覧はリポジトリ全体への主張
            // なので、空の結果は「存在しない」と読まれる。一覧に残ったコメントの一致は
            // 見て無視できるが、欠けた一覧は見えない。
            let rel_path = relative(&path, root);
            let mask = CodeMask::compute(&content, &rel_path);
            for (i, line) in hits {
                let line_1 = i + 1;
                let is_code = !mask.is_supported()
                    || pattern
                        .find_iter(line)
                        .any(|m| mask.is_code_at_column(line, line_1, m.start()));
                if !is_code {
                    continue;
                }
                refs.push(Reference {
                    file_path: rel_path.clone(),
                    line: line_1,
                    content: line.to_string(),
                });
                if refs.len() >= cap {
                    return refs;
                }
            }
        }
        refs
    }

    pub fn is_available(&self) -> bool {
        self.inner.lock().unwrap().symbols.is_some()
    }

    pub fn root(&self) -> PathBuf {
        self.inner.lock().unwrap().root.clone()
    }
}

/// root 以下の通常ファイル。.gitignore と隠しファイルを除く。
fn source_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    ignore::WalkBuilder::new(root)
        .build()
        .flatten()
        .map(|entry| entry.into_path())
        .filter(|path| path.is_file())
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// 参照検索の対象にする拡張子。文法の有無とは別で、ドキュメントや設定ファイルの
/// 中の一致を参照として出さないための表。
fn is_source_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "py"
                | "js"
                | "ts"
                | "jsx"
                | "tsx"
                | "go"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "java"
                | "rb"
                | "swift"
                | "kt"
                | "scala"
                | "zig"
        )
    )
}
