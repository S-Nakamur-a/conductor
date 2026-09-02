# mcp_serve → crates/conductor-mcp
旧テスト 24 本 → 新テスト 17 本 (全部緑 / clippy -D warnings 通過 / fmt 済み)

## resolve.rs (旧 9 → 新 5)
| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| parse_db_argは両方の書き方を読み値の無い指定を拒む | 移植 | `db引数は両方の綴りを読み値の無い指定を捨てる` (表そのまま) |
| 明示された空パスは拒む | 統合 | `dbパスの優先順位と空値の拒否` の行に吸収 |
| 明示の_db引数は環境変数に勝つ | 統合 | 同上 (両方に値のある行) |
| db引数が無ければ環境変数を使う | 統合 | 同上 |
| 見つからないときは新規作成ではなくエラー | 移植 | `見つからないときは新規作成しない` |
| リフレッシュ用パイプはデータベースの隣に置く | 移植 | `リフレッシュ用パイプはdbの隣に置く` |
| 普通のチェックアウトでのブランチ名 | 統合 | `ブランチ名はdetachedと未コミットでnoneになる` (1 つの repo を未コミット→コミット→detached と進めて 3 状態を固定) |
| detached_headならブランチはnoneになる | 統合 | 同上 |
| 最初のコミット前はブランチがnoneになる | 統合 | 同上 |

## reply.rs (旧 9 → 新 3)
| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 行範囲は単一行と範囲を描き分ける | 統合 | `位置とidの表記` |
| short_idは8文字に切り詰める | 統合 | 同上 |
| 正規化して絶対パスになるものは拒む | 統合 | `パスは同じ形に正規化され脱出は拒まれる` (通す綴り 6 + 拒む綴り 8 の表) |
| 普通の相対パスはそのまま通す | 統合 | 同上 |
| どの綴りでも同じ形に正規化する | 統合 | 同上 |
| 正規化して空になるパスは拒む | 統合 | 同上 (`./` と `.`) |
| 絶対パスと親ディレクトリ参照を捕まえる | 削除 | 同じ事実を上の表が `normalize_repo_relative` 側で固定している。`ensure_repo_relative` は唯一の呼び出し元が同ファイル内なので private 化した |
| スレッドの描画は見出しとメタ情報を必ず出す | 統合 | `スレッドの描画` (branch×replies の 4 通りで、骨組みは常に・任意節は中身があるときだけ) |
| 任意の節は中身があるときだけ出す | 統合 | 同上 |

## tools.rs (旧 6 → 新 8)
| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 公開するツールはちょうど7つ | 移植 | 同名 |
| 無いコメントの解決はnot_foundを返す | 統合 | `見つからないidはツールエラーになる` (id を取る 3 ツールを 1 表で) |
| 無いスレッドの取得はnot_foundを返す | 統合 | 同上 |
| 無いコメントへの返信はnot_foundを返す | 統合 | 同上 |
| 解決するとコメントに印が付く | 移植 | 同名 |
| 返信はclaude名義で保存される | 移植 | 同名 |
| (新規) | 追加 | `アンカーの検証` — 0 行目 / `line_end < line_start` / `MAX_COMMENT_SPAN` 境界 (9999 は可・10000 は不可) / 脱出パス |
| (新規) | 追加 | `detached_headでは書き込みを断る` — create_comment と set_change_summary の両方、DB が空のままであることも確認 |
| (新規) | 追加 | `コメントは正規化したパスと現在のブランチで保存される` |
| (新規) | 追加 | `自己レビューが増えると注意書きが付く` — `SELF_REVIEW_SOFT_LIMIT` ちょうどまでは出ない |

旧はこの 4 本を「cwd の git ブランチを安定して制御できない」として諦めていた。
`McpServer` が cwd ではなくリポジトリルートを持つようにしたので書けるようになった。

## tests/no_stdout.rs (新規 1 本)
`クレートはstdoutへ印字しない` — src/ 配下の .rs を走査し `println!` / `print!` /
`dbg!` / `io::stdout` / `stdout()` を禁じる。fd 1 を実行時に覗く形では捕まらない
(libtest がテストスレッドの print! をスレッドローカルに横取りするので、ハンドラの
println! は fd 1 に出ない)。実際に println! を仕込んで失敗することを確認済み。

## API 変更
- 入口は `conductor_mcp::run(args: impl IntoIterator<Item = String>, version: &str)`。
  旧 `mcp_serve::run()` は `std::env::args()` と `env!("CARGO_PKG_VERSION")` を
  中で読んでいた。version を引数にしたのは、MCP の initialize で名乗る実装バージョンを
  conductor バイナリのもの (0.122.0) のまま保つため。新 crate の版 (0.1.0) を
  名乗ると黙って挙動が変わる。
  呼び出し側: `conductor_mcp::run(std::env::args(), env!("CARGO_PKG_VERSION"))`。
- `McpServer::new(store, db_path, repo_root, version)`。旧は呼び出しのたびに
  `std::env::current_dir()` からリポジトリを discover していた。cwd はプロセスの
  寿命の間変わらないので、起動時に 1 度読んで持つ形に変えた (再 discover は毎回
  やるままなので、裏でブランチが変わる件は旧と同じ)。これで create_comment の
  検証をテストできる。
- `resolve::discover_repo` + `current_branch(&Repository)` → `branch_at(&Path)` 1 本。
- `reply::ensure_repo_relative` を private 化 (呼び出し元は同ファイルの
  `normalize_repo_relative` だけ)。
- `create_comment` のアンカー検証を純関数 `validate_anchor` に括り出した。
  検証の順序は body の空チェックが先になったが、複数同時に不正なときにどのエラー文が
  出るかが変わるだけで、文面は 1 字も変えていない。
- `refresh_pipe::signal_refresh` → 新 crate の `refresh_signal::signal_refresh`
  (書く側だけ)。読む側は持ち込んでいない。
- review_store の新 API に追従: `add_review(NewReview{..})`、enum は ToSql 実装済み。
  旧の `commit_ref="HEAD"` と `worktree=Some(&branch)` は NewReview に無く、
  スキーマの既定値と `INSERT ... VALUES (?1, ?2, ..., ?2)` が担当する。

## 契約 (変えていないもの)
7 ツールの名前・引数名・`#[tool(description=...)]` の文言・args.rs の doc コメント・
返信文は 1 字も変えていない。`ok_text` / `err_text` の使い分け (ツールレベルの失敗は
isError 付きの成功) もそのまま。

## 判断が要る点
- **refresh.pipe のパスは DB 基準のまま**にした (指示は `git_engine::conductor_dir` から
  導く、だった)。旧コードがそこを明示的に「git ルートではなく DB 基準」と書いていて
  テストでも固定していること、`--db` が明示されたときや git リポジトリ外で起動された
  ときに conductor_dir が破綻することが理由。通常の経路では DB 自身がメイン worktree の
  .conductor に解決されるので、結果のパスは指示どおり
  `<main worktree>/.conductor/refresh.pipe` になる。
  conductor_dir 基準に寄せるべきなら差し替えは数行。

## 追加した依存 (crates/conductor-mcp/Cargo.toml のみ)
`git2` (ブランチ / DB の探索)、`libc` (FIFO を O_NONBLOCK で開く)。

## 残したコメント (なぜ)
- `--db` の空値を捨てる: `Connection::open("")` が一時 DB を開き、全ツールが成功して
  終了時に消える。
- 探索経路がファイルの実在を要求する / `review_store::db_path` を使わない: 空の DB を
  作ると TUI に何も出ないのに全ツールが成功を報告する。
- リンク worktree の `commondir().parent()`。
- refresh.pipe を DB 基準にする理由。
- `line_start == 0` を u32 だけで弾けない理由 (読み戻す側の `saturating_sub(1)` で
  黙って 1 行ずれる)。
- `MAX_COMMENT_SPAN` がある理由 (行ごとのキャッシュ実体化 + 書き込みが必ず FIFO を突く)。
- 件数をそのまま返す方が静的な指示より効く、という設計意図。
- `signal_refresh` がベストエフォートである理由と O_NONBLOCK が要る理由、
  通常ファイルへの書き込みを拒む理由 (先頭バイトを潰す)。
- store が Mutex の背後にある理由 (rusqlite::Connection が Send だが Sync でない)。
- current-thread ランタイムでタイマーを有効にする理由 (rmcp が無いと panic する)。
- テスト側: libtest の print! 横取りで fd 1 監視が使えないこと、tokio に macros
  フィーチャが無いこと。
