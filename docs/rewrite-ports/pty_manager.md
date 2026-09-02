# pty_manager → conductor-svc/src/pty (+ conductor-core/src/cc_hook/settings.rs)

旧テスト 30 本 → 新テスト 17 本 (svc 13 / core 4)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 矢印の列はdecckmと向きに従う | 移植 | `矢印の列はdecckmと向きに従う` (テーブル化) |
| サニタイズは素のテキストとタブと改行を残す | 統合 | `貼り付けは本文だけ残す` |
| crlfと単独のcrはlfに正規化する | 統合 | 同上 |
| csiのエスケープ列を取り除く | 統合 | 同上 |
| oscの列を取り除く | 統合 | 同上 |
| 裸の制御バイトは落とし本文は残す | 統合 | 同上 |
| マルチバイトの本文は壊さない | 統合 | 同上 |
| 紛れ込んだ貼り付け終了マーカーを取り除く | 移植 | `紛れ込んだ貼り付け終了マーカーを取り除く` (ESC 不在の追加 assert があるので単独維持) |
| ホイールのsgr符号化は上下を書き分ける | 統合 | `ホイールの符号化は方式と向きに従う` |
| x10の符号化は32だけずらす | 統合 | 同上 |
| 座標は最低1にクランプする | 統合 | 同上 |
| 履歴は上限以下かつ行境界に揃える | 移植 | `履歴は上限以下かつ行境界に揃える` |
| 上限以下のバッファには触らない | 移植 | `上限以下の履歴には触らない` |
| 改行が無くても中身は空にしない | 移植 | `改行が無くても中身は空にしない` |
| 再生は新しい幅で折り返し直す | 移植 | `再生は新しい幅で折り返し直す` (vt100 の set_size が非リフローである事実も維持) |
| 空の履歴でもパーサを組み直せる | 移植 | `空の履歴でもパーサを組み直せる` |
| 代替画面の往復も組み直せる | 移植 | `代替画面の往復も組み直せる` |
| チャンク分割はマルチバイト文字を割らない | 統合 | `分割はマルチバイト文字を割らない` |
| 混在テキストもasciiも保たれる | 統合 | 同上 |
| 上限より大きい文字も扱える | 統合 | 同上 (`("あ", 1)` の行として残る) |
| 空の入力ではチャンクが出ない | 統合 | 同上 (`("", 1024)` の行として残る) |
| ロケール未設定ならutf8を強制する | 統合 | `ロケール判定はposixの優先順位に従う` |
| 空のロケール値は未設定として扱う | 統合 | 同上 |
| 既にutf8なら尊重する | 統合 | 同上 (3 綴りとも行として残る) |
| 判定ではlc_allが優先される | 統合 | 同上 |
| utf8でないlc_allは落としてlc_ctypeを通す | 統合 | 同上 |
| lc_allが無ければlangが非utf8でもlc_allに触らない | 移植 | `無いlc_allは削除対象にしない` (removes が空という別の事実なので単独) |
| 設定にsession_startフックが宣言される | 移植 (core) | `cc_hook::settings::設定にsession_startフックが宣言される` |
| フック設定は起動のたびに書き直される | 移植 (core) | `cc_hook::settings::フック設定は起動のたびに書き直される` |
| 扱いにくい実行パスでもフックのコマンドは壊れない | 移植 (core) | `cc_hook::settings::扱いにくい実行パスでもフックのコマンドは1語のまま` |
| (新規) | 追加 | `cc_hook::settings::ソケットは設定と同じディレクトリに置く` — フックへ渡す側と bind 側の綴りを固定 |

## API 変更

- `PtyManager` → `PtyStore` (計画書 2.4 の呼称)。
- `spawn_session(10 引数)` + `spawn_editor_session(8 引数)` → `spawn(Spawn { launch: Launch::{ClaudeCode,Shell,Editor}, worktree, label, working_dir, rows, cols })`。
  種類ごとにしか使わない引数 (shell_path / resume_session_id / repo_root / session_name / program / args / file) が enum の枝に入るので、
  `#[allow(clippy::too_many_arguments)]` ×3 と `unreachable!("editor sessions are spawned via spawn_editor_session")` が消えた。
- `get_screen` → `screen` / `get_output` → `output` / `session_has_visible_output` → `has_visible_output` /
  `session_application_cursor` → `application_cursor` (Rust の命名慣習)。
- `session_count()` 削除 — `sessions().len()` と同義。
- `PtySession.last_output_time: Arc<Mutex<Instant>>` (pub フィールド) → `PtySession::last_output() -> Instant`。
  利用側は 4 箇所とも `*x.lock()` して Instant を取るだけだった。
- `PtySession.is_active` / `.max_buffer_lines` 削除 — 書くだけで誰も読んでいなかった。
  `activate_session` の実効果は共有バッファ上限の引き上げだけなので、それだけを行う (`&self` で足りる)。
- `PtyManager.buffer_limits: Vec<Arc<Mutex<usize>>>` (sessions と添字で対応する並行配列) 廃止。
  reader スレッドと共有する 7 個の Arc を `SharedIo` 1 個にまとめ、セッションが直接持つ。`remove_session` の同期ずれの余地が消えた。
- `PtySession.claude_session_id` / `.spawned_at` を private 化 (`claude_session_ref` / `set_claude_session_id` 経由)。
- `write_to_session` / `write_chunked_to_session` / `write_paste_to_session` / `forward_scroll_to_session` は `&self` に
  (内部可変性なので `&mut` は不要だった)。
- `reader_thread`(9 引数の関連関数) → `reader::run(reader, SharedIo, writer)`。
- **core 側に追加**: `conductor_core::cc_hook::{install_settings, socket_path}` (新ファイル `cc_hook/settings.rs`)。
  旧 `PtyManager::write_hook_settings` / `shell_quote` と旧 `cc_notify::socket_path` の移設先。
  理由は 2 つ: (1) svc に `serde_json` が無く Cargo.toml は触れない、(2) フックの契約 (env 変数名・電文・settings・ソケットパス) が
  1 モジュールに揃い、仕掛ける側 (svc/pty) と受ける側 (svc/watch) が同じ綴りを参照できる。
- `.conductor/` の解決を `repo_root.join(".conductor")` から `git_engine::conductor_dir(repo_root)` に統一。
  DB・settings・ソケットの 3 つが linked worktree 起動時に別々の場所を指す余地を消した (`conductor_dir` の doc が言う規約どおり)。

## 残したコメント (なぜ)

- `pty/mod.rs` モジュール doc: PTY だけ Event 経路に乗せない理由 (バイト列の頻度)。
- `MAX_RAW_HISTORY_BYTES` 512 KiB: 再生が resize の中を同期で走るので、最悪 1 フレームに収まる量。
- `lock()`: 毒された Mutex を無視する理由。
- `SharedIo.raw_history`: その場描画型のセッションで再生してもリフローされない (メモリと CPU の無駄) こと。
- `reader::run`: writer を渡す理由 (CPR に即答しないと fzf/シェルが止まる)。
- `trim_raw_history`: 改行探索に上限がある理由と、空にしない方がましである理由。
- `screen::resize_session`: vt100 の `set_size` がリフローしないこと。
- `nudge_alt_screen_sessions`: fzf が SIGWINCH を待つこと、macOS がサイズ変化時にしか配送しないこと。
- `application_cursor`: DECCKM を尊重しないと矢印が効かないこと。
- `is_waiting_for_input`: Claude Code がアイドルを名乗らないので画面から読むしかないこと。
- `io::write_chunks` / `locale::utf8_chunks`: 固定オフセット分割がマルチバイトを割ること。
- `io::write_paste_to_session`: bracketed paste マーカーを条件付きにする理由。
- `io::sanitize_pasted_text`: `\x1b[201~` の混入で貼り付けが早期終了しコマンドが実行されること。
- `locale::utf8_locale_overrides`: vim が非 UTF-8 ロケールで latin1 に落ちること、C.UTF-8 を選ぶ理由。
- `spawn`: panel_id を先に決める理由、`--session-id` を強制する理由、`--settings` がレイヤーを足すこと、
  フック設定が書けなくてもパネルを動かすこと。
- `cc_hook/settings.rs`: フックを別リリースチャネルに置くと黙って壊れること、ソケットをメイン worktree 側に置くこと。

## この移植の範囲外だったもの

- **Claude 出力のスキャナ** は旧 pty_manager に無い。`get_output()` を読む側 (`terminal/terminal_cc_state.rs`) の責務。
- **vt100 0.15.2 の scrollback debug panic** に触れるコードも旧 pty_manager に無い。`set_scrollback` の呼び出しは
  全て terminal パネル側 (`terminal/{mouse,input,render/pty,resize}.rs`)。回避コードはそちらの移植で扱うこと。

## 検証

`cargo test -p conductor-svc pty::` 13 passed / `cargo clippy -p conductor-svc --all-targets -- -D warnings` 無警告 /
`cargo fmt -p conductor-svc -- --check` clean / `cargo test -p conductor-core cc_hook::` 8 passed。

`crates/conductor-core` の clippy は `semantic_index/tests.rs:233` の `type_complexity` で落ちるが、これは別移植の作業中コード。
