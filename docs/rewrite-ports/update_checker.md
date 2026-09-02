# update_checker

旧テスト 14 本 → 新テスト 7 本 (移植 13 / 削除 1) + tui 側に合成テスト 3 本

旧: `src/update_checker.rs` (照合)、`src/app/update.rs` (確認 → DL → 差し替え → 再起動)、
`src/app/state/update_flow.rs`、`src/ui/dashboard/update.rs`、`src/ui/chrome/title_bar.rs`
のバッジ、`src/event_loop/state.rs` の起動時チェック、`src/startup.rs` の exec。

新の置き場:
- `crates/conductor-core/src/update_checker/{mod,install,tests}.rs`
- `crates/conductor-tui/src/modal/update.rs` (確認 → 進捗 → 失敗)
- `crates/conductor-tui/src/task.rs` (`Task::CheckForUpdate` / `DownloadUpdate`)
- `crates/conductor-tui/src/render.rs` (`title_line` のバッジ)
- `crates/conductor-tui/src/main.rs` (`relaunch`)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| メジャーが新しい | 移植 | バージョンの新しさを比べる (表 1 行) |
| マイナーが新しい | 移植 | 同上 |
| パッチが新しい | 移植 | 同上 |
| 同じバージョン | 移植 | 同上 |
| 古いバージョン | 移植 | 同上 |
| latestが不正 | 移植 | 同上 |
| currentが不正 | 移植 | 同上 |
| バージョンが2要素だけ | 移植 | 同上。4 要素の行も足した |
| いまのバージョンは妥当 | 削除 | `current_version()` を core から外し、走っているバイナリ (`conductor_tui::VERSION`) が渡す形にした。`CARGO_PKG_VERSION` が 3 要素であることは Cargo が保証していて、テストが守っていたのは自分の crate の Cargo.toml だけ |
| いまのターゲットトリプルが取れる | 移植 | 同名 |
| いまのプラットフォーム向けの資産を見つける | 移植 | プラットフォームに合うアセットだけを選ぶ (表 1 行。4 トリプル全部を並べた) |
| 一致する資産が無ければ何も返さない | 移植 | 同上 (s390x の行) |
| tar_gz以外の資産は無視する | 移植 | 同上 (.zip の行) |
| 資産が空なら何も返さない | 移植 | 同上 (空の行) |

新規 (旧に無かった事実):
- **リリースの応答からタグとアセットを読む / タグの無い応答は読めない** — 旧は `check_for_update`
  の中に JSON の読み取りが埋まっていて、curl を叩かずには試せなかった。`parse_release` を
  切り出し、GitHub の実応答から読む鍵だけを残した fixture (`RELEASE_JSON`) で固定した
- **キャッシュは寿命を過ぎたら読まない / 壊れたキャッシュも無いキャッシュも読めないだけ** —
  旧の `read_cache` は鮮度を見ずに読んでいて、TTL はタイマーの間隔としてしか存在しなかった
- tui 側の合成テスト: `新しいリリースが届くとバッジと更新コマンドが生きる` (`TaskResult` →
  バッジ → `enabled` → メニュー経由で `Modal::Update`)、
  `更新チェックは届かなかったことを最新と混ぜない`、
  `更新の失敗はモーダルを閉じていても伝わる`、`modal/update.rs` の 4 本

API 変更 (旧 → 新):
- `check_for_update() -> Option<UpdateInfo>` → `check(max_age: Duration)`。キャッシュが
  `max_age` 以内ならネットワークに出ない (`max_age` が 0 なら必ず取り直す)。旧は起動の
  たびに必ず GitHub を叩き、`[updates] check_interval_secs` は「タイマーの周期」にしか
  効いていなかった
- `read_cache()` は非公開。「鮮度を問わず読む」入口は無くなった (下の挙動差)
- `current_version()` を削除。現在のバージョンは呼び出し側 (`conductor_tui::VERSION`) が持つ
- `UpdateInfo::tarball_url` と `release_url` を削除。前者はアプリ内ソースビルドを止めた
  時点から、後者は旧 `src/` の時点で誰も読んでいない
- `UpdateInfo` / `ReleaseAsset` に `Serialize`/`Deserialize`/`PartialEq`。キャッシュの
  `CachedAsset` / `CacheEntry` の写し替えが消えた (`#[serde(flatten)]` 1 つ)
- `app/update.rs` の `try_binary_update() -> bool` → `install(asset, version, report) -> Result<()>`。
  失敗が真の理由を持って返る (旧は 3 通りの失敗を 1 つの定型文に潰していた)。進捗は
  `impl Fn(Progress)` で受ける (trait オブジェクトは使わない)
- HTTP は curl のプロセス起動 → `reqwest::blocking`。`GITHUB_TOKEN` を argv に置かない
  ための curl `--config -` の細工ごと不要になった (ヘッダで渡すだけ)
- `UpdateState` (5 状態) + `UpdateProgress` (3 種) + `UpdateFlow` (9 フィールド) →
  `Modal::Update` の 3 バリアントと `Chrome.update` / `Workspace.relaunch` の 2 つ
- チェックの結果は `UpdateCheck { Newer, UpToDate, Unreachable }`。旧の 3 分岐
  (`Some`+新しい / `Some`+最新 / `None`) を `Option` 1 つに潰すと、通信の失敗が
  「最新でした」になり、出ていたバッジまで消える
- `svc` に `EventSender::send_task` を足した。段階を何度も返す唯一の仕事なので、
  `Services::spawn` (1 回で 1 結果) では表せない

挙動差 (旧 → 新):
- **起動時チェックがキャッシュの寿命を見る。** 鮮度が `check_interval_secs` 以内なら
  ネットワークに出ず、キャッシュのバッジを出す。旧は「古いキャッシュでバッジを即出し、
  裏で必ず取り直す」だった。設定の説明 (最小間隔) に挙動を合わせた形
- **`check_interval_secs` の周期チェックは残した。** `check_on_startup` が真のときだけ
  タイマーを持つのも旧と同じ
- **失敗の文面が具体的になった。** 「Could not install the pre-built binary…」の定型文の
  代わりに、ダウンロード / 展開 / 検証 / rename のどれで落ちたかが出る
- **再起動時の `println!` を落とした。** exec の直前に stdout へ書いていたが、書き直しでは
  stdout に出さない
- **バッジのクリック判定 (`badge_cols`) は移していない。** 描画の副産物として座標を持ち回る
  形そのものが新設計で禁じられている。バッジは表示だけで、実行はメニュー / パレットから
- **失敗はモーダルを閉じたあとでも Status に出る。** 進捗モーダルには `esc: hide` が
  あるので、隠したまま落ちると何も知らされないことになる
- **`conductor-tui` の版が 0.1.0 なので、実走では常に「更新あり」に見える。** フェーズ 7 で
  版を揃えるまでの一時的な状態

残したコメント (なぜ):
- `install.rs` `swap_in`: `~/.cargo/bin` 決め打ちが別のファイルを書き換える / 同一
  ディレクトリでないと rename が EXDEV で copy に劣化する / 上書きは macOS arm64 で
  署名を壊し SIGKILL される (各 2〜3 行)
- `install.rs` `make_launchable`: quarantine xattr だけを落とす理由
- `install.rs` `verify_runnable`: 入れ替え前のスモークテストの意図
- `task.rs` `DownloadUpdate`: spawn ではなく送信口を使う理由
- `workspace.rs` `accept_update_check`: 届かなかっただけならバッジを消さない

検証:
- `cargo test -p conductor-core update_checker::` 7 passed
- `cargo test -p conductor-tui` 283 passed / `cargo clippy --workspace --all-targets -D warnings` クリーン
- 擬似端末での実走 (`scratchpad/drive_phase4c.py`): 起動時チェックが無入力でバッジを出す /
  status では騒がない / Help メニューから Check for Updates が通る / 更新情報が無い間は
  Update and Restart が理由付きで断る / 情報が届くと確認モーダルが開く。
  実ダウンロードと再起動は走らせていない
