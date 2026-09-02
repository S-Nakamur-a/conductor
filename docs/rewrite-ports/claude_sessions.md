# claude_sessions
旧テスト 15 本 → 新テスト 14 本 (移植 5 / 削除 10 / 新規 9)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| session_idが兄弟の中からログを選ぶ | 移植 | 同名 |
| 止まっているセッションは新しい兄弟に追い出されない | 移植 | 同名 (subagents/ の配置ごと) |
| 知らないsession_idは何にも解決しない | 移植 | 解決できないidは兄弟にフォールバックしない |
| 空のプロジェクトディレクトリは何にも解決しない | 移植 | 同上 |
| clearによるローテーションを追える | 削除 | 機能廃止 (rotation.rs、計画書 3 章) |
| clearを繰り返しても最新のログまで辿れる | 削除 | 機能廃止 |
| 出力がまだ無いclearでも解決する | 削除 | 機能廃止 |
| 新しく始めた兄弟セッションは横取りしない | 削除 | 機能廃止。「兄弟に横取りされない」は上の 2 本が固定している |
| 他のパネルがpinしているログは追わない | 削除 | 機能廃止 |
| 起動より前のclearは追わない | 削除 | 機能廃止 |
| 最終ターンからかけ離れたclearは追わない | 削除 | 機能廃止 |
| ローテーションしていなければ自分自身に解決する | 削除 | 機能廃止。id が自分のログに解決することは session_idが兄弟の中からログを選ぶ が固定 |
| pinしたログが無ければ自分自身に解決する | 削除 | 機能廃止 |
| ローテーション後はclear以降のターンだけが出る | 削除 | 機能廃止 |
| symlinkされたセッションログも解決する | 移植 | 同名 |

新規 (旧は ~/.claude 直参照でテスト不能だった discovery / migrate を、ClaudeHome::at(tempdir) で固定):
- working_dirは解決後のパスで探し消えたワークツリーは生のパスで探す
- プロジェクトパスのエンコード (/ と . が -)
- 経過時間の表記
- resume一覧は新しい順で重複とログの消えたものを除く (history.jsonl の形は fixture。順序はファイルの逆順で timestamp では並べない、という旧挙動もここで固定)
- ワークツリーごとの最新セッションはログの消えたものに隠されない
- historyが無ければ空
- 移行はログとサブエージェントをリンクしhistoryに追記する
- 移行元にログが無ければ何もしない
- 戻すときリンクは外すだけ
- 戻すとき実体に置き換わったものは移行元へコピーする

API 変更:
- 公開面を `ClaudeHome` (= ~/.claude) のメソッドにした。`ClaudeHome::detect()` が旧の `dirs::home_dir()` 5 箇所の重複、`ClaudeHome::at(root)` がテストの入口。
  - `load_resumable_sessions(filter)` / `find_latest_sessions_for_paths(paths)` / `migrate_session(..)` / `unmigrate_session(..)` はシグネチャそのまま、self が付いただけ
  - `current_session_log(working_dir, pinned, spawned_at, claimed)` → `session_log(working_dir, session_id)`。spawned_at / claimed は rotation のためだけの引数だったので消えた
  - `projects_dir_for(working_dir)` を公開に (svc/tui が cwd からログの場所を引くため)
- `session_log_in_dir` は非公開に (利用側はテストのみだった)
- `rotation.rs` は持ち込まない。フックが正規経路
- discovery の `log::info!` 群 (grab デバッグ時の痕跡) は debug 1 行に。`unmigrate` の jsonl / ディレクトリで重複していた「リンクなら外す、実体なら戻す」を `take_back` 1 関数に

残したコメント (なぜ):
- session_log: canonicalize してから生パスも試す理由 (Claude Code は解決後の cwd でディレクトリ名を作る / 消えたワークツリー)
- session_log_in_dir: 兄弟の .jsonl を mtime で選ばない理由
- find_latest_sessions_for_paths: ログの消えたエントリを先に除く理由 (古い有効なセッションを隠さない)
- unmigrate_session: Claude Code が rename で書くのでリンクが実体に置き換わる
