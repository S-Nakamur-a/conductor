# review_store
旧テスト 39 本 → 新テスト 24 本 (移植 27 本を 19 本に統合、削除 12 本、新規 5 本)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| mod: db_pathはディレクトリを作る | 移植 | db_pathはディレクトリを作る (tempfile 化) |
| schema: openはwalジャーナルモードにする | 移植 | openはwalとbusy_timeoutを設定する (foreign_keys も固定) |
| schema: v4のcommit_refは既定でheadになる | 移植 | commit_refは既定でheadになる |
| schema: v4のcheckはworktreeとbranchの食い違いを拒む | 移植 | worktreeとbranchの組み合わせはcheckで縛られる (テーブル) |
| schema: v4のcheckはnullのbranchを許す | 移植 | 同上 (テーブルの 1 行) |
| schema: 既存のv5から最後まで移行できる | 移植 | 既存のv5から最後まで移行できる (到達先 9、stats テーブルの消滅も確認) |
| comments: コメントを足して取り出す | 移植 | コメントを足して取り出す |
| comments: 本文を編集する | 移植 | 本文と状態を編集する (status 更新も同じ 1 本に) |
| comments: 行範囲と書き手を持つ | 移植 | コメントを足して取り出す (line_end / author / branch を同じ 1 本で固定) |
| comments: 投稿済みの印は未投稿の一覧から外す | 移植 | 投稿済みの印は未投稿の一覧から外す |
| comments: 未解決の一覧は状態で絞る | 移植 | 未解決の一覧の絞り込み (テーブル) |
| comments: 未解決の一覧はbranchかworktreeのどちらかに当たる | 移植 | 同上 (branch NULL の旧行を worktree 列で当てる行を含む) |
| comments: 未解決の一覧はファイルパスで絞る | 移植 | 同上 |
| comments: 先頭8文字のプレフィックスで引ける | 移植 | idのプレフィックス解決 (テーブル) |
| comments: 当たらなければnoneを返す | 移植 | 同上 |
| comments: 不正なプレフィックスは拒む | 移植 | 同上 (% / _ / 空 / xyz の 4 値を持ち越し) |
| comments: 複数当たっても決定的に1件を返す | 移植 | 複数当たっても決定的に1件を返す (降順挿入の id 2 値を持ち越し) |
| comments: 公表より短いプレフィックスは拒む | 移植 | idのプレフィックス解決 (1..8 のループ) |
| replies: 返信を足して取り出す | 移植 | 返信を足して数える |
| replies: 親を消すと返信も消える | 移植 | 親を消すと返信も消える |
| replies: 返信の削除は親を消さない | 移植 | 返信の削除と編集はその返信だけに効く |
| replies: 返信の編集はその返信だけに効く | 移植 | 同上 + 無いidへの変更はエラーになる (not found を 6 メソッドで固定) |
| session_history: セッション履歴の保存と一覧と検索 | 移植 | セッション履歴の保存と一覧と検索 (テーブル) |
| view_state: ビュー状態の保存と取得 | 移植 | ビュー状態の保存と取得 (テーブル) |
| view_state: 選択中worktreeの保存と取得 | 移植 | 選択中worktreeの保存と取得 |
| worktree_metadata: 変更サマリの保存と取得と置き換え | 移植 | 変更サマリの保存と取得と置き換え |
| worktree_metadata: prレビューのメタ情報のupsertと取得 | 移植 | prレビューのメタ情報のupsertと取得 (6 列すべて往復) |
| stats: 日次の集計と連続日数 | 削除 | 機能廃止 (daily_stats は v9 で DROP) |
| stats: 日次の集計は知らない項目名を拒む | 削除 | 同上 |
| stats: 日次の集計は項目ごとに独立して増える | 削除 | 同上 |
| stats: 記録が無ければ今日の集計は全部0 | 削除 | 同上 |
| stats: 活動が無ければ連続日数は0 | 削除 | 同上 |
| stats: 連続日数は続いた過去の日を数える | 削除 | 同上 |
| stats: 間が空くと連続日数は切れる | 削除 | 同上 |
| stats: 今日の記録が無ければ昨日から数える | 削除 | 同上 |
| stats: セッション集計の一生 | 削除 | 機能廃止 (session_stats は v9 で DROP) |
| stats: セッション集計は知らない項目名を拒む | 削除 | 同上 |
| stats: 数が0のままでもセッションは閉じられる | 削除 | 同上 |
| stats: 複数のセッションは互いに独立 | 削除 | 同上 |
| (新規) | 新規 | walに切り替えられなくてもopenは成功する (:memory: は WAL にならないが open は Ok) |
| (新規) | 新規 | 読み手が居てもwalへの切替を待って成功する (別接続が読みトランザクションを持つ間に open) |
| (新規) | 新規 | v9は統計テーブルを落とす (v8 → v9、session_history は残る) |
| (新規) | 新規 | 無いidへの変更はエラーになる |
| (新規) | 新規 | ベースブランチと子ブランチ (旧は未テスト) |

API 変更:
- `add_review(9 引数)` → `add_review(NewReview { branch, file_path, line_start, line_end, kind, body, author })`。
  commit_ref はスキーマ既定 'HEAD' に任せ、worktree 列には branch を書く (旧の全呼び出しが
  worktree == branch かつ commit_ref == "HEAD" だった。CHECK が同じことを強制している)。
  呼び出し側: mcp_serve/tools.rs (1 + テスト 2)、app/review.rs (1)。
- `save_pr_review_meta(branch, 6 引数)` → `save_pr_review_meta(branch, &PrReviewMeta)`。
  `PrReviewMeta` は 6 列 (pr_number, pr_url, pr_title, base_ref, head_ref, author) すべてを持ち、
  get 側も同じ型で返す。呼び出し側: worktree/worktree_pr.rs (1)。読む側は pr_number / pr_url しか
  見ていないのでそのまま動く。
- 削除: `increment_daily_stat` `get_today_stats` `calculate_streak` `start_stats_session`
  `increment_session_stat` `end_stats_session` と `DailyStats` `SessionStatsSnapshot` `StreakInfo`。
  呼び出し側: app/state/stats.rs, app/*.rs の record_stat 周辺。
- `CommentKind` `Author` `CommentStatus` が rusqlite の `ToSql` / `FromSql` を実装 (`as_str()` と
  `Display` は維持)。
- `MIN_ID_PREFIX_LEN` を mod から再エクスポート (旧は comments モジュール直下で pub)。
- 他は名前・引数・戻り値とも同じ。

スキーマ: user_version 9 を追加 (daily_stats / session_stats を DROP)。v1〜v8 は不変。

残したコメント (なぜ):
- schema.rs 冒頭: 過去のマイグレーションを書き換えない理由 (既存 DB が通過済み)。
- schema.rs configure: busy_timeout を WAL より先に置く理由と、rusqlite が open 時に同じ既定を
  入れている事実 (実測: 読み手が居る WAL 切替は 5.4s 待って BUSY)。
- schema.rs configure: WAL 失敗を Err にしない理由 (呼び出し側が store を捨てる)。
- schema.rs migrate_to_v4: テーブルを作り直す理由 (SQLite は既存列に DEFAULT/CHECK を足せない) と
  foreign_keys を切っても壊れない理由。
- comments.rs MIN_ID_PREFIX_LEN: 8 が MCP の公表値であること。
- comments.rs resolve_id_prefix: 短い / ワイルドカード入りを通すと他人のコメントに解決される。
- comments.rs pending_reviews: branch NULL の旧行を worktree 列でも当てる理由。
- tests.rs 複数当たっても決定的に1件を返す: 降順挿入の理由。

足りない依存: なし (rusqlite / uuid / anyhow / log / tempfile は既に Cargo.toml にある)。
chrono は stats 廃止で review_store からは不要になった。

検証: cargo test -p conductor-core review_store:: 24 本 ok、clippy -D warnings ok、fmt ok。
(git_engine/tests.rs を別エージェントが編集中でクレート全体のテストビルドが一時的に割れる
ことがある。review_store 単体は通っている。)
