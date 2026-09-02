# watch/ 移植報告

対象: `src/file_watcher.rs`, `src/config_watcher.rs`, `src/cc_notify.rs`,
`src/refresh_pipe.rs` (読み側のみ) → `crates/conductor-svc/src/watch/`

## テスト対照表

### file_watcher.rs (旧: テストなし)

| 旧テスト | 新テスト | 状態 |
|---|---|---|
| (なし) | `ファイル変更を検知する` | 新規 |
| (なし) | `gitとconductor配下の変更は無視する` | 新規 |

### config_watcher.rs

| 旧テスト | 新テスト | 状態 |
|---|---|---|
| `ファイル名が完全一致する` | 同名 | 移植 (純粋関数、無変更) |
| `拡張子が付いていれば一致しない` | 同名 | 移植 |
| `別のファイル名には一致しない` | 同名 | 移植 |
| `親ディレクトリだけでは一致しない` | 同名 | 移植 |
| `パスがファイル名だけでも一致する` | 同名 | 移植 |
| (なし) | `設定ファイルの変更を検知する` | 新規 (watcher 統合テスト) |

### cc_notify.rs

| 旧テスト | 新テスト | 状態 |
|---|---|---|
| `状態のメッセージを読む` | — | **削除**: `parse_message` は `conductor_core::cc_hook::Notification::parse` へ移設済み。同じ電文を検証する `電文は往復する` が core 側にある |
| `セッションのローテーションを読む` | — | **削除**: 同上 |
| `壊れたメッセージは拒む` | — | **削除**: `壊れた電文は拒む` として core 側に存在 |
| `空白を含むcwdも壊れない` | — | **削除**: core 側の `電文は往復する` が同じ cwd ケースを含む |
| (なし) | `状態のメッセージが届く` | 新規 (ソケット→WatchEvent の統合テスト) |
| (なし) | `セッションのローテーションが届く` | 新規 |
| (なし) | `dropでソケットファイルが片付く` | 新規 |

### refresh_pipe.rs (読み側のみ移植)

| 旧テスト | 新テスト | 状態 |
|---|---|---|
| `書き込み1回でイベントが出る` | 同名 | 移植 (signal_refresh → ブロッキング open の直接書き込みに変更) |
| `複数回の書き込みでその数だけイベントが出る` | 同名 | 移植 |
| `書き込みが無ければイベントも出ない` | 同名 | 移植 |
| `パイプが消えたリスナを畳んでもハングしない` | 同名 | 移植 (Drop の安全性、Ctrl+Q 退行の番人) |
| `パイプが無ければ即座に返る` | — | **未移植**: `signal_refresh` (書き手側) 自体のテスト。conductor-mcp へ移る関数の性質なので、移設先で再作成すべき |
| `読み手がいなければ即座に返る` | — | **未移植**: 同上 |
| `普通のファイルには書き込まない` | — | **未移植**: 同上 |

移植 12 / 新規 6 (統合テスト) / 削除 4 (core へ移設済みで重複) / 未移植 3 (書き手側、conductor-mcp 行き)

## API 変更

- 4 つの旧イベント型 (`FsEvent` / `ConfigEvent` / `CcNotifyEvent` / `RefreshEvent`) を
  `watch::WatchEvent` 1 本に統合。全 watcher が同じ mpsc へ送るため。
- 各 watcher の `poll()` を全廃。`new(..., sender: EventSender<P>)` が自前スレッド
  (notify 系は notify 自身の内部スレッド) を起動し、`EventSender::send_watch` で
  直接送る。呼び出し側の `Services<P>::try_recv` が唯一の受け取り口。
- `CcNotifyListener::new` はソケット path 解決を自前で持たず
  `conductor_core::cc_hook::socket_path` を再利用。電文パースも
  `conductor_core::cc_hook::Notification::parse` に一本化 (`parse_message` 廃止)。
- `RefreshPipe::new` は `.conductor/refresh.pipe` の場所を
  `conductor_core::git_engine::conductor_dir` から導出。
- `RefreshPipe` は書き込み側 (`signal_refresh`) を持たない。conductor-mcp へ移設予定。
- `RefreshPipe::new` / 旧 `#[cfg(test)] fn from_path` の重複を解消し、
  両方が private な `from_path` へ委譲する形に統合。
- 各 `new` は `P: Send + 'static` のジェネリック。watcher は `P` の具体型を知らない。

## 残したコメント (なぜ)

- `file_watcher.rs`: `.git/` `.conductor/` 除外の理由 (git 操作・revidere 書き込みでの
  誤リフレッシュ防止、rename イベントの paths[0] が .git 側になり得る点)。
- `config_watcher.rs`: ファイルでなく親ディレクトリを監視する理由 (エディタの
  一時ファイル→リネーム保存で inode が入れ替わるため)。
- `cc_notify.rs`: 1 回の read で足りない理由 (フォーマット済み文字列の複数回 write)。
- `refresh_pipe.rs`: FIFO 特有の開き方の理由一式 — `File::open` が FIFO の
  O_NONBLOCK-at-open を表現できないので libc を直叩きする理由、EOF で開き直す
  ループの理由、Drop で読み手のブロックを解くために O_NONBLOCK で書き込み用に
  開く理由。全て「コードだけでは伝わらない why」。

## 前提として置いた判断 (要確認なら教えてください)

- `FileWatcher` に debounce や .gitignore 解釈は実装していない。旧コードにも
  無かった機能で、conductor-svc の Cargo.toml に `ignore`/`git2` が無いこと、
  および debounce は UI ループ側の方針 (将来の conductor-tui) に属するという
  設計原則と整合すると判断した。

## ブロッカー (解消済み)

`crates/conductor-svc/src/lib.rs` で `gen` が edition 2024 の予約語と衝突し
`cargo check -p conductor-svc` が 17 エラーで落ちる問題を報告済み。この報告後、
別セッションで修正され、現在は解消している (`cargo check` / `test` / `clippy` /
`fmt` 全て通過済み)。
