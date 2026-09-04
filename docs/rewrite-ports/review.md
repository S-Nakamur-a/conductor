# review (フェーズ 3b)

旧テスト 19 本 (下表) → 新テスト 41 本 (移植 15 / 削除 4)

新の置き場: `crates/conductor-tui/src/{review,comment_list,modal}.rs`、
`crates/conductor-tui/src/panels/viewer/{thread,tabs,mod,render}.rs`、
`crates/conductor-tui/src/panels/explorer/mod.rs`、
`crates/conductor-core/src/review_store/viewed.rs`。

旧のレビュー機能はテストがほとんど無い (`app/review*.rs`, `ui/review.rs`,
`viewer/comment_actions.rs`, `viewer/render/comment_thread.rs`,
`viewer/input/inline_reply.rs`, `viewer/render/summary_view.rs`,
`explorer/render/comments.rs` は合わせて 0 本)。仕様はコードそのものから拾い、
新規テストで固定した。

## review_state.rs

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| バッジのために解決済みのコメントも残す | 移植 | 未解決は既定で開き解決済みは閉じspaceが両方を裏返す (`thread.rs`。解決済みも印は出続け、閉じるのは展開だけ) |
| 重なった範囲も扱える | 移植 | 重なった範囲は共有行で両方に当たり終端は自分だけを持つ (`review.rs`) |

## viewer/input/diff_nav.rs (viewer.md で 3b へ先送りしていた 3 本)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| コメント間の移動 | 移植 | コメント間の移動は今の行を飛ばして両端で止まる |
| 削除行にはコメントが付かない | 移植 | 削除行ではコメントを始められない (旧はナビゲーションが削除行を素通りする形で示していた。新は `comment_line` が None を返し、作成そのものを断る) |
| 畳みに隠れたコメントにも辿り着ける | 移植 | コメント間の移動は今の行を飛ばして両端で止まる (`goto_line` が `fold.reveal` を通るので、畳みに隠れた行でも着地する) |

## event/mouse/tests.rs (コメントと畳みの当たり判定)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| ガターとバッジのクリックは必ずコメントを始める | 移植 | ガターを押すとコメントの作成欄が開く / コメントのある行の印はスレッドを開閉し行番号の桁は作成を始める / 開いたスレッドの下の行を押すとその行が対象になる / 差分表示でもガターから作成でき削除行では断る (`viewer/mod.rs`。一度は削除したが、画面のどこにも作成の入口が見えず「機能が消えた」と受け取られた。作成の実体は `start_comment` 1 つで、キーの `c` と共有) |
| マーカーのクリックは既存スレッドへ寄せる | 移植 | ガターの桁は印と畳みと本文で意味が分かれる |
| テスト行のバッジを押すとテストが走る | 削除 | テスト実行はフェーズ 5 |
| 範囲の途中は最も近い終了行へ振り替える | 移植 | 重なった範囲は共有行で両方に当たり終端は自分だけを持つ (`anchor_for`。旧は doc が「最も近い」と言い実装は最小値だった。新は実装どおり「最も早く終わるもの」を doc にした) |
| 畳みマーカーは行番号の右のガターを取る | 移植 | ガターの桁は印と畳みと本文で意味が分かれる |

## viewer.md で 3b へ先送りしていたタブ帯の 5 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| tab_row: 見えているタブは選べて閉じられる | 移植 | 見えているタブは押した位置のものが選ばれる (閉じるボタンは置かなかった。`x` があり、帯に ✕ を並べると 1 枚あたり 2 桁増える) |
| tab_row: 溢れてもアクティブなタブは見える位置に残る | 移植 | 窓はアクティブが外に出たときだけ寄せ直す |
| tab_row: 溢れの印を押すと隠れたタブに届く | 移植 | 溢れると印が出て押した向きへ1枚ずつ送る |
| tab_row: スクロールがアクティブなタブに巻き戻されない | 移植 | 窓はアクティブが外に出たときだけ寄せ直す |
| tab_row: 溢れは左右どちらにも印が出る | 移植 | 溢れると印が出て押した向きへ1枚ずつ送る |

## viewer.md で 3b へ先送りしていた折りたたみ hover の 3 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| マーカーへのhoverは範囲全体に印を付ける | 削除 | 新はマウス移動を追わない (`run.rs` が受けるのは押下とホイールだけ)。hover の状態そのものが無い |
| 見出し以外の行へのhoverは何も出さない | 削除 | 同上 |
| 読み直すとhoverは消える | 削除 | 同上 |

## refresh_pipe.rs (本数は watchers.md 側で数えている)

7 本すべて `docs/rewrite-ports/watchers.md` で処理済み (読み側は
`conductor-svc/src/watch/refresh_pipe.rs`)。3b では受け取った
`WatchEvent::RefreshRequested` の行き先だけを足した。

| 新テスト名 | 何を固定するか |
|---|---|
| watchの行き先は種類で分かれる | MCP の合図は `Task::LoadReview`、他はターミナルへ |

## explorer/keys.rs

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| コメント一覧はdiffのエラーを無視する | 移植 | cとdで下区画の中身が入れ替わりキーの層も変わる (旧はバナーのぶんだけ窓がずれないことを見ていた。新はコメント一覧が `banner_rows` を通らない別の Viewport を持つので、同じ事実が「中身ごとに窓が違う」に化けた) |

## 新規 (旧に無かった事実)

`review.rs`: 別のファイルのコメントは混ざらない / 読み込みの失敗は手元のコメントを消さない。
`thread.rs`: 未解決は既定で開き解決済みは閉じspaceが両方を裏返す / ガターの印は終端行と範囲の途中を見分ける / スレッドは本文と返信を終端行の下にまとめる。
`comment_list.rs`: 返信のあるコメントだけが開き畳むと親へ寄る / 一覧の操作はコメントを名指しした効果になる / コメントの削除は返信の巻き添えを問う / 解決の切り替えは今の状態を裏返す / 行は場所と返信数を添え解決済みは後退する / 本文は1行目だけを出し残りの行数を添える / コメントが無ければその旨を1行出す。
`modal.rs`: 空の本文は保存せず理由を出す / 改行はshift_enterで本文に入る / tabは新規のときだけ種別を入れ替える / 編集は今の本文から始まり同じidへ書き戻す / escは何も書かずに閉じる / 一覧モーダルからの移動は閉じてviewerへ渡す。
`viewer/mod.rs`: cは選択の範囲をそのままコメントのアンカーにする / 返信と解決はカーソル行のコメントに効きコメントが無ければ知らせる。
`viewer/render.rs`: コメントのある行だけが印の桁を開く / 開いたスレッドは行の直後に割り込み窓の高さを食う / 変更サマリは差分のときだけ本文の上に出る。
`viewer/tabs.rs`: 長いパスは先頭を省いてファイル名を残す。
`explorer/mod.rs`: viewedは選択中のファイルの印を反転させる。
`run.rs`: 書いたコメントはスレッドと一覧に出て起動し直しても残る (route → Effect → svc → 実 DB の合成テスト)。
`conductor-core`: 返信はworktreeごとにまとめて引ける / viewedの印はブランチごとに残る / v9は統計テーブルを落としv10はviewedを足す。

## 語彙の追加

- `Effect`: **追加なし**。書き込みは全部 `Effect::Spawn(Task::WriteReview(..))` で、
  スレッドの開閉のようにパネル内で閉じるものは `Action` のまま。既存の
  `Effect::ToggleViewed` の意味だけが変わった (Explorer のメモリを触る → 反転を
  DB へ投げる)
- `Task`: `LoadReview` / `WriteReview(ReviewWrite)`。`ReviewWrite` は
  AddComment / EditComment / DeleteComment / SetStatus / AddReply / EditReply /
  DeleteReply / SetViewed の 8 種。**書き込みは必ず読み直して返す**ので、結果は
  どれも 1 つ
- `TaskResult`: `Review(Result<Box<Snapshot>, String>)` の 1 つだけ
- `Modal`: `CommentEditor` (新規・編集・返信・返信の編集を 1 つの入力欄で兼ねる) と
  `CommentList` (全画面。`C` で開き、Explorer の下区画と同じ
  `comment_list::CommentList` を持つ)
- `TaskEnv.branch`: レビューの行はブランチで引くので、Task が毎回運ぶ代わりに環境に置いた

## core / svc に触った点

- `review_store`: `user_version = 10` で `viewed_files (branch, file_path)` を追加。
  `set_viewed` / `viewed_files` (`viewed.rs`) と `replies_for_worktree` (`replies.rs`)
- `keymap/default_keybinds.toml`: viewer / viewer_diff_mode に
  `R = reply_to_comment`, `r = toggle_resolve` (旧はインラインの返信・解決が
  マウス専用で、キーボードだけでは一覧まで戻る必要があった)。
  explorer_diff_list に `c = show_comment_list`、explorer_comment_list に
  `d = show_diff_list` (下区画の 2 つの一覧が互いに行き来できないと、
  ツリーまで戻る往復が毎回 2 手増える)
- svc: 変更なし。`RefreshPipe` は既にあり、`main.rs` から起動するようにしただけ。
  PTY の spawn は既に `CONDUCTOR_DB_PATH` を渡していた

## 旧との挙動差 (意図的)

1. **コメントの作成の実体が 1 つ。** 旧は `c` (アンカー方式) とガタークリック
   (`"file:line "` を文字列で前置きする方式) の 2 実装があり、マウスとキーボードで
   違う UI が開いた。新は入口が `c` とガタークリックの 2 つでも、開くのは同じ
   モーダル。当たり判定は描画と同じ列 (`render::rows`) から引くので、割り込む
   スレッドで押した行がずれない。shift+ドラッグの範囲指定は戻していない
2. **本文の入力は常にモーダル。** 旧は新規だけインラインの作成ボックスで、編集と
   返信はモーダルだった
3. **コメント詳細モーダルを置かない。** 旧の詳細 (本文全文 + 返信 + スクロール) は
   Viewer のインラインスレッドが同じものを出す。一覧は索引に徹し、`space` は
   返信の開閉、`enter` は位置へのジャンプにした。描画が `max_scroll` を状態へ
   書き戻す旧の作りもここで消えた
4. **返信は最初から全部読む。** 旧は件数だけ先に読んで本文は展開時に遅延取得し、
   キャッシュ 2 枚と無効化を抱えていた。1 ブランチぶんは 1 クエリで足りる
5. **変更サマリはバナー。** 旧は Viewer 全面を占める SUMMARY 疑似ファイルで、
   変更ファイル一覧にも `▣ SUMMARY` の行が生えていた。新は diff の上に 6 行まで
   出す (素のファイルを開くたびに場所を取らせない)。`DiffState::has_summary` は
   未使用のまま
6. **テンプレートを持ち込まない。** 旧は使う経路だけあって保存する経路が無く、
   `comment_templates` への INSERT がリポジトリ内に 1 箇所も無かった
7. **`viewed` は永続化する。** 旧は `HashSet` だけで doc と実装が食い違っていた
8. **`Ask Claude` ボタンを置かない。** PTY へのプロンプト投入はフェーズ 4 以降
9. **左右 2 列の diff にはコメントの印を出さない。** スレッドは出す。印の桁を
   足すと左右の幅の計算が 2 通りになる
10. **入れ子の範囲では内側が対象。** 開閉 (`space`) も返信 (`R`) も解決 (`r`) も
   `review::innermost` の 1 件を指す。旧は開閉が内側 (`min`)、それ以外は
   `file_comments` の先頭 = 外側で、押した先が読めなかった

## 残したコメント (なぜ)

- `review.rs` モジュール doc: 部分更新をせず Snapshot を丸ごと入れ替える理由 (MCP が
  同じ DB を書く)
- `review.rs` `install` の Err 側: 読めなかっただけで消えたわけではない
- `review.rs` `innermost`: 入れ子の範囲でどちらに寄せるかの選択
- `comment_list.rs` `collapse`: 返信の行から畳むと自分の行が消える
- `comment_list.rs` 解決済みの色: 明るい印がミュートな本文の上に乗る害
- `thread.rs` `ThreadFolds`: 反転だけを覚える理由 (追加・解決のたびの手当てが要らない)
- `thread.rs` `author_bg`: 署名を読まずに書き手を見分ける
- `modal.rs` `key`: enter を改行にすると確定の手段が無くなる
- `viewer/mod.rs` `comment_line`: コメントの座標が新ファイル側の行番号であること
- `viewer/render.rs` `summary_banner`: diff のときだけ出す理由
- `viewer/tabs.rs` `reveal_tab`: 送った窓を毎フレーム巻き戻さない
- `task.rs` `WriteReview`: 書いたあと必ず読み直す理由

## 実走で確認したこと

擬似端末 (`scratchpad/drive_review.py` / `drive_mcp.py`) で `conductor-next` を起動:

- 変更一覧 → Enter で diff → `c` → 本文 → Enter で `.conductor/conductor.db` に
  行が入り、行の直下にスレッドが出る。ガターは印のぶん 2 桁開く
- `space` でスレッドが畳めて開く。`R` で返信、`r` で解決 (バイラインに `✓ resolved`)
- Esc Esc で Explorer へ戻り `c` で下区画が `Comments (1/1)` に替わる
- 起動し直すと同じコメントが一覧に出る
- TUI を上げたまま `conductor-next mcp-serve` の `create_comment` を叩くと、
  refresh pipe 経由で `Comments (1/1)` → `(2/2)` に増え、Claude 面の色で
  インラインスレッドに出る。`set_change_summary` は diff の上にバナーとして出る
