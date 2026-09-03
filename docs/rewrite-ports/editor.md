# editor (旧 src/terminal/editor.rs)

旧テスト 7 本 → 新テスト 3 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| エディタのコマンドは優先順で選び素朴に分割する | 移植 | エディタのコマンドは優先順で選び素朴に分割する (`panels/terminal/mod.rs`。9 通りに `"  vim  "` を足して 10 通り) |
| エディタの中身の大きさは枠を引く | 移植 | エディタの内容領域は枠だけを引く (`panels/terminal/mod.rs`) |
| 中身の大きさが0になることはない | 移植 | 同上 |
| 領域が0なら既定の大きさになる | 削除 | 大きさはレイアウトが決まる前に `DEFAULT_PTY_SIZE` で起動し、`sync_sizes` が次のフレームで直す。「0 の領域から寸法を計算する」経路そのものが無い |
| エディタの対象はworktree基準で相対を解決する | 移植 | eでエディタを起こし終了したらviewerへ戻って読み直す (`run.rs`。Viewer が `root.join(active_path)` を載せた `Effect::OpenInEditor` を出すことを assert) |
| ファイルが開いていなければ対象は無い | 削除 | 対象の有無を判定するのは Viewer で、開いていなければ `Effect::Status` を返すだけ。`editor_target` に当たる関数が無い |
| 空のパスなら対象は無い | 削除 | 同上。`active_path()` は空文字を返さない (タブが無ければ `None`) |

## 新設

| 新テスト名 | 何を固定するか |
|---|---|
| eでエディタを起こし終了したらviewerへ戻って読み直す | Viewer の `e` → `Effect::OpenInEditor` → PTY 起動 → `Region::Editor` が Explorer と Viewer の列を併合 → プロセス終了 → `Focus::Viewer` と `Task::LoadFile` の再発行 → 区画が消える、までを実 PTY で通す |
| 全ての区画は_hitで自分に戻る / 区画は重ならず画面を埋める | `Focus::Editor` を反復に足した (`layout.rs`) |

## API で変えた点

- 「どのエディタか」(`$VISUAL` → `$EDITOR` → `vi`) は `editor_argv()` が環境から
  読み、`TerminalPanel::open_editor` は解決済みの argv を受け取る。テストは
  プロセス全体の環境変数を書き換えずに起動経路を通せる
- 旧 `EditorPanel` が持っていた描画キャッシュは無い。PTY は描画のたびに
  vt100 から読むので、他のターミナル区画と同じ扱いになった
- `Focus::Editor` の区画は `Region::Editor` として `layout` が返す。旧は
  `ui/layout/` が Explorer と Viewer の矩形を足して描画時に合成していた
- grab された worktree でエディタを拒む分岐は移していない。新 tui には
  `is_selected_worktree_grabbed` に当たる導線がまだ無い (フェーズ 4 の grab モーダル側)
- 閉じたときの読み直しは旧と同じく本文と変更ファイル一覧の両方 (`run.rs` の `tick_editor`)
