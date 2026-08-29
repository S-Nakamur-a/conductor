//! SymbolIndex: スレッドセーフで tree-sitter を使ったインデックス本体 —
//! リポジトリを走査して構築する処理と、コードナビゲーションが使う
//! 定義・実装・参照のクエリメソッド。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::CodeMask;
use super::extract_go::extract_go_symbols;
use super::extract_rust::extract_rust_symbols;
use super::extract_ts::extract_ts_symbols;
use super::model::{Reference, Scope, Symbol, SymbolKind};

/// mutex で保護された内部データ。
pub(super) struct IndexData {
    // 兄弟の tests モジュールが直接シンボルを仕込んだり調べたりできるように
    // pub(super) にしてある。分割前のファイルで両者が1つのモジュールに
    // あったのと同じ形。
    pub(super) symbols: Vec<Symbol>,
    pub(super) available: bool,
    /// [SymbolIndex::set_root] でインクリメントされる。ビルド開始時にこの値を
    /// 記録し、完了時点でこれが進んでいたら公開を拒否する。
    pub(super) generation: u64,
}

/// スレッドセーフな tree-sitter ベースのシンボルインデックス。
pub struct SymbolIndex {
    root: Arc<Mutex<PathBuf>>,
    // IndexData のフィールドと同じ理由で pub(super) にしてある — tests
    // モジュールが mutex に直接手を伸ばしてフィクスチャデータを仕込む。
    pub(super) data: Arc<Mutex<IndexData>>,
}

impl SymbolIndex {
    /// root を起点とする新しい空のシンボルインデックスを作る。
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(Mutex::new(root)),
            data: Arc::new(Mutex::new(IndexData {
                symbols: Vec::new(),
                available: false,
                generation: 0,
            })),
        }
    }

    /// インデックスの向き先を別のツリーに変更し、保持している内容を破棄する。
    ///
    /// 利用不可としてマークするのは副作用ではなく、それ自体が目的である。
    /// シンボルは古いツリーを説明するものであり、viewer が別の worktree に
    /// 移った後もそれをもとに答え続けると、見た目には何もおかしくないまま、
    /// 別のブランチで計算された行番号を使って正しいファイルへジャンプする
    /// という結果が黙って生まれてしまう。再構築が終わるまで黙っているのが
    /// 誠実な答え方である。
    ///
    /// すでに進行中の再構築は止められない（BackgroundOp は join handle を
    /// drop するだけで、ワーカーは誰か聞いているかどうかに関わらず結果を
    /// 書き込む）ので、代わりに generation を進め、そのワーカーの結果が
    /// 到着した時点で拒否されるようにする。
    pub fn set_root(&self, root: PathBuf) {
        let mut current = self.root.lock().unwrap();
        if *current == root {
            return;
        }
        *current = root;
        let mut data = self.data.lock().unwrap();
        data.symbols.clear();
        data.available = false;
        data.generation = data.generation.wrapping_add(1);
    }

    /// tree-sitter でソースファイルをパースしてインデックスを構築する。
    /// インデックスされたシンボル数を返す。ビルドが上書きされていた場合は 0。
    pub fn build(&self) -> Result<usize> {
        // root ロックの下でペアとして読む。set_root は両方を一緒に動かすため。
        // それぞれを独立にサンプリングすると、ビルドが古い root と新しい
        // generation を拾ってしまい、その結果 generation チェックをすり抜けた
        // 古いツリーを公開してしまう窓ができる — これはまさにこのカウンタが
        // 阻止するためにある事態である。set_root はこの2つのロックを同じ順序
        // で取得するので、片方を保持したままもう片方を取ってもデッドロックしない。
        let (root, generation) = {
            let root = self.root.lock().unwrap();
            (root.clone(), self.data.lock().unwrap().generation)
        };

        let mut parser = tree_sitter::Parser::new();
        let mut symbols = Vec::new();

        let walker = ignore::WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            let lang = match ext {
                "rs" => Lang::Rust,
                "go" => Lang::Go,
                "ts" | "tsx" => Lang::TypeScript,
                "js" | "jsx" => Lang::JavaScript,
                _ => continue,
            };

            let ts_lang: tree_sitter::Language = match lang {
                Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
                Lang::Go => tree_sitter_go::LANGUAGE.into(),
                Lang::TypeScript | Lang::JavaScript => {
                    if ext == "tsx" || ext == "jsx" {
                        tree_sitter_typescript::LANGUAGE_TSX.into()
                    } else {
                        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
                    }
                }
            };

            if parser.set_language(&ts_lang).is_err() {
                continue;
            }

            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let tree = match parser.parse(&source, None) {
                Some(t) => t,
                None => continue,
            };

            match lang {
                Lang::Rust => {
                    extract_rust_symbols(tree.root_node(), &source, &rel_path, &mut symbols)
                }
                Lang::Go => extract_go_symbols(tree.root_node(), &source, &rel_path, &mut symbols),
                Lang::TypeScript | Lang::JavaScript => {
                    extract_ts_symbols(tree.root_node(), &source, &rel_path, &mut symbols)
                }
            }
        }

        // ファイルの外から引けないシンボルは索引に載せない。ここは名前でしか
        // 引けないので、同名のローカルを区別する手立てが無く、別のファイルの
        // ものが答えとして出てしまう。SCIP は local に一意な符号を振るので
        // Document の中でなら引けるが、その区別はここには無い。
        symbols.retain(|s| s.scope == Scope::Global);

        Ok(self.publish(symbols, generation))
    }

    /// ビルドが開始時に自分自身へ刻む generation。
    ///
    /// テスト用の差し込み口: build はこれを root ロックの下で読むことで
    /// ペアをアトミックにサンプリングするが、このアクセサ単体ではそれができない。
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.data.lock().unwrap().generation
    }

    /// generation が刻まれてから root が動いていない限り、symbols をインデックス
    /// の内容として設置する。公開した件数を返す — 0 なら結果は捨てられたことを
    /// 意味する。
    ///
    /// [Self::build] とは分離してある。これにより破棄ルールをスレッドを
    /// 絡み合わせずに検証できる: 重要な順序（ビルド開始 → root が動く →
    /// ビルド完了）は、ここでは3つの単純な呼び出しとして表現できるが、build
    /// を通して駆動しようとすると、遅い走査と re-root を競合させてスケジューラ
    /// の協力を期待することになってしまう。
    pub(super) fn publish(&self, symbols: Vec<Symbol>, generation: u64) -> usize {
        let mut data = self.data.lock().unwrap();
        // このビルドがツリーを走査している間に root が動いたので、これらの
        // シンボルはもう誰も見ていないツリーを説明している。公開すると、もし
        // このビルドが後から終わった場合、より新しいビルドが生成したものを
        // 上書きしてしまう。
        if data.generation != generation {
            return 0;
        }
        let count = symbols.len();
        data.symbols = symbols;
        data.available = true;
        count
    }

    /// 指定した名前に一致する定義シンボルを探す。
    /// `from` のファイルから見た、その名前の定義。
    ///
    /// 名前しか根拠が無いので、別の言語のファイルにある同名の定義は落とす。
    /// 落とさないと Go の `rollbar` が TypeScript の `const rollbar` に当たり、
    /// ホバーがその宣言をそのまま答えとして出す。`from` は問い合わせ元のファイルで、
    /// 分類できない拡張子なら絞らない。
    pub fn find_definitions(&self, name: &str, from: &Path) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| {
                s.name == name && !matches!(s.kind, SymbolKind::Field | SymbolKind::EnumVariant)
            })
            .filter(|s| crate::semantic_index::same_language(from, Path::new(&s.file_path)))
            .cloned()
            .collect()
    }

    /// 指定した名前に一致する実装シンボルを探す。
    pub fn find_implementations(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Impl && s.parent.as_deref() == Some(name))
            .cloned()
            .collect()
    }

    /// ソースファイルを検索してシンボル名への参照を探す。.gitignore を
    /// 尊重するため ignore クレートのウォーカーを使う。
    ///
    /// ファイルごとに2パス行い、2パス目はほぼ全ての場合スキップされる:
    /// 素朴な正規表現の行スキャンで name に言及しているファイルだけを見つけ、
    /// *それらだけ* を [CodeMask] でパースして、コメントや文字列内の言及と
    /// 実際の使用箇所とを区別する。リポジトリのほとんどのファイルは指定の
    /// 名前をまったく含まないので、これにより tree-sitter によるパース
    /// （コストの高い部分、code_mask::CodeMask::compute を参照）を、
    /// 全ファイルに対して事前に払うのではなく、共通経路から外しておける。
    ///
    /// パースを「ほぼ全ての場合」スキップできるのは、名前が特徴的な場合に
    /// 限られ、ありふれた名前では成り立たない: new はここでは約200ファイルに
    /// 出現し、それら全部をパースすると約157msかかる。フレーム経路にある
    /// 呼び出し側は、これではなく [Self::count_references_upto] を使わなければ
    /// ならない。
    pub fn find_references(&self, name: &str, root: &Path) -> Vec<Reference> {
        self.collect_references(name, root, usize::MAX)
    }

    /// 参照を数える。cap 件見つかった時点で打ち切る。
    ///
    /// ホバーのポップアップはシンボルの横にこの数字を表示し、ポインタが
    /// 止まるたびに再描画される — UI スレッド上で、16msのフレーム予算内で。
    /// new のような名前の正確な件数を出すには、それに言及するすべての
    /// ファイルをパースする必要があり、計測では約157msかかりフレームを
    /// 10枚落とした。ポップアップが役に立つために正確な数字は必要ないので、
    /// スキャンを早めに打ち切り、呼び出し側は上限を「他多数」として描画する。
    ///
    /// (count, hit_cap) を返す。
    pub fn count_references_upto(&self, name: &str, root: &Path, cap: usize) -> (usize, bool) {
        let found = self.collect_references(name, root, cap);
        (found.len(), found.len() >= cap)
    }

    fn collect_references(&self, name: &str, root: &Path, cap: usize) -> Vec<Reference> {
        let pattern = match regex::Regex::new(&format!(r"\b{}\b", regex::escape(name))) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        let mut refs = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // コード以外の拡張子（ドキュメント、設定ファイル）は本物の参照を
            // 持つことはなく、たまたま名前に一致するテキストがあるだけ。
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(
                ext,
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
            ) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // パス1: 安価な正規表現スキャンで、パースはしない — この変更が
            // 入る前と同じコスト。hits は content を借用するので、この
            // ファイルのイテレーション中だけ生存する。
            let hits: Vec<(usize, &str)> = content
                .lines()
                .enumerate()
                .filter(|(_, line)| pattern.is_match(line))
                .collect();
            if hits.is_empty() {
                continue;
            }

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            // パス2: 実際に name に言及しているファイルに対してのみ行う。
            // viewer のマスクと同じように rel_path の拡張子で振り分ける。
            //
            // その言語の文法が存在しない場合、hits は捨てずにフィルタなしの
            // まま残す。他の箇所では、解析できないファイルはナビゲーションを
            // 提供しないという慎重な答えを返す。それはジャンプを提示することが
            // 1語について何かを主張する行為だからである。参照検索はリポジトリ
            // 全体について何かを主張する行為であり、そこでは一見慎重に見える
            // 答えのほうが危険になる: 「結果なし」は「本当に存在しない」と
            // 読まれてしまうので、パースできない言語のヒットを黙って全部
            // 捨てることは、本物の答えと同じ確信度で偽の主張をすることになる。
            // 一覧に残ったコメント内の一致は目に見えて無視もできるが、
            // 欠落した一覧は見えない。
            let mask = CodeMask::compute(&content, &rel_path);
            for (i, line) in hits {
                let line_1 = i + 1;
                let is_code = !mask.is_supported()
                    || pattern
                        .find_iter(line)
                        .any(|m| mask.is_code_at_column(line, line_1, m.start()));
                if is_code {
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
        }

        refs
    }

    /// インデックスが正常に構築済みかどうか。
    pub fn is_available(&self) -> bool {
        self.data.lock().unwrap().available
    }

    /// このインデックスの root パスを返す。
    pub fn root(&self) -> PathBuf {
        self.root.lock().unwrap().clone()
    }
}

// バックグラウンドスレッドでの利用向けに clone を許可する。
impl Clone for SymbolIndex {
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            data: Arc::clone(&self.data),
        }
    }
}

// 言語検出

#[derive(Debug, Clone, Copy)]
enum Lang {
    Rust,
    Go,
    TypeScript,
    JavaScript,
}
