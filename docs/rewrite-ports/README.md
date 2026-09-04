# 書き直し時のテスト振り分け表

旧 src/ のモジュールを crates/ へ書き直すとき、旧テスト 1 本ごとに「移植した / 削除した (理由)」を記録する。
「多い」は削除理由にならない。固定している事実を減らさないための台帳で、書き直しが終わったら消してよい。

## フェーズ 7 (切替) で消した旧テスト

旧 `src/` を丸ごと消したので、それまでの台帳に載っていなかった 2 件をここに残す。

- `src/hit_map.rs` の `ColumnSpans` 3 本 — 削除。当たり判定が純関数 `layout` + `hit` に
  変わって対象そのものが無くなった。往復は `conductor-tui/src/layout.rs` のテストが固定する
- `src/refresh_pipe.rs` の書き手側 3 本 — `conductor-mcp/src/refresh_signal.rs` へ移植
  (watchers.md の「未移植」が解消)
- `src/widget/click.rs` の `ClickTracker` 4 本 — 切替時に落ちていたのを移植し直した。click.md

Smart Worktree (`src/worktree/worktree_smart.rs`) はその後に移植した。テスト 6 本の
移植先と、落とした挙動 (生成中の Esc キャンセル) は git_modals.md にある。
