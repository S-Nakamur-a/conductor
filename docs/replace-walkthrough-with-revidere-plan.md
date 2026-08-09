# walkthrough を廃止し revidere に置き換える計画

対象ブランチ: `refactor/replace-walkthrough-with-revidere`
調査日: 2026-08-05 / conductor 0.102.0 / revidere 0.2.0 (`8ea399e`, remote 未設定)

---

## 1. 現状把握 — walkthrough はどこにあるか

### 1.1 丸ごと消えるファイル

| ファイル | 行数 | 役割 |
|---|---:|---|
| `src/walkthrough.rs` | 637 | データモデル・生成プロンプト・応答パーサ |
| `src/review_store/walkthroughs.rs` | 635 | SQLite 永続化 (`begin`/`save`/`fail`/`get`) |
| `src/app/review_walkthrough.rs` | 490 | 生成のオーケストレーション (背景スレッド、ブランチごと1本) |
| `src/ui/walkthrough_pane.rs` | 265 | Explorer 下段ペインの描画 + 詳細オーバーレイ |
| `src/app/walkthrough_view.rs` | 251 | ステップ選択・diff へのジャンプ・viewed トグル |
| `src/event/explorer_walkthrough.rs` | 43 | ペインのキー処理 |
| `src/app/state/walkthrough.rs` | 34 | `WalkthroughState` / `LoadedWalkthrough` |
| `plugins/conductor/commands/conductor-walkthrough.md` | 44 | スラッシュコマンド (MCP 経由で保存) |
| **計** | **約 2,400 行** | |

なお `toggle_path_viewed` (`src/app/walkthrough_view.rs:174`) だけは walkthrough とは
無関係に diff 一覧の `v` キーが使っているので、消さずに移設する。

### 1.2 書き換えが要るファイル (実測、file:line)

**状態と配線**

- `src/main.rs:52` — `mod walkthrough;`
- `src/app/mod.rs:21-22,25-27,36,101` — モジュール宣言・re-export・`App.walkthrough` フィールド
- `src/app/state/mod.rs:22,36`
- `src/app/lifecycle.rs:110` — 初期化
- `src/app/update.rs:117` — `poll_walkthrough_generation()`
- `src/app/review.rs:7,18-23,31` — `refresh_reviews` がステップを読み直す
- `src/event_loop/mod.rs:81` — 終了時 `shutdown_walkthrough_generation()`
- `src/viewer/state.rs:151-152,186-200,221-225` — `ExplorerBottomView::Walkthrough` と選択/スクロール/`viewed_steps`

**入口 (キー・パレット・メニュー)**

- `src/keymap/action.rs:164-185,235-241` — `ShowWalkthrough` / `WalkthroughNextStep` / `WalkthroughPrevStep` / `GenerateWalkthrough` / `ForceGenerateWalkthrough`
- `src/keymap/context.rs:12-13,33,51` — `KeyContext::ExplorerWalkthrough`
- `src/default_keybinds.toml:147-154,203-219` — `w` / `W` / `alt+w` / `[layers.explorer_walkthrough]`
- `src/command_palette/commands.rs:246-293`, `types.rs:52-53,66`
- `src/menu/model.rs:135-143` — Review メニューの3行
- `src/app/commands.rs:52-53,60,231-239`
- `src/event/global.rs:187-192`, `event/mod.rs:13,52,66-67,97-98,226-231`, `event/scroll.rs:36-47`, `event/explorer/tree.rs:19,33-34,44-45`

**描画**

- `src/ui/mod.rs:16`, `ui/explorer_panel/mod.rs:62-63`
- `src/ui/explorer_panel/file_tree.rs:67-76` — 「walkthrough できたよ」バッジ
- `src/ui/viewer_panel/diff_view.rs:99-132,192-213` — ステップのバナーと行範囲ハイライト
- `src/ui/viewer_panel/diff_line.rs:22-25,50-51,227-229` — 行範囲の下線
- `src/ui/viewer_panel/file_view.rs:19,122-132`
- `src/ui/layout/overlays.rs:68-69` — 詳細オーバーレイ
- `src/ui/dashboard/help.rs:132,188`
- `src/ui/viewer_panel/summary_view.rs:51-52` — SUMMARY 疑似ファイルの空表示文言

**永続化と MCP**

- `src/review_store/schema.rs:276-340` — v6 で `walkthroughs` / `walkthrough_steps` を作り、v7 で `head_commit` 列を追加。テストは `463-491`
- `src/review_store/mod.rs:11,26`, `worktree_metadata.rs:36-37`
- `src/mcp_serve/tools.rs:36,360,381-487` — `save_walkthrough` ツール本体。テスト `510-524,562-731`
- `src/mcp_serve/args.rs:12,98,151` — `SaveWalkthrough` 引数型

**設定**

- `src/config/sections.rs:134-136` — `[review] walkthrough_language`
- `src/config/persist.rs:66-68,107-113` — 雛形コメント
- `src/config/tests_config.rs:50`

**テスト**

- `src/keymap/tests/resolution.rs:75-87,269-290`
- `src/command_palette/tests.rs:55-60`

**文書とコメント (機能ではないが記述が残る)**

- `README.md:45,117-127`, `CLAUDE.md:29,102,117-118`
- `src/ai_caller.rs:43-45,278,302,446`, `src/gemini_api.rs:5`, `src/repo_path.rs:1,7`,
  `src/diff_state/display_list.rs:209,230`, `src/diff_state/tests.rs:223,255`,
  `src/ui/markdown/mod.rs:65`, `src/ui/reflow_view/user_text.rs:70`,
  `src/app/worktree_smart.rs:67,79`, `src/app/worktree_pr.rs:73`
- `docs/code-nav-ast-queries-plan.md:239-240` は当時のログなので触らない

合計でおよそ **45 ファイル**に手が入る。

### 1.3 walkthrough の実行フロー

```
パレット / W キー
  → App::cmd_generate_walkthrough            (app/review_walkthrough.rs:131)
     ├ 同じ HEAD の ready 行があればスキップ  (:173)
     ├ begin_walkthrough で generating 行を先に置く (:205)
     └ 背景スレッド → walkthrough::generate  (walkthrough.rs:416)
          → ai_caller::build_caller(&[api])   ※ conductor は claude を自分で起動しない
          → complete(system, user, cancel)    working_dir = 対象 worktree
          → parse_generated (JSON 抽出→検証)  (walkthrough.rs:275)
  → poll_walkthrough_generation               (app/review_walkthrough.rs:274)
     ├ save_walkthrough  → walkthroughs / walkthrough_steps + change_summary
     └ comments[] → add_review (Question, author=Claude)
```

入出力:

- **入** — ブランチ名 / base ref (PR intake のメタから) / `[review] walkthrough_language` / 対象 worktree のパス
- **出** — `title` / `summary` / `steps[]`(`file_path`, `line_start`, `line_end`, `kind`, `title`, `body`) / `comments[]`
- 行番号は **new 側のみ** (`walkthrough.rs:192`)。削除行は指せない
- 保存先は `<repo-root>/.conductor/conductor.db`。ブランチごとに1本、履歴なし

もう1つの入口が MCP で、`/conductor-walkthrough` スラッシュコマンドが
`save_walkthrough` ツールを呼ぶ (`mcp_serve/tools.rs:383`)。両方が同じテーブルに書く。

---

## 2. revidere の実像

### 2.1 何であるか

3 クレートのワークスペース。**ライブラリと解析が意図的に分かれている**。

| クレート | 成果 | 依存 | 役割 |
|---|---|---|---|
| `crates/revidere` | lib | serde / serde_json のみ | 成果物の型・`Annotations`・`ReadingOrder`。**ホストが埋め込むのはこれだけ** |
| `crates/revidere-cli` | bin `revidere` | + toml | 解析。プロンプト・AI 起動・応答の解釈・充足検査・キャッシュ |
| `crates/revidere-view` | bin `revidere-view` | + ratatui/crossterm | 参照実装の TUI |

`README.md:19-20` が明言している —
「解析はライブラリに入れていない。成果物を読むだけのホストに、AI 呼び出しの都合を背負わせないため」。

### 2.2 CLI

```
revidere analyze [--repo <path>] [--base <ref>] [--head <ref>] [--out <path>]
                 [--config <path>] [--ai <cmd>] [--timeout <s>] [--no-repair] [--no-cache]
revidere verify | ledger | prompt | config | check <file>
revidere-view [<file>] [--repo <path>] [--dump --width --height --tab --scroll]
```

- `--head worktree` で HEAD vs 作業ツリー。未追跡ファイルは `--no-index` で1件ずつ足す (`README.md:38-41`)
- 既定の出力先は `<repo>/.revidere/review.json` (`crates/revidere/src/review.rs:17`)
- 充足検査に落ちたら **終了コード 2**
- 応答は `<repo>/.revidere/cache/` に貯まる。鍵は argv + system + user + **diff 本文** (`cache.rs:92`)、古いものから 50 件

### 2.3 AI の継ぎ目

revidere も AI CLI を同梱しない。`<repo>/.revidere/config.toml` → `~/.config/revidere/config.toml`
の順に読む。現行の実機設定 (`~/.config/revidere/config.toml:14`):

```toml
[ai]
command = ["claudep", "-w", "{workdir}", "--timeout", "20m", "{prompt}"]
timeout_secs = 900
```

**これは conductor の `[api]` seam と同じ思想**（プロンプトを渡して補完テキストを受け取るだけ、
モデル選択はユーザ側）で、別系統の設定ファイルに置かれているだけ。

### 2.4 成果物のスキーマ (`crates/revidere/src/review.rs`)

```
Review { schema, base, head, overview, sections[], impacts[], coverage }
  Overview  { problem, change, mechanism, placement, scope }        ← 毎回同じ5欄
  Section   { title, body, importance, reason, ranges[], relations[] }
    Importance = core | ripple | follow | minor
    Range     { path, side, start, end }   side = new | old | file
  Impact    { feature, change, verify, gap, confidence }  confidence = fact | guess
  Coverage  { total, classified, unclassified[], conflicts[], unknown[] }
```

ホストが最初に触るのは `Annotations::from_json` → `ReadingOrder::build(&diff, &ann)`
(`README.md:148-169`)。**歩くのは diff であって節ではない**ので、成果物が壊れていても
変更行は画面から消えず、最悪でも帯のない素の diff に退化する。

### 2.5 local 参照 → リモート参照の差分

現在 remote 未設定 (`git remote -v` が空)。切り替えで動くのは **2 行だけ**。

```toml
# いま (local)
revidere = { path = "../../revidere/crates/revidere" }
# 公開後
revidere = { git = "https://github.com/S-Nakamur-a/revidere", package = "revidere" }
```

path 依存は `Cargo.lock` に版を刻まないので、`cargo install --path .` がリポジトリの
場所に依存する。CI (`.github/workflows/release.yml`) では revidere を並べて置かない限り
ビルドが通らない — これが **local 参照のまま CI に出せない理由**で、公開までは
ローカル専用ブランチに留めるか、`[patch]` を使うことになる。

---

## 3. ギャップ分析

### 3.1 対比表

| 観点 | walkthrough | revidere | 移行後 |
|---|---|---|---|
| 生成の起動 | conductor が `[api]` 経由で背景スレッド | `revidere analyze` (CLI) | conductor が `revidere analyze` を背景で起動 |
| モデル選択 | `[api] provider/command` | `[ai] command` | 設定ファイルが `~/.config/revidere/config.toml` へ移る |
| 出力の器 | SQLite (`conductor.db`) | JSON (`.revidere/review.json`) | ファイルへ |
| 単位 | ブランチ | base…head の組 | worktree ごとに別ディレクトリなので自然に分かれる |
| 構造 | intent → core → ripple → test の**物語順** | 重要度順 (core/ripple/follow/minor) + 5欄 overview + 機能影響 | 並べ替えの軸が変わる |
| 網羅の保証 | **なし** (任意の点を指すだけ) | **充足検査** — 全変更位置がちょうど1節に属する | ★獲得 |
| 削除行 | 指せない (new 側のみ) | `side: old` で一級 | ★獲得 |
| バイナリ/rename | 指せない | `side: file` | ★獲得 |
| 重要度の根拠 | なし | `reason` が全節必須 | ★獲得 |
| 機能への影響 | なし | `impacts[]` (verify / gap / confidence) | ★獲得 |
| 節どうしの関係 | なし | `relations[]` | ★獲得 |
| 作業ツリー差分 | 不可 (merge-base 固定) | `--head worktree` | ★獲得 |
| 応答の再利用 | なし (毎回フル生成) | キャッシュ (実測 4:26 → 0.08s) | ★獲得 |
| インラインコメント自動生成 | あり (`comments[]` → question コメント) | **なし** | ▲喪失 |
| change summary (SUMMARY 疑似ファイル) | 生成の副作用で埋まる | なし | ▲喪失 (代替は §3.3) |
| 出力言語の設定 | `[review] walkthrough_language` | **日本語固定** (`prompt.rs:127`) | ▲喪失 (実害は薄い) |
| 生成中/失敗の状態表示 | DB の `generating`/`failed` 行 | プロセスの生死のみ | ホスト側で持ち直す |
| 進捗キャンセル | `AtomicBool` → 子プロセス kill | ホストの仕事 | 同等を作り直す |

### 3.2 壊れる箇所

1. **既存の walkthrough データは消える。** `walkthroughs` / `walkthrough_steps` を落とすので、
   既に生成済みのツアーは読めなくなる。移行スクリプトは書かない（データモデルが
   step 列 → 節 + 範囲で別物であり、変換しても意味が保たれない）。
2. **`/conductor-walkthrough` スラッシュコマンドが消える。** マーケットプレイスから
   プラグインを入れている利用者には破壊的変更。MCP ツールも 8 → 7 個になり、
   `tools.rs:510` の「ちょうど 8 個」テストを直す必要がある。
3. **`[review] walkthrough_language` が無効になる。** 設定に残っていても読まれない。
4. **`.revidere/` が conductor の `.gitignore` に無い** (現状 `/target`, `/.conductor/`,
   `node_modules/`, `dist/`, `.env`, `.vscode`, `.claude/` のみ)。追加しないと成果物と
   キャッシュが commit 対象に出てくる。
5. **`revidere` の SCHEMA_VERSION が実態を追っていない。** 版は 1 のままだが、
   実データ上は `contexts` → `sections`、`decision` → 重要度 4 値へ形が動いている。
   実際に手元の `conductor/.revidere/review-pr313.json` は今のバイナリで読めない
   (§5.2)。ホストとして埋め込む以上、**上流で版を上げてもらうのが前提**。

### 3.3 手作業が要る箇所

- **SUMMARY 疑似ファイル** — walkthrough が副作用で書いていた change_summary は、
  MCP の `set_change_summary` (`tools.rs:333`) が残るので手段自体は生きている。
  自動で埋まらなくなるだけ。`overview` を流し込む配線を足すなら別タスク。
- **インライン question コメント** — 自動生成が消える。`create_comment` MCP ツールと
  `/conductor-walkthrough` 相当のスキルで代替するかは別途判断。
- **`w` / `W` / `alt+w` キー** — 同じキーを revidere の入口に付け替える（決定済み、§4-S3）。
  意味は `w` = 節ビューを開く / `W` = 解析を走らせる / `alt+w` = キャッシュを無視して再実行。
  `W` が「同じ HEAD ならスキップ」していた挙動は、revidere のキャッシュがそのまま
  引き受ける（鍵に diff 本文が入るので、差分が動いていなければ AI は起動しない）。

---

## 4. 移行手順

各ステップは単独でビルドが通る単位に切ってある。

### S0. 前提を整える (コード変更なし)

- `revidere` / `revidere-view` を `~/.cargo/bin` へ (`make install` in revidere)
- `~/.config/revidere/config.toml` の `[ai] command` を確認 — **済** (§5.1)
- **検証**: `revidere config --repo <conductor-worktree>` が設定の読み先を出す

### S1. 削除 — walkthrough を落とす

削除: §1.1 の 8 ファイル。
書き換え: §1.2 のうち「状態と配線」「入口」「描画」「永続化と MCP」「設定」「テスト」。

要点:

- `ExplorerBottomView` は `DiffList` / `Comments` の 2 値へ。`Walkthrough` の分岐が
  `event/explorer/tree.rs:44`、`ui/explorer_panel/mod.rs:62` から消える
- `toggle_path_viewed` を `app/walkthrough_view.rs:174` から `app/review.rs` へ移設
- `review_store/schema.rs` は **v6/v7 の記述は残したまま新しい版で DROP** する。
  過去のマイグレーションを書き換えると既存 DB が壊れる
- `mcp_serve/tools.rs:510` の「8 個」テストを 7 個に
- `[review] walkthrough_language` を削除。`config/persist.rs` の雛形コメントも

**検証**: `cargo build` / `cargo test` / `cargo clippy` が通る。
`grep -ri walkthrough src/ plugins/` が 0 件 (docs の歴史記述を除く)。

### S2. 追加 — 成果物を読む層

```toml
# Cargo.toml
revidere = { path = "../../revidere/crates/revidere" }
```

- `.gitignore` に `/.revidere/` を追加
- 読み込み層を新設 (`src/revidere.rs` 仮)。`Annotations::from_json` を呼び、
  `LoadError` の 2 値 (JSON 破損 / スキーマ版違い) をそのままステータスに出す。
  **ファイルが無いのは正常** — 素の diff を描くだけ (`README.md:151-153`)
- 成果物の場所は `revidere::review::artifact_path(worktree)` から取る。
  自分でパスを組み立てない (書く側と読む側が割れると黙って食い違う)
- `App` に `revidere: Option<Annotations>` 相当を持たせ、`refresh_reviews`
  (`app/review.rs:18`) が walkthrough を読んでいた場所で読み直す。
  ファイル監視 (`file_watcher.rs`) が `.revidere/review.json` の更新を拾えるようにする
- **`Importance` も `Side` も `Section` も自前で宣言し直さない** (`README.md:182-184`)。
  型が動いたときに黙って壊れるのを防ぐ。色は `Importance::recommended_rgb()` を
  テーマ色に写すだけ

**検証**: `conductor/.revidere/review-pr315.json` を worktree に置いて起動し、
成果物が読めていることをステータスに出す。壊れた JSON と版違いで別々の文言が出る。

### S3. 追加 — 2 列のレビュービュー

**画面全体を「読む順 (節一覧) | diff」の 2 列に差し替える** (Terminal 列も隠す)。

- `ui/layout/render.rs:48` の `if app.editor.is_some()` が既に
  「Explorer + Viewer を 1 枚に潰す」前例になっている。同じ層に分岐を足すが、
  こちらは **`main_area` 全体**を取るので `render.rs:82-84` の terminal 描画も
  短絡させる必要がある (エディタは terminal 列を残しているので、そこだけ挙動が違う)
- 左列は `ReadingOrder::build(&diff, &ann)` の `sections` を上から。
  重要度順で、持ち主なしの行は末尾の「未分類」に落ちる
- 右列は同じ `ReadingOrder` を歩いて描く。**歩くのは diff であって節ではない**ので、
  成果物が壊れていても変更行は消えない (`README.md:171-173`)
- worktree ストリップは最大化時と同じく隠す (`ui/worktree_bar.rs` の既存挙動に合わせる)
- 抜けるキーは `q` / `Esc` で通常の 3 列へ戻る
- `KeyContext` を 1 つ追加 (`Revidere` 仮)。`ExplorerWalkthrough` が消えた枠を使う

**検証**: 成果物ありで起動 → `w` で 2 列になり、節が重要度順に並び、
diff の変更行に重要度の帯が出る。`revidere-view --dump` の同じ成果物・同じ幅の
出力と読み比べて、節の並びと帯の位置が一致する。成果物を消すと素の diff に退化する。

### S4. 追加 — 解析の起動

conductor が `revidere analyze` を背景プロセスとして起こす。

- 起動は `--repo <worktree> --head worktree`。**レビューしたいものは大抵まだ
  コミットされていない**ので、merge-base 固定だった walkthrough とはここが変わる
- 出力先は既定 (`<worktree>/.revidere/review.json`) のまま。conductor の worktree は
  それぞれ別ディレクトリなので、ブランチごとに自然に分かれる (`--out` は要らない)
- 実装は `app/review_walkthrough.rs` の骨格をほぼそのまま流用できる:
  ブランチごとに 1 本・`AtomicBool` でキャンセル・`poll_*` で回収・終了時 `abort_all`。
  違いは中身が `ai_caller` ではなく `std::process::Command` になること
- 終了コードの扱い: **0 = 成功 / 2 = 充足検査が未通過 (成果物はある)** / それ以外は失敗。
  2 は「読めるが穴がある」なので、成果物を読んだ上で警告を出す
- `revidere` が PATH に無い場合の文言をはっきり出す (これが唯一の新しい必須依存)
- キーは `w` = 2 列ビューを開く / `W` = 解析 / `alt+w` = `--no-cache` 付きで再解析

**検証**: 適当な worktree に変更を置いて `W`。数分後にステータスが完了を報告し、
`w` で節が出る。もう一度 `W` を押すと **AI が起動せずキャッシュから即座に返る**
(revidere が「貯めてある応答を使う」と出す)。`revidere` を PATH から外して
分かる文言が出ることも確認する。

### S5. 文書

- `README.md:45,117-127` — walkthrough の説明を revidere の説明へ
- `CLAUDE.md:29,102,117-118` — モジュール表と MCP の記述
- `Cargo.toml` の version を **0.103.0** へ (機能の入れ替え = MINOR。
  MCP ツールが減る点は破壊的だが、プラグイン側の互換は既に「壊してよい」方針)

---

## 5. local 検証 — 実施済み

すべて実機で実行した。生の出力を添える。

### 5.1 設定の読み先

```
$ revidere config --repo <conductor-worktree>
設定: /Users/shunnaka/.config/revidere/config.toml
AI コマンド: claudep -w {workdir} --timeout 20m {prompt}
実時間上限: 900 秒
貯めた応答: 0 件 / <worktree>/.revidere/cache
```

### 5.2 既存成果物の充足検査

```
$ revidere check .revidere/review-pr315.json --repo .
変更位置 706 件 / 分類済み 706 件

充足検査: 通過
節 16 件（中核 6 / 波及 2 / 追従 1 / 周辺 7）
機能への影響 6 件

$ revidere check .revidere/review-pr313.json --repo .
失敗: unknown variant `decision`, expected one of `core`, `ripple`, `follow`, `minor`

$ revidere check .revidere/samples/pr315.json --repo ../conductor
失敗: missing field `sections` at line 625 column 1
```

→ **最新の成果物は通るが、少し前のものは読めない。** `SCHEMA_VERSION` は 1 のまま
形が動いている (§3.2-5)。conductor をホストにするなら上流で版を上げてもらう。

### 5.3 台帳と git の一致

```
$ revidere verify --repo <conductor> --base 53e0b15 --head da171f5
一致: 11 ファイル / 変更位置 706 件

$ revidere ledger --repo <conductor> --base 53e0b15 --head da171f5 | head
Cargo.lock [modified]
  new: 445
  old: 445
src/app/appearance.rs [modified]
  new: 8-43, 46-49, 57-66, 87-88, 103-105, 123-126
  old: 4-5, 19-21, 56, 62-80
```

### 5.4 作業ツリーモード — **1 件不具合を発見**

一時的に `src/repo_path.rs` を1行いじり、未追跡ファイルを1つ置いて実行 (実行後に復元済み):

```
$ revidere ledger --repo . --head worktree
src/repo_path.rs [modified]
  new: 84-85
scratch_revidere_probe.txt [added]
  new: 1
---
変更位置 3 件

$ revidere verify --repo . --head worktree
ファイル数が違う: numstat 1 + 未追跡 2 / 台帳 2
未追跡 2 ファイルを台帳に含めた
不一致: 2 ファイル / 変更位置 3 件
```

原因: この worktree の `.claude` は **ディレクトリへのシンボリックリンク**で、
git は未追跡として数えるが `--no-index` では 1 ファイルとして起こせない。
数が合わず `verify` が「不一致」を出す。**解析自体は正しく通り、台帳から `.claude` が
落ちるだけ**なので致命ではないが、`verify` の数え方は上流の修正対象。

### 5.5 end-to-end の生成

```
$ revidere analyze --repo <conductor> --base 53e0b15 --head da171f5 --out <scratch>/review-pr315-probe.json
設定: /Users/shunnaka/.config/revidere/config.toml
<conductor>: 53e0b15...da171f5 / 11 ファイル / 変更位置 706 件
AI を起動: claudep -w {workdir} --timeout 20m {prompt}
書き出した: <scratch>/review-pr315-probe.json
変更位置 706 件 / 分類済み 706 件

充足検査: 通過
節 11 件（中核 5 / 波及 0 / 追従 0 / 周辺 6）
機能への影響 6 件

real 4m53s
```

キャッシュには当たらず AI を起動した（プロンプトが前回から動いているため）。
**706 件すべてが分類され、充足検査を通過**。

### 5.6 描画

`revidere-view --dump` で端末を起こさずに 1 画面取得。

- タブ1「概要」— 困っていたこと / やったこと / 仕組み / 置き場所 の 4 欄が描画される
- タブ2「diff」— 左に「読む順」(中核→周辺の入れ子)、右に節ごとの diff。
  変更行に重要度の帯 (`▌`) が付き、ステータス行に **「全部の変更行に説明あり」**

**結論: revidere は local 参照のまま期待どおり動く。** 計画の前提は成立している。

---

## 6. 決まったこと (2026-08-05)

1. **解析は conductor が `revidere analyze` を起動する。** revidere が PATH 上の
   必須依存になる。モデル選択は revidere の `[ai] command` に委ねるので、
   「conductor はどのモデルが答えるかを決めない」という原則は保たれる。
   ユーザから見える変化は、設定の置き場が `~/.config/conductor/config.toml` の
   `[api]` から `~/.config/revidere/config.toml` の `[ai]` へ移ること。
2. **表示は画面全体を 2 列に差し替える。** Terminal 列も隠し、
   「読む順 (節一覧) | diff」の 2 列にする。
3. **`w` / `W` / `alt+w` は同じキーのまま revidere に付け替える。**

残っている未確定は 1 つだけ:

- **`[api]` セクションを conductor に残すか。** スマート worktree 命名
  (`app/worktree_smart.rs`) がまだ使っているので **残す**。walkthrough が消えても
  `[api]` の利用者はゼロにならない。`config/persist.rs:107-113` の
  「walkthrough もここを見る」という説明だけ落とす。

---

## 7. 実装の結果 (2026-08-06)

S1〜S5 を実装済み。`cargo build` / `cargo test` (988 件) / `cargo clippy` すべて通過。
版は 0.103.0。

### 実機での確認 (pty でキーを送って画面を採取)

小さな demo リポジトリ (`calc.py` に例外ガードを追加 + テスト新設、変更位置 11 件) を
作り、`revidere analyze --head worktree` で成果物を用意してから conductor を起動した。

| 操作 | 結果 |
|---|---|
| 起動 | Explorer のタイトルに 🧭 バッジ = 成果物を読めている |
| `w` | 画面全体が「読む順 \| diff」の 2 列に。Terminal 列は消える |
| 左列 | 中核/周辺の帯付き、周辺が中核の下に字下げされて並ぶ |
| 右列 | 節の本文 + 「なぜ中核:」の理由 + hunk + 変更行に `▌` の帯 |
| 見出し | 「変更行 11  全部の変更行に説明あり」= 充足検査の結果 |
| `n` / `N` | 節の先頭ちょうどへスクロール (3 節を順に確認) |
| `j` / `k` | 行スクロール (内容が画面に収まるときは動かない = 正しい) |
| `Enter` | 3 列レイアウトへ戻り、Viewer がその節のファイルの diff を開く |
| `q` | 3 列レイアウトへ戻る |
| `W` | `revidere analyze` が起動。キャッシュに当たり 2 秒以内に `✓ Review ready for 'main'.` |
| `W` (PATH に revidere 無し) | `✗ revidere failed for 'main': \`revidere\` is not on PATH — install it with \`cargo install --path crates/revidere-cli\`` |

### 計画から変えたこと

- **`Enter` で Viewer へ飛ぶ経路を足した。** 計画には無かったが、レビューコメントを
  書けるのは Viewer なので、これが無いと 2 列ビューが既存のコメント作成から
  切り離されてしまう。`diff_state` の寛容なパス解決 (`resolve_changed_path` /
  `reveal_path`) は、これで walkthrough から引き継いだ用途を保っている。
- **読み直しに mtime の門を付けた。** `crate::revidere::load` は git diff を取り直すので、
  MCP がコメントを 1 件書くたびに走らせるには重い。成果物のパスと更新時刻が
  前回と同じなら丸ごと飛ばす。ビューを開くときだけ門を通さず読み直す。
- **`.revidere/` をファイル監視の除外に足した。** 解析中は貯めた応答を書き続けるので、
  監視したままだと高コストなリフレッシュが走り続ける。
- **`PrReviewMeta.base_ref` を読む側から外した。** 唯一の読み手が walkthrough だった。
  列と書き込みは残してある (PR の素性の記録として)。
- **path 依存は絶対パスにした。** worktree は `conductor-worktrees/<ブランチ名>/` に
  作られ、ブランチ名の `/` の数だけ深さが変わるので、本体と worktree の両方から
  届く相対パスが書けない。

### 実機で見つかったこと

1. **対象リポジトリで `.revidere/` が ignore されていないと、revidere が自分の出力を
   解析対象に含める。** demo で実測: 変更位置 11 件のはずが、前回の成果物を未追跡
   ファイルとして拾って 98 件になった。conductor 自身のリポジトリは `.gitignore` に
   `/.revidere/` を足したので大丈夫だが、**conductor がレビューする他のリポジトリは
   対象外**。本来の直し場所は revidere 側 (自分のディレクトリを未追跡の走査から
   外す) なので §8 に回してある。当面の回避は対象リポジトリの `.gitignore` か
   `.git/info/exclude` に `.revidere/` を書くこと。
2. **`~/.cargo/bin/revidere` が古いと、conductor が読めない成果物を書く。** conductor は
   `crates/revidere` (ソース) にリンクし、解析は PATH の `revidere` を起動するので、
   この 2 つが揃っていないと「解析は成功したのに読めない」になる。
   `cargo install --path crates/revidere-cli` で揃えること。

### 実機を見てからの手直し (2026-08-06)

初回の実装を実機で触ってもらって出た指摘。3 件を直した。

1. **右列に構文の色が無い** — 通常の Viewer は `ViewerState` にファイル全体の
   ハイライト結果をキャッシュしているが、2 列ビューが描くのは「開いているファイル」
   ではなく diff の行そのものなので、その経路には乗れない。ブロック単位で
   `syntect` をかけ直す形にした。構文定義の解決 (`viewer::find_syntax`) と
   タブ展開 (`ui::viewer_panel::expand_tabs_at`) は Viewer と同じ実装を借りている
   — 拡張子の対応表を 2 つ持つと、片方だけ直したときに同じファイルが場所によって
   色付いたり付かなかったりする。
   ハンクは飛び飛びの断片なので、パーサの状態はファイル先頭からの続きにはならない
   (文字列やコメントの途中から始まると数行ずれる)。全部を無彩色にするよりは読める、
   という割り切り。
2. **`+`/`-` の記号を落とし、追加・削除は背景色にした** — 1 の結果として前景色は
   構文が使うので、追加・削除は前景では表せない。背景を行末まで塗る GitHub 式に
   して、記号は落とした。記号・背景・帯 (▌ = その節の持ち物) の 3 つが並ぶと
   どれが何かが読めなくなる。
3. **左列のクリックでビューが閉じていた** — 描画は `Focus::Revidere` で
   短絡しているのにマウスだけがアコーディオンのカラム判定に流れていたので、
   左列のクリックが Explorer/Viewer のハンドラに当たり、そこがフォーカスを
   移していた。`main_area` の中はこのビュー専用のハンドラで受け、フォーカスは
   一切動かさないようにした (出るのは Esc だけ)。`main_area` の外 (タイトル・
   メニュー・worktree ストリップ) はこのビューでも出ているので素通しのまま。
   左列のクリックで節を選び、ホイールは左列で節送り・右列で 3 行スクロール。

4. **総括 (`Overview` と `Impact`) を丸ごと落としていた** — revidere 自身の TUI が
   1 ページ目に出しているもの。意図した省略ではなく、単に節だけを描いていた。
   **別画面**にした。最初は右列の先頭に混ぜたが、総括は結構な量があり、読むのは
   最初の一度きりなのに、そのあとずっと縦を取り続ける。GitHub が PR の説明と
   Files changed を分けているのと同じ切り分けにした。ビューを開いた直後は総括。
   **切り替えは行き先ごとに別のキー** (`o` = 総括へ、`d` = 節と diff へ) で、
   1 つのキーで交互に切り替える形は採らなかった — 押した結果がいまどちらを
   出しているかに依存すると、キーの割り当てを説明するのも試すのも面倒になる。
   スクロール位置はそれぞれ別に持つので、行き来しても読みかけの場所が残る。
   機能への影響は事実と推測をタグで分けて出す (推測を事実の顔で出されると、
   確かめずに信じてしまう)。
5. **折り返しが文字数で切っていて、日本語の本文が枠から溢れて消えていた** — 4 の
   確認中に見つけた。全角は 1 文字 2 列なので、幅 133 で切ると 266 列に伸び、
   はみ出した分は `Paragraph` に黙って落とされる。節の本文はどれも後半が読めない
   状態だった。表示幅で切るように直し、全角と混在の 2 件をテストで固定した。
6. **解析の状態が worktree ストリップに常時出るようにした** — 解析は数分かかり、
   複数の worktree で同時に走る。終わったことはステータス行に 1 度出るだけなので、
   どれが終わったのかも、どれが古いのかも後から分からなかった。チップの末尾に
   1 文字: スピナー = 実行中、`✓` = いまの HEAD を見て作られている、`!` = その後に
   コミットが載っている。解析していない worktree には何も出さない (印が付いていない
   ことに意味を持たせる)。

   古さの判定は「成果物のファイルが HEAD コミットより前に書かれたか」。解析時の
   commit id をどこにも書き残していないので突き合わせられないが、commit / amend /
   rebase / merge はどれも新しい committer 時刻を刻むので、前へ進む操作は全部
   捕まる。**捕まらないのは古いコミットへ戻したとき (checkout / reset --hard) だけ。**
   時刻で見る利点もあって、端末から直接 revidere を走らせた成果物も、conductor を
   再起動したあとも同じように判定できる — conductor 側が覚えている必要がない。
   `WorktreeInfo` に `head_time` を足したが、repo はもう開いているので追加のコストは
   ほぼ無い。

   スピナーを回すあいだの tick は、Claude の待機パルスと同じ 80ms にしてある。
   worktree 作成 (数秒) と同じ 60fps で数分回すのは割に合わない。

右列は `syntect` と折り返しを毎フレームやり直すには重いので、幅・テーマ・成果物
のどれかが変わったときだけ組み直すキャッシュを `RevidereState` に持たせた。

pty でマウスを注入して確認したこと (200x50 と 200x22):

- 左列 3 行目のクリックで選択が 1 番目から 3 番目へ移り、ビューは開いたまま
- 右列でのホイール 2 段 = 6 行スクロール
- Esc で Explorer に戻る
- 追加行に背景色 (`48;2;20;60;20`)、行の途中で前景色が切り替わる (構文トークン)

## 8. revidere を同梱する (2026-08-09)

外部依存として公開するのをやめ、conductor のリポジトリに取り込んだ。

- `crates/revidere` (成果物の型と読む順) / `crates/revidere-cli` (解析) /
  `crates/revidere-fixtures` (テストの骨組み) をワークスペースのメンバーにする
- `crates/revidere-view` は持ち込まない。同じ役目を `Focus::Revidere` の
  2 列ビューが引き受けているので、同じ画面を 1 リポジトリに 2 つ抱えない
- 解析は `conductor revidere analyze` として本体のバイナリに入れた。
  `mcp-serve` や `cc-hook` と同じ理由で、別の成果物にすると必ずずれる。
  PATH 探索が消えるので、リリースの tarball だけで AI レビューが動く
- 実装は `crates/revidere-cli` のライブラリ側にあり、`revidere` バイナリと
  `conductor revidere` の両方が同じ `run()` を通る
- 子プロセスとして起こすのは変えない。中断がプロセスの kill で済み、その先の
  AI コマンドまで確実に道連れにできるため

残っているもの:

- **`.revidere/` を自分の未追跡走査から外す** (§7 の 1。実害が一番大きい)
- `verify` が未追跡のシンボリックリンクで数え違える (§5.4)
