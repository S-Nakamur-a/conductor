# git modals (switch-branch / grab / prune / cherry-pick / pull / merge / reset / PR / publish)

旧テスト 17 本 → 新テスト 25 本

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
| システムプロンプトはツールを禁じjsonを求める (worktree_smart.rs) | 保留 | Smart Worktree は未移植 (下記) |
| 素のjsonを読める (worktree_smart.rs) | 保留 | 同上 |
| コードフェンスの中のjsonを読める (worktree_smart.rs) | 保留 | 同上 |
| 地の文に包まれたjsonを読める (worktree_smart.rs) | 保留 | 同上 |
| 前置きの後ろのjsonを読める (worktree_smart.rs) | 保留 | 同上 |
| jsonが1つも無い応答はエラー (worktree_smart.rs) | 保留 | 同上 |

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

## 未移植 (保留)

**Smart Worktree** (`src/worktree/worktree_smart.rs`, 313 行 + テスト 6 本)。
自由記述からブランチ名・プロンプト・セッション名を LLM に作らせ、worktree を作って
その prompt で Claude Code を自動起動する機能。`CommandId` を持たず (New Worktree の
入力欄で Tab を押すと入る隠れた第 2 モード)、`NOT_YET` にも載っていないので、
このフェーズの 11 コマンドのどれでもない。移植には `ai_caller` の配線が要るので、
revidere の解析ワーカーと同じフェーズで扱うのが筋。テスト 6 本はそのとき一緒に移す。

**worktree 作成のベースブランチ選択** (`load_base_branches` / `filtered_base_branches`)。
New Worktree の 2 段目 (ブランチ名 → ベース選択)。フェーズ 2 の「worktree 作成
(通常 / base 指定)」の担当で、今の `CreateWorktree` は `resolve_base_ref` の既定を使う。
