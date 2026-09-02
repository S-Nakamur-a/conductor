# test_run
旧テスト 18 本 (go_test.rs 8 + rust_test.rs 10) → 新テスト 18 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| go_test::テストでないファイルからは何も出ない | 移植 | 同名で移植 (tests::go サブモジュール) |
| go_test::ファイルと関数とサブテストを見つける | 移植 | 同名で移植 |
| go_test::test_mainは実行できるテストとして出さない | 移植 | 同名で移植 |
| go_test::ルート直下のファイルはカレントディレクトリを対象にする | 移植 | 同名で移植 |
| go_test::厄介なディレクトリ名はシェル用に引用する | 移植 | 同名で移植 |
| go_test::ディレクトリ名の単引用符はエスケープする | 移植 | 同名で移植 |
| go_test::テストが無ければボタンも出ない | 移植 | 同名で移植 |
| go_test::テスト関数の外のサブテストは無視する | 移植 | 同名で移植 |
| rust_test::rustでないファイルからは何も出ない | 移植 | 同名で移植 (tests::rust サブモジュール) |
| rust_test::srcとtestsの外のファイルからは何も出ない | 移植 | 同名で移植 |
| rust_test::ファイルとモジュールと関数を見つける | 移植 | 同名で移植 |
| rust_test::tokio_testのasync関数も見つける | 移植 | 同名で移植 |
| rust_test::入れ子のモジュールは完全なパスになる | 移植 | 同名で移植 |
| rust_test::テストの無いcfg_testモジュールからは何も出ない | 移植 | 同名で移植 |
| rust_test::mod_rsはディレクトリのモジュールに対応する | 移植 | 同名で移植 |
| rust_test::クレートルートのファイルボタンは全部を走らせる | 移植 | 同名で移植 |
| rust_test::統合テストのファイルはtestフラグを使う | 移植 | 同名で移植 |
| rust_test::厄介なファイル名はシェル用に引用する | 移植 | 同名で移植 |

18 本とも固定していた事実 (Go の正規表現ベース走査、Rust の tree-sitter ベース走査、
シェルクォート、モジュールパスの組み立て) をそのまま持ち越した。go/rust それぞれの
`fn lines(src: &str) -> Vec<String>` ヘルパーは重複していたので `tests.rs` の親モジュールに
1 本へ集約 (旧コードで完全に同一実装が 2 箇所にあった重複の除去)。

API 変更: 旧 `test_run.rs` + `go_test.rs` + `rust_test.rs` (3 ファイル、フラットな
`src::test_run` / `src::go_test` / `src::rust_test`) を `test_run/{mod.rs, go.rs, rust.rs, tests.rs}`
の1モジュールへ統合。`shell_single_quote()` を `pub` から private へ縮小した。旧コードでは
`go_test.rs` と `rust_test.rs` が別クレートモジュールとして `crate::test_run::shell_single_quote`
を呼ぶ必要があったが、新レイアウトでは `go`/`rust` が `test_run` の子モジュールなので
祖先の private アイテムに直接アクセスできる。呼び出し側 (`src/viewer/*`) は
`TestRun`/`TestRunKind`/`scan_go_test_runs`/`scan_rust_test_runs` しか使っておらず
(`grep -rn 'crate::test_run\|crate::go_test\|crate::rust_test' src/` で確認済み)、
`shell_single_quote` を外部から呼ぶ箇所は無いため公開する理由が無かった。

残したコメント (なぜ): モジュール doc に、Rust だけ tree-sitter-rust でパースする理由
(`#[test]` が `#[cfg(test)] mod` に入れ子になり、ハーネスが完全なモジュールパスで
テストを名指しするため、行単位の正規表現では追えない — Go の平坦な命名規約との対比)。
`is_test_fn` の「属性は前方の兄弟だが文法バージョンによっては子にもなるので両方調べる」
という tree-sitter-rust の文法上の癖。
