# click
旧テスト 4 本 → 新テスト 7 本 (移植 4 / 削除 0 / 新設 3)

フェーズ 7 の切替時に旧 `src/widget/click.rs` ごと落ちていて、ダブルクリックの挙動
(空の端末区画で新規セッション / 一覧の 2 回目のクリックで固定して開く) が新 UI から
消えていた。判定器そのものと、それを読む 2 箇所の配線を戻した記録。

新の置き場: `crates/conductor-tui/src/click.rs` (旧 `src/widget/click.rs` を丸ごと移設)。
読み手は `panels/terminal/mod.rs` の `Pane::clicks` と
`panels/explorer/mod.rs` の `tree_clicks` / `changes_clicks`。

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 最初のクリックはダブルにならない | 移植 | 同名 |
| 同じ場所を続けて2回押すとダブルになる | 移植 | 同名 |
| 別の場所を押すと数え直しになる | 移植 | 同名 |
| 遅い2回目はダブルにならない | 移植 | 同名 |
| (新設) | 新設 | 空の区画は2回目のクリックでセッションを起こす (terminal)。旧 `src/terminal/mouse.rs` の分岐にテストが無かった |
| (新設) | 新設 | ツリーの2回目のクリックは固定して開く (explorer)。旧 `src/explorer/pointer.rs` の `open_as` にテストが無かった |
| (新設) | 新設 | 変更一覧の2回目のクリックは固定して開く (explorer)。同上 |

API 変更:
- `ClickTracker` / `is_double(index) -> bool` は旧と同形。`Debug` を derive に足した
  (`ExplorerPanel` が `Debug` を derive しているため)
- 旧の `OpenAs { Preview, Persistent }` は無くなっているので、`Effect::OpenFile` の
  `preview: bool` を直接組む。ヘルパ (旧 `open_as`) は 1 行なので置かない
- 端末側は旧の `current_worktree_sessions(kind).is_empty()` ではなく `Pane::session.is_none()` で
  空を判定する。`follow_worktree` が worktree 切替のたびにその worktree のセッションを
  入れ直すので同値で、パネル自身の状態だけで閉じる
- `ExplorerPanel::activate_change` / `open_diff` に `preview: bool` を足した。キーボードの
  Enter と `step_changed_file` は従来どおり固定 (`false`) で開く

残したコメント (なぜ):
- `click.rs` `is_double`: 位置も条件にする理由 (時間だけだと別の行の連打が 2 回目になる)
- `terminal/mod.rs` `click`: 1 クリックで起こさない理由 (フォーカスを移すだけのクリックが
  プロセスを増やす)
- `explorer/mod.rs` フィールド: ツリーと変更一覧でトラッカーを分ける理由
  (1 つだと両方を 1 回ずつ押しただけで 2 回目になる)
- `explorer/mod.rs` `click`: 2 回目で固定する理由 (preview のタブは 1 枚しか残らない)

旧の doc から落としたもの: 分割前は判定用の時刻と添字が `viewer.click` にあった、という
経緯の 2 行 (旧 `src/` はもう無い)。

検証: `cargo test -p conductor-tui` 474 本 pass / `cargo clippy --workspace` clean / `cargo fmt --all` 済み。
