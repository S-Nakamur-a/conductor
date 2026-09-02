# symbol_index
旧テスト 22 本 → 新テスト 20 本 (cargo test 20 passed, clippy --tests -D warnings ok, rustfmt ok)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| tests.rs: 作りたての索引は構築するまで使えない | 移植 | 作りたての索引は構築するまで使えない |
| tests.rs: 構築前の索引は定義を見つけない | 統合 | 作りたての索引は構築するまで使えない (同じ「未構築」状態の事実。1 本に) |
| tests.rs: rustのトップレベルの項目は全種類拾う | 移植 (拡張) | 宣言の種類を言語ごとに拾う (Rust に加え Go / TS もテーブルで固定。行番号も) |
| tests.rs: フィールドは定義として提示しない | 移植 (拡張) | フィールドと列挙子は定義として提示しない (コードが除いている EnumVariant も固定) |
| tests.rs: 参照検索はコードでない拡張子を飛ばす | 移植 | 同名 |
| tests.rs: 参照検索はコメントと文字列の一致を飛ばす | 移植 | 同名 |
| tests.rs: ホバーの参照数はフレームの予算に収まる | 移植 | 同名 (root は workspace 全体。crate 単体では `new` の最悪ケースにならない) |
| tests.rs: パースは名前が当たったファイルまで遅らせる | 移植 | 同名 |
| tests.rs: 実装検索はimplのシンボルに当たる | 移植 | 同名 |
| tests.rs: 根の付け替えは索引の答える対象を差し替える | 移植 | 同名 |
| tests.rs: 同じパスへの付け替えは何もしない | 移植 | 同名 |
| tests.rs: 付け替え前に始まったビルドは捨てる | 移植 | 同名 |
| tests.rs: パースできない言語の一致は捨てない | 移植 | 同名 |
| tests.rs: ローカル束縛は定義の候補にしない | 移植 | 同名 |
| code_mask.rs: rustはコメントと文字列と文字をマスクする | 統合 | マスクは地の文だけを隠す (テーブルの 1 行。期待値は旧のまま) |
| code_mask.rs: goはコメントと両方の文字列形式をマスクする | 統合 | マスクは地の文だけを隠す |
| code_mask.rs: typescriptのテンプレート補間は飛べるまま残す | 統合 | マスクは地の文だけを隠す |
| code_mask.rs: rustのformat捕捉は飛べるまま残す | 統合 | マスクは地の文だけを隠す |
| code_mask.rs: 複数行のブロックコメントは全行をマスクする | 移植 | 同名 |
| code_mask.rs: 出現番号はタブ展開を生き延びる | 移植 | 同名 |
| code_mask.rs: 対応しない言語は何も提示しない | 移植 (拡張) | 同名 (is_supported() == false も固定) |
| code_mask.rs: 範囲外の問い合わせはコードではない | 移植 | 同名 |
| (新規) | 追加 | 定義の解決は言語を跨がない (計画書 4.3 の 4。旧は semantic_index 側に居て symbol_index にテストが無かった) |
| (新規) | 追加 | 拡張子の分類 (Language::of_path / language_for_ext / same_language の分類不能は通す) |

API 変更:
- `same_language(asking, candidate)` と `Language { Rust, Go, TypeScript }` + `Language::of_path` を symbol_index に移した。
  semantic_index::same_language の呼び出し側 (bridge.rs) は `symbol_index::same_language` に付け替える。
  旧 roots::Language::of_file は目印ファイル (Cargo.toml など) も言語に数えるが、
  それは索引ルート・出自の概念なので symbol_index には持ち込まない。semantic_index を移す
  ときは roots 側で `for_marker(name).or_else(|| Language::of_path(path))` の形に。
  効果の差: 問い合わせ元が Cargo.toml のときに旧は .rs に絞っていたが新は絞らない
  (Cargo.toml は CodeMask が非対応なのでそもそもジャンプが出ない)。
- 拡張子→言語の判定を language.rs の 1 箇所にした (旧は index.rs の Lang / code_mask の
  grammar_for / language_for_ext / roots::Language::of_file の 4 箇所)。集合は和集合で、
  mts / cts / mjs / cjs も TypeScript 文法で索引・マスクの対象になった (旧は roots 側だけが知っていた)。
- `SymbolIndex::build()` は `Result<usize>` → `usize`。Err を返す経路が無かった。
  app/code_nav.rs の `match index.build() { Ok.. Err.. }` は count をそのまま送る形に。
- `Scope` を再エクスポート (Symbol.scope が pub なのに旧は名前で書けなかった)。
- `SymbolIndex` は `#[derive(Clone)]`。root と data の 2 本の Mutex を 1 本にした
  (旧の build が「root ロックの下で generation を読む」順序に頼っていた箇所が消える)。
- `language_for_ext` は pub(crate) → pub (viewer/fold.rs が crate 越しに使う)。
- extract_rust / extract_go / extract_ts / extract_common の 4 ファイルは extract.rs 1 本の
  テーブル (ノード種別 → SymbolKind、ローカル器の名前) に畳んだ。

足りない依存: `ignore` (報告後、lead が conductor-core/Cargo.toml に追加済み)。

残したコメント (なぜ):
- code_mask 冒頭: 出現番号で持つ理由 (タブ展開)、allowlist にする理由
- MAX_TRACKED_PER_LINE = 128: 実測で最大 76
- CodeMask::is_supported: 沈黙とゼロ件の扱いが逆になる理由
- masked_kinds: TS は template_string でなく string_fragment (補間を飲む)、Rust は raw 文字列も format! に届く
- subtract_format_args: tree-sitter-rust が format 文字列を分割しない (159 ファイル 945 件)
- extract の走査: カーソル 1 本 (再帰版は抽出の 30%)
- set_root: 古いツリーで答え続ける危険、進行中の構築は止められない
- build の retain: 名前でしか引けないのでローカルは載せない
- find_references / count_references_upto: `new` で 157ms、UI スレッド 16ms
- collect_references: 文法の無い言語の一致を残す理由
- same_language: rollbar の症状、分類不能を通す理由
