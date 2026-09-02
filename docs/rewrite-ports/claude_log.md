# claude_log (旧 src/reflow/log/)
旧テスト 67 本 (tests.rs 51 + tool_class.rs 内 16) → 新テスト 22 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| メタのレコードは飛ばす | 移植 | 描くターンの無いレコードは落とす (isMeta 行) |
| コマンドの包みはスラッシュ呼び出しとして描く | 移植 | ユーザターンのラッパーは画面で見えていた形に畳む |
| 引数の無い包みは素のコマンド名を出す | 移植 | 同上 |
| ローカルコマンドのstdoutは包みを外して整える | 移植 | 同上 |
| タスクの通知は要約に畳まれる | 移植 | 同上 |
| 要約の無いタスクの通知は何も描かない | 移植 | 同上 (2 行) |
| タスクの通知はどこにあっても畳まれる | 移植 | 同上 |
| 通知が二重でも要約は最初の1つだけ残る | 移植 | 同上 |
| ローカルコマンドのstdoutが空なら落とす | 移植 | 同上 |
| teammateの包みはteammateのブロックになる | 移植 | 同上 |
| teammateのsummary属性は無視する | 移植 | 同上 |
| 閉じていないteammateの本文は末尾まで取る | 移植 | 同上 |
| teammate_idが無ければ地の文として扱う | 移植 | 同上 |
| 本文の途中のteammateタグへの言及は書き換えない | 移植 | 同上 |
| system_reminderの範囲はユーザの本文に残す | 移植 | 同上 |
| reminderだけのユーザブロックも1ターンとして描く | 移植 | 同上 |
| 先頭の閉じていないコマンドタグは地の文のまま | 移植 | 同上 |
| 本文の途中のコマンドタグへの言及は書き換えない | 移植 | 同上 |
| 文字列のcontentは1つの本文ブロックになる | 移植 | 同上 (先頭行) |
| 包みのタグを引用したassistantの本文には触らない | 移植 | assistantの本文はラッパーを畳まない |
| 配列のcontentは複数種類のブロックになる | 移植 | 配列のcontentは種類ごとのブロックになる (完全一致で比較) |
| サイドチェーンの印を読む | 削除 | 描くターンの無いレコードは落とす の「サイドチェーン」行が isSidechain の読み取りごと固定している |
| 文字列の結果は行数を数える | 移植 | 結果の行は上限なく全部残す |
| プレビューの行は描画用に整える | 移植 | 結果の行から端末をずらす文字を除く |
| 配列の結果も行数を数える | 移植 | 結果の行は上限なく全部残す (JSON の配列形式で) |
| 結果の行は上限なく全部残す | 移植 | 同名 |
| 結果のidは同じセッションのレコードをまたいで解決する | 移植 | 結果の種類は前のレコードにある呼び出しから決まる |
| 知らない呼び出しidの結果は隠す | 移植 | 対応の無い結果は隠す |
| 結果のエラー印を拾う | 移植 | 同名 |
| 呼び出しの失敗印は後から来る対の結果で決まる | 移植 | 同名 (テーブル) |
| 結果が失敗でなければ印は立たない | 移植 | 同上 |
| 対の結果が無ければ印は立たない | 移植 | 同上 |
| thinkingの本文を拾う | 移植 | 同名 |
| thinkingの秒数は呼び出し側の値をそのまま通す | 削除 | 内部関数の引数の素通しを見ているだけ。秒数の由来は「thinkingの秒数は直前に表示したレコードとの時刻差」が固定 |
| 空の本文ブロックは除く | 移植 | 描くものの無いブロックは飛ばす |
| 知らない種類のブロックは飛ばす | 移植 | 同上 |
| 描くターンの無いレコードは落とす | 移植 | 同名 (message 無しの行を追加) |
| 混在したレコードでも件数が合う | 移植 | 壊れた行とノイズを飛ばしても順序が保たれる |
| キュー操作のレコードは何も描かない | 移植 | キュー操作の記録は描かない (JSON は fixture のまま) |
| セッションのメタ記録は飛ばす | 移植 | セッションのジャーナルは何も描かない (JOURNAL_RECORDS fixture) |
| ファイルが無ければ空を返す | 移植 | ファイルが無ければ空 |
| thinkingの長さは時刻から計算する | 移植 | thinkingの秒数は直前に表示したレコードとの時刻差 |
| 時刻が無ければthinkingの長さは1秒に落ちる | 移植 | 同上 |
| 飛ばしたメタのレコードは直前として数えない | 移植 | 同上 |
| compactの要約本文は表示しない | 移植 | compactの並びは実測どおりのブロックになり要約本文は出ない (ブロック列の完全一致で本文の漏れも固定) |
| compactの並びは実測どおりのブロックになる | 移植 | 同上 |
| 行数の無い添付は件数の節を落とす | 移植 | 添付で描くのはfileとcompact_file_referenceだけ |
| 添付が1行なら複数形にしない | 移植 | 同上 |
| 表示パスが無ければファイル名に落ちる | 移植 | 同上 |
| 表示しない種類の添付は何も描かない | 移植 | 同上 (3 行) |
| compact以外のsystemレコードは何も描かない | 移植 | セッションのジャーナルは何も描かない (system/something_else を JOURNAL_RECORDS に含めた) |
| (tool_class) readは集計バケットのreadになる | 移植 | ツールの分類表 |
| (tool_class) grepとglobは集計バケットのsearchになる | 移植 | 同上 |
| (tool_class) bashのlsは集計バケットのlistになる | 移植 | 同上 |
| (tool_class) bashのcatは集計バケットのreadに合流する | 移植 | 同上 |
| (tool_class) bashのそれ以外のコマンドはインラインになる | 移植 | 同上 |
| (tool_class) 先頭に空白があっても最初の語で振り分ける | 移植 | 同上 |
| (tool_class) writeはfile_pathを引数にしたインラインになる | 移植 | 同上 |
| (tool_class) editはupdateとして表示する | 移植 | 同上 |
| (tool_class) taskはdescriptionを引数にしたagentとして表示する | 移植 | 同上 |
| (tool_class) webfetchはurlを引数にしたfetchとして表示する | 移植 | 同上 |
| (tool_class) todowriteは隠す | 移植 | 同上 |
| (tool_class) 知らないツールは汎用の引数キー探索に落ちる | 移植 | 同上 |
| (tool_class) キーが無ければ引数はnoneになる | 移植 | 同上 |
| (tool_class) 空文字の引数はnoneになる | 移植 | 同上 |
| (tool_class) 知らないツールの引数は優先順にキーを試す | 移植 | 未知のツールの引数は固定の優先順でキーを試す |
| (tool_class) 既知のキーが1つも無ければnoneになる | 移植 | 同上 |

新規: ファイルから読む (load_session の結合 1 本)、結果の行は上限なく全部残す に「content 無し」行。

API 変更:
- `parse_jsonl(&str) -> Vec<LogEntry>` を公開に追加。`load_session` はそれを呼ぶだけ。テストが一時ファイルを経由しなくなった
- 生スキーマ型 (LogRecord / Block / Content / Message / TextOnly / ToolResultContent) の re-export をやめた。旧でも `#[allow(unused_imports)]` 付きで利用側は無かった
- `DisplayBlock` / `LogEntry` に PartialEq を derive (テストの完全一致比較のため)
- 内部: tool_use と tool_result の対応 (事前スキャンした errored 集合 + id→ResultKind) を `ToolPairing` 1 型にまとめ、`content_to_display_blocks` の 5 引数を 4 に。`sanitize_preview_line` は `sanitize.rs` の `sanitize_line` に
- ratatui 依存は元から無し

残したコメント (なぜ):
- schema: isMeta / isCompactSummary が何か、添付 29 種のうち描かれるのは 2 種 (許可リストの根拠)
- sanitize: 端末と ratatui でタブ・色エスケープの幅解釈が違う
- tool_class: テーブルが実測由来、Counted は is_error を無視 / Inline だけがエラー行を描く、BUCKET_ORDER の実測、ResultKind で Inline と Hidden を分けたまま持つ理由
- convert: ToolPairing の事前スキャンが要る理由 (result は後ろのレコード)、ペア無しを Hidden にする理由、ラッパー畳み込みの実測 (task-notification はどこにあっても、system-reminder は畳まない)
- session: 飛ばしたレコードを秒数の基準にしない、attachment の時刻は compact のもの、ジャーナルは描かれない
