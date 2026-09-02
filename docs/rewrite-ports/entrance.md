# entrance (起動演出)

旧テスト 17 本 → 新テスト 13 本 (移植 16 / 削除 1) + tui 側に合成テスト 1 本

旧: `src/anim.rs` (3 本)、`src/app/state/entrance.rs` (6 本)、
`src/ui/common/entrance.rs` (8 本)、`src/ui/layout/render.rs` の `render_entrance`、
`src/event_loop/phases.rs` の skip と `start_if_pending`、`src/app/focus.rs` の再描画ポンプ。

新の置き場: `crates/conductor-tui/src/entrance.rs` (状態・描画・テストを 1 ファイル)。
配線は `run.rs` (時計の開始 / 入力で skip / 動いている間 dirty)、`liveness.rs`、
`render.rs` の末尾、`workspace.rs` の `Workspace.entrance`。

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| anim: 進みは開始で0終了で1になる | 移植 | `ratio` + `ease` に分かれたので、両端と飽和は 四隅は進捗のどの時点でも同じ位置にある と 完了時は一切伏せない が progress 0..=1 の全域を掃いて固定する |
| anim: 進みは単調で途中も範囲に収まる | 移植 | 同上 (0.05 刻み 21 点)。中点対称は smoothstep の恒等式で、`ease` の式そのもの |
| anim: 長さ0なら即座に完了する | 移植 | `ratio` の 0 除算ガードは残した。ENTRANCE_MS / INDEX_DONE_MS は定数なので、旧の `FOCUS_MS` 相当の可変長引数は無くなった |
| entrance: 設定で切られていれば起動演出は始まらない | 移植 | 同名 |
| entrance: 時計は最初のフレームまで動かない | 移植 | 同名 |
| entrance: 時計は一度始めたら打ち直さない | 移植 | 同名 |
| entrance: 入力で完成状態へ飛ぶ | 移植 | 同名 (`is_animating` も一緒に落ちることを追加) |
| entrance: 索引の開始は縁でだけ動く | 移植 | 同名 |
| entrance: ゆらぎは指定した幅に収まる | 移植 | 同名 (本数が PANELS と一致することを追加) |
| fx: 四隅は進捗のどの時点でも同じ位置にある | 移植 | 同名 |
| fx: 完了時は一切伏せない | 移植 | 同名 |
| fx: 開始時はパネルの内側が伏せられる | 移植 | 同名 |
| fx: 中身はずらしを増やしても枠の完成を待つ | 移植 | 同名 |
| fx: ひと呼吸の光は終端で残らない | 移植 | 同名 |
| fx: ゆらぎが広くてもパネルの順序は入れ替わらない | 移植 | 同名 |
| fx: 不定バーは辺を埋め尽くさない | 移植 | 同名 |
| fx: 索引完了は枠だけに触る | 移植 | 同名 |

新規 (旧に無かった事実):
- **起動演出中は索引の合図を重ねない** — 旧は `render_entrance` の `return` 1 行で守って
  いて、テストが無かった。合わせ方が新しい `apply` 1 本に入ったので、そこで固定した
- tui 側の合成テスト `起動演出の有無がフレームを流す理由になる` (`run.rs`) —
  `startup_animation = false` なら起動直後から `Liveness::Idle`、true なら `Active`、
  skip で `Idle` に戻る

API 変更 (旧 → 新):
- `EntranceState` (状態) と `ui::common::entrance` (描画) の 2 モジュール → `entrance.rs` 1 つ。
  片方が定数 (`ENTRANCE_MS` / `JITTER_MS` / `PANEL_STAGGER`) を借りるだけの依存が消えた
- 公開は `Entrance::{new, start_if_pending, skip, note_index_building, is_animating}` と
  `entrance::apply` の 6 つ。`boot_progress` / `offsets` / `index_bar_phase` /
  `index_done_progress` と、`apply_entrance` / `apply_index_bar` / `apply_index_done` /
  `offsets()` は非公開になった (描画の入口が `apply` 1 本になったので外から呼ぶ理由が無い)
- `src/anim.rs` は独立モジュールとしては消えた。`eased_progress(Duration, u64)` は
  `ratio` + `ease` に分かれている。旧の唯一のもう一人の利用者だったフォーカス遷移
  (`FOCUS_MS`) は書き直しに持ち込んでいないので、曲線の持ち主は演出だけになった
- `PANELS` を 4 → 5 に。新しい `layout` は Explorer を 2 区画に分けており、演出のパネルも
  `ExplorerTree / ExplorerChanges / Viewer / TerminalClaude / TerminalShell` の 5 枚

挙動差 (旧 → 新):
- **索引バー (`apply_index_bar` / `apply_index_done`) は描く側だけ配線した。**
  `note_index_building` を呼ぶ人がまだいない。フェーズ 5 で `semantic_index` を繋ぐときに
  「索引を作っているか」を毎フレーム渡せば、そのまま出番が来る
- **`Workspace::for_test()` は演出を切ってある。** 演出は画面を伏せるので、切らないと
  描画とフレームの理由を見るテストが読めない

残したコメント (なぜ):
- モジュール doc: 幅を動かすと PTY がフレームごとに resize される
- `start_if_pending`: 起動から数えると演出が画面に出る前に終わる (実測 2 秒)
- `apply` / `apply_entrance`: 光を重ねない・枠が閉じるまで中身を伏せる理由
- `frames_closed_at`: 固定値だとずらしを増やしたときに中身が先に出る
- `apply_glow` / `raise`: 前景しか触らない理由 (`Color::Reset` に `lerp` が効かない)、
  ライトテーマで白へ寄せると文字が地に溶ける
- `apply_index_bar`: 進行度が取れないので割合で描けない、埋め尽くすと残り時間に読まれる
- `offsets` / `jitter`: ゆらぎがずらしより広いと順序が入れ替わる、質のよい乱数は要らない
- `paint_edge`: 四隅が毎回同じ位置に落ちること

検証:
- `cargo test -p conductor-tui entrance::` 13 passed
- 擬似端末での実走 (`scratchpad/drive_phase4c.py`): `startup_animation = true` なら
  最初のフレーム + 0.25s の時点でまだパネルが組み上がっておらず、2.5s 後には 4 枚とも
  読める。`false` なら同じ時点で 4 枚とも出ている
