# grep_search
旧テスト 0 本 → 新テスト 4 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| (無し) | — | — |
| (新規) | 追加 | gitignoreされたファイルは検索対象から外れる |
| (新規) | 追加 | リテラルモードでは正規表現の特殊文字がそのまま扱われる |
| (新規) | 追加 | 大文字小文字の区別はオプションで切り替わる |
| (新規) | 追加 | search_fileは指定した1ファイルだけを検索しオフセットを刻む |

旧モジュールにテストは無かった。パターンコンパイル (正規表現/リテラル、大小文字)、
gitignore の尊重、マッチのオフセット計算という核となる振る舞いを 4 本で固定した。

API 変更: **`run_search()` / `run_search_files()` / `GrepProgress` / `BATCH_SIZE` を
core から削除した (team-lead の明示指示)。** どちらも `mpsc::channel` へ結果を送りながら
`thread::spawn` するバックグラウンド実行の関心事で、UI/呼び出し側 (将来の `svc` crate) の
責務であり domain 層には置かない。代わりに同期関数を公開する:
`compile_pattern(pattern, regex_mode, case_sensitive) -> Result<Regex, regex::Error>`、
`search_file(root, rel_path, re) -> Vec<GrepMatch>` (1 ファイルを検索する基本単位として新規)、
`search_files(root, rel_paths, re) -> Vec<GrepMatch>` (旧 `run_search_files` の同期版)、
`search_tree(root, re) -> Vec<GrepMatch>` (旧 `run_search` の同期版)。`GrepMatch` と
`MAX_RESULTS` はそのまま公開 (呼び出し側での打ち切り判定に必要)。スレッド起動・進捗配信は
呼び出し側が `search_file`/`search_files`/`search_tree` をスレッドで包んで実装する。

残したコメント (なぜ): モジュール doc に「同期関数のみを公開し、バックグラウンド実行は
呼び出し側の責務」という設計判断の理由を記載 (旧コードには無かった新規のコメント。API を
削った理由がコードだけでは分からないため)。
