# revidere view (フェーズ 6b)

旧テスト 15 本 → 新テスト 20 本 (revidere/tests.rs 12 + revidere/render.rs 5 + task.rs 2 + layout.rs 1)
旧: `src/revidere/{mod,run,render}.rs`
新: `crates/conductor-tui/src/panels/revidere/{mod,artifact,render,tests}.rs`,
`crates/conductor-tui/src/modal/revidere.rs`, `crates/conductor-tui/src/task.rs`

`src/app/revidere.rs` と `src/revidere.rs` は存在しない。指示にあったその 2 つの中身は
`src/revidere/run.rs` (解析の駆動) と `src/revidere/mod.rs` (読み込み) にある。
`src/ui/dashboard/review_confirm.rs` と `src/revidere/{state,input}.rs` にテストは無い。

## 振り分け

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| mod: 成果物が無いのはエラーではない | 移植 | 同名 |
| mod: 壊れた成果物は理由を返す | 移植 | 同名 |
| mod: 前の回の差分の成果物は無いものとして扱う | 移植 | 前回からの差分は起点が今の前回と一致するものだけ残す (テーブル) |
| mod: この回の差分の成果物は残す | 移植 | 同上 |
| mod: 成果物からレビュー対象のコミットを読む | 移植 | 確認の文言は成果物と今のコミットの関係で決まる |
| mod: 成果物より後のコミットで古くなる | 削除 | `artifact_state` (mtime 判定) ごと落とした。読んだ成果物の head と worktree の head_oid を突き合わせる `RevidereState::artifact` に一本化したので、mtime を見る経路が無い。後継の事実は上の行が固定する |
| mod: 成果物が無ければnone | 移植 | 同上 (`Artifact::None`) |
| run: 別のブランチは並行して解析できる | 移植 | 解析はブランチ単位で数え全停止で全部に止まれと伝える |
| run: 終わった解析はブランチの枠を解放する | 移植 | 同上 |
| run: 死んだワーカーもブランチの枠を解放する | 削除 | 枠の解放は受信の成否ではなく `TaskResult::Analyzed` の到着で起きる。ワーカーは `catch_unwind` で必ず結果を返すので、「結果を送らずに死ぬ」状態が作れない |
| run: 全停止はすべてのワーカーに止まれと伝える | 移植 | 上のテーブルに畳んだ |
| render: 折り返しは表示幅で割り文字を1つも落とさない | 移植 | 同名 |
| render: 混在テキストでも幅を超えない | 移植 | 同名 |
| render: 幅0なら本文をそのまま返す | 移植 | 同名 |
| render: 段落の間の空行は残す | 移植 | 同名 |

## 新設 (旧に対応するテストが無かったもの)

- 説明もれがあっても成果物は読める — coverage 不完全でも `Loaded` になる
- 左列の枠題が説明もれの件数を言う
- 解析の確認から2列ビューまで通る (w → 確認 → y → `Task::Analyze` → 結果 → `Focus::Revidere`)
- 左列で選び右列へ渡って_viewer_へ出す (l/h/enter と `Effect::OpenChangedFile`)
- 区間の切り替えは読み直しを頼む
- 同じコミットの作り直しは貯めた応答を捨てる (確認の文言と `force` が同じ値から出る)
- 呼び先の見分けは_provider_と叩く先を含む (`Ai::identity` が定数化していない)
- 解析完了はフォーカスを奪わない
- 末尾のn文字は文字境界を壊さない
- タブは表示幅で埋める
- レビューは_2_列で_main_を占め概要では_1_列になる (layout)
- 既存の `全ての区画は_hitで自分に戻る` / `区画は重ならず画面を埋める` に `Focus::Revidere` を追加

## 削除 (旧の仕組みごと)

- `未実装のコマンドは理由付きで灰色になる` (`command/tests.rs`) — `NOT_YET` の表・`not_yet`
  関数・`enabled` の先頭分岐ごと消した。3 つのコマンドが実装されたので固定する対象が無い

## API と挙動の差

- **確認の鮮度判定は前方一致にした。** 成果物が書く head は短縮 oid、`WorktreeInfo::head_oid`
  は完全な oid なので、旧の `Some(&analysed) == head.as_ref()` は決して真にならなかった。
  `RevidereArtifact::Current` が到達不能で、「同じコミットの作り直しでキャッシュを捨てる」が
  一度も働いていない。`head.starts_with(analysed)` に直した
- **成果物の読み直しは worktree 切替と、解析の完了と、`w` のときだけ。** 旧は
  `refresh_reviews` から毎回呼ばれるので `(パス, mtime)` の門が要ったが、その経路が無くなった
- **右列の diff は Viewer の `unified_line` を通す。** syntect のブロック単位ハイライトは
  落ちる (新しい Viewer の diff 自体がハイライトを持たないため)。持ち物の帯 `▌` は残る
- **項目から Viewer へ出す口は `Effect::OpenChangedFile`。** 旧 `jump_to_selected_section` が
  App 越しに 8 つのパネルを触っていた処理を、Explorer の `open_changed` 1 つに寄せた
- **`CommandId::ForceAnalyzeRevidere` は確認を通さない。** 旧と同じで、キーマップのヘルプが
  "Re-analyse without asking" と名乗っている
- **解析の完了でフォーカスは動かない** (旧の設計を維持)。終わるのは数分後で、その頃には
  端末で打鍵している。知らせるのはステータスだけで、読み直すのは今見ているブランチの解析が
  終わったときだけ。不変条件は `解析完了はフォーカスを奪わない` が固定する
- **落としたもの**: Changed files パネル右上の状態チップ (`ArtifactState` と
  `cmd_revidere_badge_click`)、worktree ストリップの解析状態表示。どちらも指示の範囲外で、
  毎フレーム `metadata()` を引く作りだった

## 語彙の追加

- `Task::LoadRevidere` / `Task::Analyze`、`TaskResult::RevidereLoaded` / `TaskResult::Analyzed`、
  `AnalyzeOutcome`
- `Modal::RevidereConfirm`
- `Region::RevidereOrder` / `Region::RevidereDiff`
- `Effect::OpenChangedFile`
- `Panels::revidere` (`RevidereState`)
- `Workspace::worktree_path()`

## core / 他 crate に触った点

- `crates/conductor-tui/Cargo.toml` に `revidere` (と dev の `revidere-fixtures`) を足した
- `crates/conductor-core/src/keymap/default_keybinds.toml` の `[layers.revidere]` に
  `l` / `h` / `right` / `left` (列の行き来) と `w` (ビューを閉じる) を追加
- `crates/conductor-tui/src/panels/viewer/render.rs` の `unified_line` を `pub(crate)` に
- `Action::RevidererPrevSection` の綴りを `Action::ReviderePrevSection` に直した
  (`conductor-core` の宣言含め 5 箇所。キーマップの文字列 `revidere_prev_section` は不変)
