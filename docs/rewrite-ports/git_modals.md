# git modals (switch-branch / grab / prune / cherry-pick / pull / merge / reset / PR / publish)

旧テスト 17 本 → 新テスト 32 本

対象の旧ファイル: `src/worktree/{worktree_branches,worktree_grab,worktree_pr,worktree_smart,worktree_crud,worktree_commands,ops,input}.rs`、
`src/ui/dashboard/{branch_picker,worktree,review_confirm,input}.rs`、`src/overlay.rs`、
`src/app/review_publish.rs`、`src/app/state/publish.rs`、`src/event/dialogs.rs`、
`src/event/overlay/{vcs,repo}.rs`。

この 18 ファイルのうち `#[test]` を持つのは 5 ファイルだけで、この機能群の挙動は
ほぼテストされていなかった。`App` を組み立てないと 1 行も動かせない形だったのが原因で、
移植先ではモーダルの `update` が `Vec<Effect>` を返すので `Workspace` だけで押せる。
新テストの本数が増えているのはそのため。

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 開いたときは最初のファイルだけ展開する (overlay.rs) | 削除 | 参照一覧のオーバーレイはフェーズ 5 (コード知能) の担当で、この 11 コマンドに含まれない |
| 同じファイルが飛び飛びでも1つの見出しにまとまる (overlay.rs) | 削除 | 同上 |
| 畳んでも選択はその見出しに残る (overlay.rs) | 削除 | 同上 |
| 投稿中にもう一度確定しても二重に始めない (review_publish.rs) | 削除 | `should_start_publish` は `BackgroundOp` の実行中フラグを読むための関数で、その概念ごと消えた。確認モーダルは `Effect::PopModal` を必ず先頭に置くので、y の二度押しでは 2 回目のキーが届く相手がいない |
| 何も走っていなければ確定で始まる (review_publish.rs) | 削除 | 同上 (タウトロジー側) |
| 明示的な改行は行になる (dashboard/input.rs) | 移植済 | `modal/input.rs` の同名テスト (フェーズ 1 で移植済み) |
| 長い行は幅で折り返しカーソルも追う (dashboard/input.rs) | 移植済 | `modal/input.rs` の「長い行は幅で折り返す」+「カーソルはその位置の行に入る」 |
| 全角文字は境界で割れない (dashboard/input.rs) | 移植済 | `modal/input.rs` の同名テスト |
| 空の本文は1行と原点のカーソルになる (dashboard/input.rs) | 移植済 | `modal/input.rs` の「空の本文は1行になる」 |
| headと空はdiffのベースとして拒む (worktree_crud.rs) | 削除 | base ref の解決は `conductor-core::git_engine` 側の関数で、そちらの `git_engine.md` で移植済み |
| 実在するrefは受け入れる (worktree_crud.rs) | 削除 | 同上 |
| システムプロンプトはツールを禁じjsonを求める (worktree_smart.rs) | 移植済 | `conductor-core/src/smart_worktree.rs` の同名テスト |
| 素のjsonを読める (worktree_smart.rs) | 移植済 | 同上 |
| コードフェンスの中のjsonを読める (worktree_smart.rs) | 移植済 | 同上 |
| 地の文に包まれたjsonを読める (worktree_smart.rs) | 移植済 | 同上 |
| 前置きの後ろのjsonを読める (worktree_smart.rs) | 移植済 | 同上 |
| jsonが1つも無い応答はエラー (worktree_smart.rs) | 移植済 | 同上 |

## 新しく書いたテスト

旧側に対応する `#[test]` が無く、コードと確認ダイアログの文言が唯一の仕様だったもの。

`modal/branch.rs`
- リモートは手元の一覧とfetchの両方を頼む
- 選んだリモートブランチから_worktreeを作る
- 一覧が入れ替わっても選んでいたブランチを追いかける (旧 `poll_bg_branches` の選択復元)
- grabは選んだブランチの元のworktreeを渡す
- escは何もせず閉じる

`modal/commits.rs`
- 候補が無ければ開かない
- 選んだコミットを今のworktreeへ積む
- tabは取り出し元を回して読み直す
- コミットが無ければ何も積まない

`modal/pr.rs`
- 空の入力では何も始めない
- 失敗しても入力は残り編集で理由が消える (旧 `PrInputOverlay::error` の仕様)
- 走っている間もescで閉じられる

`modal/publish.rs`
- 何が飛ぶかと落とす件数を出す
- nは何も飛ばさず閉じる
- yで投稿のタスクが飛ぶ
- 関係ないキーでは閉じない

`command/tests.rs`
- git系コマンドは状態で灰色になる (enabled の述語 6 本をテーブルで)

`run.rs` (合成。一時 git リポジトリを実際に触る)
- メニューからのmergeは確認を通ってからタスクになる
- git操作の結果は文言と一覧の取り直しになる
- pr取り込みに失敗しても入力は残る
- pr取り込みが成功すると閉じてその_worktreeへ移る
- リモートブランチを選ぶと_worktreeができる
- prune_は消えたworktreeを数えて確認してから消す
- cherry_pickは選んだコミットを今のworktreeへ積む

## Smart Worktree (移植済)

旧 `src/worktree/worktree_smart.rs` の移植先。New Worktree の入力欄で Tab を押すと入る
第 2 の面のまま (`CommandId` は持たない)。

- AI に訊いて Plan にするところまで: `conductor-core/src/smart_worktree.rs` (テスト 6 本もここ)
- worktree の作成: `conductor-tui/src/task.rs` の `Task::SmartWorktree` / `TaskResult::SmartWorktreeCreated`
- 入力欄の 2 面: `modal/mod.rs` の `Prompt::alternate` (テスト「tabは宛先を入れ替え打ちかけの本文を残す」)
- 作成後の Claude 起動とプロンプト投入: `Effect::SmartSession` →
  `panels/terminal/mod.rs` の `launch_claude_at` / `flush_deferred`

**生成中の Esc によるキャンセルは落とした。** 旧は保留中の Esc を大域で横取りしていたが、
新の `route` には自由な Esc の持ち主がおらず、作るとルーティングの面が増える。生成は
数秒で `[api] command_timeout_secs` に頭打ちされるので実害が小さいと判断した。
途中経過ステータス (旧 `SmartBranchResolved`) も落とした — Task は結果を 1 度返す形で、
spawn 時の "Smart worktree: generating…" で足りる。

## 未移植 (保留)

**worktree 作成のベースブランチ選択** (`load_base_branches` / `filtered_base_branches`)。
New Worktree の 2 段目 (ブランチ名 → ベース選択)。フェーズ 2 の「worktree 作成
(通常 / base 指定)」の担当で、今の `CreateWorktree` は `resolve_base_ref` の既定を使う。
