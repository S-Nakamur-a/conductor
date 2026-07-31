# Claude パネル scroll-up のネイティブ完全一致 — テスト設計

**ステータス**: 設計のみ（実装なし）
**対象バージョン**: Claude Code v2.1.220 / conductor 現在の HEAD
**日付**: 2026-07-31

---

## 0. 3行サマリ

1. scroll-up は**必ず** reflow view（`.jsonl` からの自前再描画）に入る。したがって「完全一致」とは **Claude Code のレンダラを Rust で再実装し、それを証明すること**である。
2. ネイティブは alt-screen を使わず、**単語ごとに絶対列(CHA `ESC[nG`)を宣言して** pre-wrap 済みの行を emit する。つまり**正解の列番号はバイト列に書いてある** — 幅モデルを推定する必要がない。
3. `claude --resume <uuid>` + `pty.fork` で、**API 課金ゼロ・バイト単位で決定的に**正解データを採取できる。しかも**手書き合成 `.jsonl` でも動く**ので、任意のコンテンツ種別の正解を生成できる。→ 人手採取はほぼ不要（§5）。

---

## 1. 現状把握

### 1.1 scroll-up の経路

Claude パネルで上スクロールすると、初回で**必ず** reflow view にハイジャックされる。

| 入口 | file:line |
|---|---|
| キーボード `ScrollbackUp` | `src/event/mod.rs:477-487` |
| キーボード `ScrollbackTop` | `src/event/mod.rs:513-521` |
| マウスホイール | `src/event/mouse/scroll.rs:255-262` |

```rust
// src/event/mod.rs:481-487
if app.focus == Focus::TerminalClaude
    && app.terminal.scroll_claude == 0
    && !app.reflow.active
{ app.open_reflow(); return true; }
```

**唯一の例外**: worktree が grabbed のときのみ、マウスホイールが hijack をスキップし vt100 スクロールバックに入る（`src/event/mouse/scroll.rs:257` の `!app.is_selected_worktree_grabbed()`）。キーボードは grabbed 時は全ブロック（`src/event/mod.rs:409-415`）。

→ **テスト対象は実質 100% が reflow view (`src/ui/reflow_view/`)。**

### 1.2 レンダリング経路

```
Claude セッション .jsonl
  → src/claude_log/{schema,convert,session}.rs   … パース＋正規化
  → LogEntry / DisplayBlock (src/claude_log/model.rs)
  → src/ui/reflow_view/build.rs:23 build_lines()  … 幅変更時のみ再構築
      └ src/ui/markdown/ (MarkdownFlavor::Transcript)  … 本文の md レンダリング
  → Vec<Line<'static>> を app.reflow.cached_lines にキャッシュ
  → src/ui/reflow_view/render.rs:112-127  … buf.set_line で Buffer に直書き
```

主要関数:

| 役割 | file:line |
|---|---|
| 行構築 | `src/ui/reflow_view/build.rs:23` `build_lines` |
| tool_result 描画 | `src/ui/reflow_view/build.rs:161` `render_tool_result` |
| マーカー付与 | `src/ui/reflow_view/helpers.rs:16` `with_marker` |
| 幅切り詰め | `src/ui/reflow_view/helpers.rs:60` `truncate_to_width` |
| 描画 | `src/ui/reflow_view/render.rs:22` `render` |
| スクロール算術 | `src/event/reflow.rs:14` `clamp_scroll` / `:22` `at_bottom` |
| キー処理 | `src/event/reflow_key.rs:21` `handle_reflow_key` |
| 折り返し | `src/ui/markdown/wrap.rs:60` `wrap_cells` |
| ANSI 除去 | `src/claude_log/convert.rs:34` `sanitize_preview_line` |
| tool 要約 | `src/claude_log/convert.rs:12` `summarise_tool_input` |
| user ターン正規化 | `src/claude_log/convert.rs:139` `normalise_user_text` |
| 起動 | `src/app/reflow.rs:97-116` `open_reflow`（早期 return 2箇所） |

### 1.3 既存テストのカバレッジ

`cargo test` → **726 passed / 0 failed / 1 ignored**。`tests/` ディレクトリなし、golden/snapshot ファイル **0件**。

| カバー済み | 未カバー |
|---|---|
| `pad_glyph_to` / `with_marker` / `truncate_to_width`（`src/ui/reflow_view/tests.rs`、10本） | **レンダリング済みグリッドの assert（0件）** |
| `clamp_scroll` / `at_bottom` / `sweep_progress`（`src/event/reflow.rs:66-105`） | **ネイティブとの一致検証（0件）** |
| `sanitize_preview_line` の ANSI/OSC/タブ（`src/claude_log/tests.rs:195-213`） | `build_lines` 全体（`&mut App` 依存でテスト不能） |
| markdown レンダリング（`src/ui/markdown/tests/`） | `render` 全体（同上 + `Instant::now()`） |
| — | Unicode 境界（ZWJ・結合文字・VS） |
| — | リサイズ時のスクロール位置保持 |
| — | tool_result の truncate 境界 |

**`TestBackend` は既に実使用されている**（`src/ui/tab_bar.rs:301,325,346,353` が `terminal.backend().buffer().clone()` でセル比較、`src/ui/viewer_panel/markdown_view.rs:223-247` も同様）→ **新規依存ゼロでグリッド assert が書ける。**

### 1.4 ネイティブの実描画（実測）

`claude --resume <uuid>` を pty(100x30) で走らせ master の生バイトを採取。

```
# assistant テキスト
\x1b[38;2;255;255;255m⏺\x1b[3G\x1b[39mThe\x1b[7Gdiff\x1b[12Gis\x1b[15Ga\x1b[17Gsingle-line\r\r\n\x1b[3Gchanged,\x1b[12Gno

# tool_use（--verbose）
\x1b[38;2;78;186;101m⏺\x1b[3G\x1b[39m\x1b[1mBash\x1b[22m(ls\x1b[11G-la\x1b[15G/tmp)

# tool_result（--verbose）— 全12行、cap なし
\x1b[38;2;153;153;153m \x1b[3G⎿\x1b[5G\xa0\x1b[39mline1\r\r\n\x1b[6Gline2\r\r\n … \x1b[6Gline12

# tool_result（--verbose なし）— 1行に畳む
\x1b[3G\x1b[38;2;153;153;153mRead\x1b[8G\x1b[1m1\x1b[22m file\x1b[15G(ctrl+o\x1b[23Gto\x1b[26Gexpand)\x1b[39m

# user ターン — 全幅の背景ブロック
\x1b[48;2;55;55;55m  \x1b[38;2;255;255;255mInvestigate per the method…\x1b[39m<空白パディング>\x1b[49m

# 動的フレームの境界（seam の整列マーカー）
\x1b[38;2;136;136;136m────…────\x1b[39m\r\r\n❯\xa0
```

読み取れる性質:
- **alt-screen に入らない**（`ESC[?1049h`/`[?47h`/`[?1047h` = 0、`ESC[2J` = 0、`ESC[3J` = 0、CUP = 0）。行送りは `\r\r\n` → 履歴は端末の scrollback に流れる。
- **単語ごとに絶対列 CHA を発行**。端末の autowrap に頼らず**自分で折り返して確定した行を emit** する（分割不能な `X`×200 をネイティブ側が3行に割って出力することを実測）。
- ガターは 2 列（本文は col 3）、tool_result 本文は col 6。

### 1.5 台帳 — 「寄せるために頑張っていた」既存ロジックと、一致しない理由

| # | 項目 | ネイティブ（実測） | conductor | file:line | 一致しない理由（仮説→実測で確定したもの） |
|---|---|---|---|---|---|
| D1 | tool_result の行数 | verbose: **全行**／既定: **1行に畳む** | **4行 cap + `… +N lines`** | `model.rs:45`, `build.rs:196` | **確定**: 独自の折り畳み規則。ネイティブは二重描画を持つ（verbose 切替） |
| D2 | tool_result の各行 | **折り返す**（col6..幅） | **`…` で切り捨て** | `build.rs:183`, `helpers.rs:60` | **確定**: 実装差 |
| D3 | user ターン | **全幅 bg RGB(55,55,55)** + 2列インデント + 白文字 | `> ` + coral | `glyphs.rs:24`, `build.rs:71` | **確定**: 表現形式が別物 |
| D4 | 本文色 | 純白 **RGB(255,255,255)** | `Color::Reset`（端末既定） | `palette.rs:17` | **意図的逸脱**。コメントに「hardcoded pure white read as harsh」と明記 |
| D5 | assistant/tool グリフ | `⏺` U+23FA | `*` | `glyphs.rs:21` | **意図的逸脱**。端末2列描画による bleed 対策。ネイティブは**絶対列指定で同じ問題を回避している** |
| D6 | thinking グリフ | **`✻` U+273B**（実セッションで確認。畳んだ形は `✻ Crunched for 7s` 等、動詞はランダム） | `*` | `glyphs.rs:31` | 意図的逸脱。**`glyphs.rs:31` のコメント「Claude Code uses ✻」は正しい**（`∴` は合成 fixture 由来の誤検出だった） |
| D7 | tool_result グリフ | `⎿` U+23BF | `└` U+2514 | `glyphs.rs:28` | 意図的逸脱（同上） |
| D8 | 有効幅 | **全幅** | **width − 1** | `render.rs:36-39` | **意図的逸脱**。本文中の絵文字の under-count 1列ぶんを吸収する安全余白 |
| D9 | tool_result 本文色 | default fg（`ESC[39m`）。灰は `⎿` のみ | 全体 dim(INACTIVE) | `build.rs:192` | 実装差 |
| D10 | tool 引数の要約 | ツール別（`Bash(ls -la /tmp)`, `Read 1 file`） | 4キー先頭一致のみ | `convert.rs:12-25` | 実装差 |
| D11 | エントリ間の空行 | 文脈依存 | **無条件で1行** | `build.rs:152` | 実装差 |
| D12 | 見出し/リスト | 未突合 | Transcript flavor（見出し=太字のみ、`- `、マーカー色=fg） | `markdown/render.rs:35-39,96-113` | **未検証**。コメントの主張のみ |
| D13 | tool_result の ANSI | 保持（色付き diff 等） | **全除去** | `convert.rs:34-76` | 実装差 |
| D14 | ガター幅 / 本文列 | col 3 / col 6 | col 3 / col 6 | `glyphs.rs:6`, `build.rs:171` | **✓ 一致** |
| D15 | tool bullet の色 | RGB(78,186,101) | `palette::SUCCESS` = RGB(78,186,101) | `palette.rs:19` | **✓ 一致** |
| D16 | 灰色トークン | RGB(153,153,153) | `palette::INACTIVE` = RGB(153,153,153) | `palette.rs:23` | **✓ 一致** |

**幅計算は差異要因ではない**（実測）。`unicode-width 0.2.0`(Rust) と `string-width@8.2.2`(Node) を全 1,112,064 コードポイントで突合した結果、相違は **379個のみ**（Hangul jungseong U+1161-11FF、Indic 母音記号、U+0600-0604 等の書式文字）。`👨‍👩‍👧‍👦` は**両方 2**、`👋🏽` も**両方 2**。East Asian Ambiguous・CJK・半角カナ・VS16・keycap・国旗はすべて一致。ASCII 圏で効く唯一の差は **TAB**（rust=1 / node=0）。

### 1.6 テストを書いた瞬間に落ちる既知欠陥（コード確認済み）

| # | 箇所 | 内容 |
|---|---|---|
| B1 | `helpers.rs:60-78` | `budget = max_cols − 1` を常に確保するため**ぴったり収まる文字列まで切る**。`truncate_to_width("hello", 5)` → `"hell…"` |
| B2 | `build.rs:88-96` | summary 非空の枝で `name` が無検査 push。長いツール名×狭幅で `summary_budget` が 0 に飽和し**行が幅を超える**（bleed 再発経路） |
| B3 | `build.rs:174,186-192` | `is_error` がコネクタ `└` だけを赤くし、本文は常に dim |
| B4 | `build.rs:171-183` | 幅 < 5 で `  └  ` 固定5列が truncate されず幅超過 |
| B5 | `render.rs:62-69` | 幅変更で `cached_lines` を作り直すが `scroll` は**生の行番号のまま** → 再折り返しで表示位置が飛ぶ |
| B6 | `app/reflow.rs:97-116` | session 未 pin / ログ未生成だと `open_reflow` が早期 return。A経路が実質到達不能なので **scroll-up が完全に無反応になる**（可用性バグ、parity 以前） |

---

## 2. 判定器（oracle）の設計

### 2.1 粒度の選択 → **セル単位グリッド。char 面と属性面を別ファイルに分ける**

| 候補 | 判定 | 理由 |
|---|---|---|
| 生 ANSI 文字列 | ✗（conductor 側） | reflow_view は ANSI を出力しない。`render.rs:112-127` は `buf.set_line` で ratatui `Buffer` に直書き。ANSI 化は crossterm backend の仕事でテスト対象外の層 |
| **セル単位グリッド** | **◎ 推奨** | ユーザーが見るものと 1:1。bleed（幅超過）・全角の割れ・折り返し位置・末尾スペースが全部見える。`TestBackend` で完全に決定論的 |
| `Line` + `Span` 構造 | △ | 過剰に厳しい。`cells_to_line`(`wrap.rs:34-58`) の coalesce 規則が変わるだけで落ちるが画面は変わらない。**偽陽性の温床** |
| 構造的アサーション | ○（併用） | 不変条件層でのみ使う。「幅を超えない」「論理行数==視覚行数」は property に向く |

**char 面 / 属性面を分離する理由**: 1枚に混ぜるとテーマ調整（色）とレイアウト回帰（バグ）が同じ差分に埋もれ、golden 更新時に人間がレビューを放棄する。char 面は各行を `|` で囲む（**末尾スペースの有無が bleed の主要シグナル**）。

**ネイティブ側の非対称に注意**: ネイティブ真値は生 ANSI（列宣言込み）、conductor 出力は `Buffer`。共通形式に落とすには生 ANSI → `vt100`（既に依存にある `vt100 = "0.15"`）→ セルグリッド、conductor `Buffer` → セルグリッド、で揃える。**ただしグリッド化するとネイティブが宣言した列番号という情報が失われる**ので、CHA 列は別途パースして保持する（§2.3）。

### 2.2 seam oracle — ネイティブ真値を**プロセス内で**得る

ハイジャックは `scroll_claude == 0`（`event/mod.rs:481`）、つまり**ライブ PTY の最下部を見ている瞬間**に発火する。直前フレームの vt100 グリッドは**ネイティブ自身が描いた本物**。よって:

> **reflow view の該当行 == ハイジャック直前の vt100 グリッドの対応行**

が Node もスクショも無しに機械判定できる。一致しなければユーザーは**必ず画面の飛びとして知覚する**（同じ瞬間の同じ内容だから）ので、これが体験上の「完全一致」の定義そのもの。

**成立条件（実測で PASS）**: ネイティブが alt-screen を使わない → 確認済み。

**必須の修正2点**（これがないと必ず落ちる）:

- **修正A — 単純な「末尾H行」比較は不成立。** ハイジャック時点の vt100 末尾は transcript ではなく**ライブの動的フレーム**（`────` / `❯` / `────` / `⏵⏵ auto mode on … N tokens` / `● high · /effort`）。reflow はこれを一切描かない。しかも動的フレーム行数は幅で変わる（幅100 で5行、幅60 でフッタが折返して7行）。さらに reflow は `pending_bottom` で「最後の内容行=最下行」に固定（`render.rs:74-80`）。
  → **全幅 `────`（`ESC[38;2;136;136;136m` + `─`×幅）を動的フレーム開始マーカーとして検出し、その直前の transcript 行で整列する。**
- **修正B — リサイズ後は oracle 無効。** ネイティブは自分で折り返して確定した行を CHA 付きで emit するので、vt100 に残る過去行は **emit 当時の幅で焼き付く**。reflow は現在幅で組み直す（`render.rs:62-69`）。既存コードもこれを認識している（`src/pty_manager/spawn.rs:186-192`「Claude and the transient editor repaint in place at a fixed width, so replay would cost memory without ever reflowing — skip it for them.」）。
  → **seam の適用条件は「最後の resize 以降」。**

**射程の上限**: vt100 パーサのスクロールバックは **1,000 行**。`src/pty_manager/spawn.rs:180-184` が `vt100::Parser::new(rows, cols, self.inactive_scrollback)`、`src/config/sections.rs:71` で `inactive_scrollback: 1000`。`activate_session`(`mod.rs:190-200`) が書く `active_scrollback: 10000` は `session.max_buffer_lines` と `buffer_limits`（プレーンテキストの `output_buffer`）にしか効かず**パーサには反映されない**。`[terminal] inactive_scrollback` で変更可。

→ **seam oracle が守れるのは「ライブ tail に接する 1,000 行以内・リサイズなし」だけ。** それより古い履歴（そもそも vt100 に無いから reflow を作った）は §2.3 の採取 oracle で覆う。

### 2.3 採取 oracle — `--resume` 再描画を正解データにする

`claude --resume <uuid>` を pty で走らせると**過去の会話をそのまま端末に描き直す**。API 課金なし、resume だけでは `.jsonl` に追記されない（mtime のみ更新）。

**決定性は実測でバイト完全一致**:

| 条件 | 結果 |
|---|---|
| 100x40 ×3回 | 5008B **完全一致** |
| 60x40 ×2 / 80x40 ×2 / 120x40 ×2 | すべて**完全一致** |
| tool_use fixture ×2 / `--verbose` ×2 | **完全一致** |

**最大の落とし穴 — `--verbose` の有無で別物**:

```
[resume 既定]                            [resume --verbose]
❯ run a command                          ❯ run a command
  Thought for 1s (ctrl+o to expand)      ∴ I should think about this carefully...
⏺ Running it now.                        ⏺ Running it now.
  Listed 1 directory (ctrl+o to expand)  ⏺ Bash(ls -la /tmp)
⏺ Done.                                    ⎿  line1 … line12（全12行）
                                         ⏺ Done.
```

**どちらを正解とするかは仕様判断**（§6 Q2）。

**合成 `.jsonl` が使える**: 手書き `.jsonl` を `~/.claude/projects/<cwd の / を - に置換>/<uuid>.jsonl` に置けばネイティブが読んで描画する（実測済み）。→ **任意のコンテンツ種別の正解を、実会話なしで生成できる。** これが人手採取をほぼ不要にする鍵。

**CHA 列の抽出**: ネイティブは折り返し位置を `ESC[nG` で宣言するので、**「ネイティブが置いた列 == conductor が置いた列」**を直接比較できる。幅モデルを一致させる必要はない。

### 2.4 差分の可視化

char 面はこの形式で出す（列尺は 10 列ごと、`|` で行端を明示）:

```
=== char plane: tool_result / width=40 / native(--verbose) vs conductor ===
          1         2         3         4
 col ....5....0....5....0....5....0....5....0
 r12 native   | ⏺ Bash(ls -la /tmp)                   |
 r12 conductor| * Bash(ls -la /tmp)                   |
 r12 diff     | ^                                     |
                ↑ col1: '⏺'(U+23FA) != '*'(U+002A)

 r13 native   |  ⎿ line1                              |
 r13 conductor|  └  line1                             |
 r13 diff     |  ^^^^                                 |
                ↑ col2: '⎿' != '└' / col3-5: 本文開始 col6 vs col6 (一致)

 r17 native   |  ⎿ line5                              |
 r17 conductor|  … +8 lines                           |
 r17 diff     |  ^^^^^^^^^^^                          |
                ↑ D1: conductor は4行 cap（native は全12行）
```

全角の桁ズレは**セル占有を明示**する（`▓` = 全角の後続セル）:

```
 r04 native   | 日本語のテキストです                   |
 r04 cells    | 日▓本▓語▓の▓テ▓キ▓ス▓ト▓で▓す▓      |
 r04 conductor| 日本語のテキストで…                    |
 r04 diff     |                    ^^^^                |
                ↑ col18: native は col20 まで、conductor は col18 で truncate（B1）
```

属性面は別ファイルに、変化点だけを列挙する（全セル出すと読めない）:

```
=== attr plane: r12 ===
 col  native                    conductor
   0  fg=#4EBA65               fg=#4EBA65        ok
   2  fg=default bold          fg=default bold   ok
   6  fg=default               fg=#999999        DIFF  ← D9
```

実装: `similar`（`inline` feature 付きで**既に依存にある**）で行内差分を取り、`^` マーカーを生成する。

---

## 3. テストケース網羅

凡例: **[P]**=不変条件(property) / **[G]**=自己 golden / **[N]**=ネイティブ台帳比較 / **[S]**=seam

### 3.1 基本表示

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| プレーン1行 assistant [G][N] | マーカー + 本文が col3 から | char 面 golden | 低 |
| 複数行テキスト [G] | 2行目以降が2列インデント | char 面 | `with_marker` の `i==0` 分岐(`helpers.rs:28`) |
| 本文中の空行 [P][N] | md が空行を保持するか | 行数 assert | `MdBlock::Blank` の扱い |
| blocks が空の entry [P] | `build.rs:152` の空行だけ残る | 行数 == 1 | 誰も試さない |
| entries 0件 [P] | `total_lines == 0`、パニックなし | 数値 | 既存カバーあり |
| loading 状態 [G] | 中央寄せプレースホルダ | char 面 | `render.rs:48-59` の x 座標算術、幅0時 |
| 連続 tool_use 間の空行 [N] | ネイティブは入れるか | 台帳 D11 | `build.rs:152` は無条件 |
| **`open_reflow` 失敗** [P] | `app/reflow.rs:97-116` の2つの早期 return | 状態遷移 assert | **B6 可用性バグ。scroll-up が無反応になる** |

### 3.2 折り返し境界（最優先。`wrap_cells` と `truncate_to_width` の**両方**に同じ境界を当てる）

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| **幅ちょうど (w == width)** [P] | 切らない・折らない | `…` なし、1行 | **B1 で現在落ちる** |
| 幅 −1 [P] | 1行 | char 面 | 低 |
| 幅 +1 [P] | 2行、2行目1文字 | char 面 | `cur_w + cw > width && !cur.is_empty()`(`wrap.rs:92,142,149`) |
| 全角が境界に半分だけ入る [P] | 全角を割らない | 前行が width−1 で終わる | `char_width` 2 の飛び越え |
| 単語長 == width ちょうど [P] | `word_w > width` が false → 通常枝 | 1行 | `wrap.rs:135` の `>` vs `>=` |
| 単語 > width（ハード分割） [P] | セル境界で分割 | 各行 ≤ width | `wrap.rs:135-147` |
| 連続する長行 × N [P] | 全行 ≤ body_width | property | 累積誤差 |
| 行末スペースの消失 [N] | `wrap.rs:110-124` はスペースを落とす | 台帳 | 空白位置ズレ |
| body_width == 0（panel ≤ 2） [P] | `wrap_cells` は `.max(1)` で救われるが `truncate_to_width(_,0)` は `""` | 幅超えなし | 極小幅は誰も試さない |
| width 1,2,3 / height 0,1 [P] | 早期 return と `.max(1)` | パニックなし | `render.rs:23,36-39` |
| **width−1 マージンによる恒常ズレ** [N] | ネイティブ比で常に1列狭い | 台帳 D8 | `render.rs:36-39` |
| **ネイティブ宣言列との一致** [N] | `ESC[nG` の n == conductor の列 | 列番号の直接比較 | **これが折り返し parity の本命 oracle** |

### 3.3 Unicode

**方針の訂正**: 幅モデルは実測で一致しているため（§1.5）、ネイティブ一致を oracle にできる。自己整合性が要るのは**クラスタ境界の扱い**のみ。

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| 全角 CJK [P][N] | 幅2で折り返す | char 面 + 列比較 | 中 |
| **ZWJ 絵文字** [P][N] | (i)パニックなし (ii)幅超えなし (iii)**ZWJ 途中で切らない** | 不変条件 | **`truncate_to_width` は `char_indices` で切る = 宙ぶらりんの ZWJ が残る** |
| 肌色修飾子 `👋🏽` [P] | 同上 | 同上 | 同上 |
| VS16 / VS15 [P] | 異体字セレクタ幅0 | 幅計算が壊れない | `glyphs.rs` の教訓そのもの |
| 結合文字（`e`+U+0301） [P] | 幅1・分離しない | 不変条件 | `char_indices` で分離される |
| East Asian Ambiguous [N] | **実測で両者一致** | 列比較 | 端末設定依存は残るが実装差はない |
| Hangul jungseong / Indic 母音記号 [N] | **379個の既知相違に含まれる** | 台帳に明記 | rust=0 / node=1 |
| **TAB** [N] | rust=1 / node=0。`sanitize` は空白4つに展開(`convert.rs:70`) | 台帳 | assistant text は sanitize されない（下記） |
| 不正 UTF-8 の jsonl 行 [P] | パースが落ちない | Err か lossy | 実ログコーパスで自動検出 |
| RTL / bidi [P] | パニックなし・幅超えなし | 不変条件 | 論理順序と表示順序の乖離 |

### 3.4 ANSI

**重要な非対称性**: `sanitize_preview_line`(`convert.rs:34-76`) は tool_result と local-command-stdout にしか適用されず、**assistant text はそのまま markdown へ流れる**。

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| tool_result の SGR / OSC8(BEL,ST) / タブ [P] | 剥がされる | 文字列 | 既存 `claude_log/tests.rs:195-213` でカバー済 |
| **CSI 未終端** [P] | 残りを食い切る | 無限ループなし | `convert.rs:44-48` |
| 単独 ESC・不明エスケープ形 [P] | 次バイトも捨てる | 文字列 | `convert.rs:66-68` |
| **assistant text 中の ANSI** [P] | sanitize されず markdown へ | 現挙動を固定 or バグ判定 | **未検証の穴** |
| **折り返しをまたぐスタイル継続** [P][G] | Cell 単位なので継続する | 属性面 golden で2行目==1行目 | `spans_to_cells`→`cells_to_line` 往復(`wrap.rs:25-58`) |
| リセット直後の折り返し [G] | span 分割位置 | 属性面 | `cells_to_line` の coalesce |
| tool_result の色保持 [N] | ネイティブは色付き diff を出すか | 台帳 D13 | conductor は全除去 |

### 3.5 コンテンツ種別

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| user 入力 [N] | **全幅 bg ブロック vs `> `** | 台帳 D3・大差分 | **表現が別物** |
| `<command-name>` 正常 [P] | `/foo bar` に正規化 | 文字列 | `convert.rs:147-158` |
| **`<command-name>` 未終端** [P] | 正規化せず素通し | 全文が残る | 意図的に守っている挙動、回帰しやすい |
| `<local-command-stdout>` [P] | 展開 + sanitize | 文字列 | 中 |
| `<system-reminder>` 全除去で空 [P] | エントリが消え空行だけ残る | 行数 | `build.rs:152` との相互作用 |
| **本文中の `<system-reminder>` 言及** [P] | `strip_tag_spans` は位置無関係に消す | 現挙動 | `convert.rs:166` は先頭限定でない = 仕様バグ候補 |
| tool_use（4キー各々 + 該当なし） [P] | 要約抽出 | 文字列 | 既存テストあり |
| **tool_use: name 超長 × 狭幅** [P] | 幅を超えない | 不変条件 | **B2 で現在落ちる** |
| tool_use: ツール別表現 [N] | `Bash(...)`, `Read 1 file` 等 | 台帳 D10 | **ネイティブ一致の主戦場** |
| tool_use の bold 範囲 [N] | name のみ bold、`(args)` は非 bold | 属性面 | conductor は `(args)` を dim（`build.rs:95`） |
| tool_result 正常 [N] | col6 本文・全行 vs 4行 cap | 台帳 D1/D2 | **最大の差分** |
| tool_result `total==0` [P] | `(no content)` | 文字列 | `build.rs:176-179` |
| **tool_result: is_error** [P] | 本文も赤か | 属性面 | **B3 で現在落ちる** |
| diff 出力 [N] | ネイティブは色付き diff | 台帳・大差分 | 高 |
| コードブロック [G] | syntect + hard wrap | char + 属性面 | **syntect テーマ固定が必須**（さもなくば golden がぶれる） |
| thinking（空 text） [P] | ヘッダのみ | 行数1 | `build.rs:121` |
| thinking（長文） [G] | dim italic + 2列インデント | 属性面 | `build.rs:134-145` が span style を**全上書き**しコードブロック色を潰す |
| thinking の畳み込み [N] | 既定は `Thought for 1s (ctrl+o to expand)` | 台帳 | verbose 依存 |
| **todo リスト** [N] | Transcript flavor の `- `/`[x]`(`markdown/render.rs:96-113`) vs ネイティブ | 台帳 D12 | 未検証の近似面 |
| 見出し [N] | Transcript flavor は太字のみ(`render.rs:35-39`) | 台帳 D12 | 同上 |
| 表 [N] | `render_table` にネイティブ対応物があるか | 台帳 D12 | 同上 |
| エラー表示 [N] | is_error の見せ方 | 台帳 | 未検証 |

### 3.6 スクロール

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| 上端/下端の飽和、total<inner、total==inner、scroll>total [P] | clamp | 数値 | **既存カバー済**(`event/reflow.rs:66-105`) |
| 1行スクロール [P] | ±1 | 数値 | 低 |
| ページ単位 [P][N] | `mod.rs:489` は `size/2`（**半ページ**） | 台帳 | ネイティブのページ量と比較 |
| スクロール中の新規出力 [P] | entries は open 時 snapshot → **追記されない** | 現挙動を記録 | ネイティブは追従する。台帳 or バグ |
| 長い履歴の途中復帰 [P] | 4.8MB / 10万行の性能・メモリ | 実測 | `cached_lines` が全行 `Vec<Line>` |
| **`/clear`・`/resume` 後の内容不一致** [S] | 別セッションIDに回転し**回転前の transcript が出る**（`app/reflow.rs:90-96` に明記） | seam で検出 | **幅テストでは絶対に捕まらない内容不一致。seam の独自価値** |

### 3.7 リサイズ

| ケース | 何を検証 | 期待出力の形 | なぜ壊れやすいか |
|---|---|---|---|
| **幅変更でスクロール位置が飛ぶ** [P] | `render.rs:62-69` は `cached_lines` を作り直すが `scroll` は行番号のまま | **B5 の破綻を明示的に assert し「直したら反転させる」とコメント** | **高。アンカー `(entry_idx, block_idx, offset)` 方式にしないと直らない** |
| width 1 と 2 が同じキャッシュ [P] | `render_area.width` が両方1 → 正しく共有 | 数値 | 低 |
| **幅 W1→W2→W1 の冪等性** [P] | 初回と一致 | char 面完全一致 | markdown cache キー `{ei}:{bi}`(`build.rs:57,122`) が**幅を含まない** — 衝突を要検証 |
| 高さのみ変更 [P] | 再ビルドなし、clamp のみ | 数値 | 中 |
| リサイズ後の seam 無効化 [S] | oracle を適用しないこと | ガード assert | 修正B |

### 3.8 省略・切り詰め

| ケース | 何を検証 | 期待出力の形 |
|---|---|---|
| `total == 4`（cap ちょうど） [P] | `… +N` を出さない | 行数（`build.rs:196` は `>` なので正しい） |
| `total == 5` [P] | `… +1 lines` | 文字列 |
| `+1 lines` の単複 [N] | ネイティブは `+1 line` か | 台帳 |
| `… +N lines` 自体が幅超過 [P] | truncate 済 | 幅 assert |
| **幅 < 5 で `  └  ` が幅超過** [P] | 固定5列が truncate されない | 不変条件 | **B4 で現在落ちる** |
| tool_result 各行の truncate [N] | ネイティブは折り返す | 台帳 D2 の主要差分 |
| `(ctrl+o to expand)` [N] | 既定 resume の畳み表現 | 台帳 D1 |

### 3.9 実ログ 27本を使った不変条件 fuzzing（**最も費用対効果が高い**）

期待出力を**一切書かずに**、27本 × 幅 {20,40,60,80,120,200} で `build_lines` を回し次を assert:

1. 全行の `display_width` ≤ `render_area.width`（**bleed の直接検出**）
2. パニックしない・無限ループしない
3. `total_lines == cached_lines.len()`
4. 同一幅で2回ビルドして一致（決定性）
5. 幅 W1→W2→W1 の往復で初回と一致（冪等性）

4.8MB のログには人間が思いつかない入力（壊れた UTF-8、巨大 tool_result、ネストしたコードフェンス、ZWJ 絵文字）が確実に含まれている。

**成立条件**: ログを**リポジトリに入れない**。`CONDUCTOR_TRANSCRIPT_CORPUS=<dir>` で参照し未設定なら skip。パス・トークン・プロンプトが混入するのでコミット不可。

---

## 4. テストプラン（レベルと規模）

| レベル | 対象 | 前提 | 目安 |
|---|---|---|---|
| 単体（純粋） | `truncate_to_width` / `pad_glyph_to` / `wrap_cells` / `sanitize_preview_line` / `summarise_tool_input` / `normalise_user_text` | なし | 60〜80 |
| Property (`proptest` 追加) | 任意 String × width 1..200 で「wrap 行 ≤ width」「truncate ≤ max_cols」「wrap 連結 == 元文字列（スペース除く）」 | 新規依存1 | 6〜10 |
| **コーパス不変条件** | 実ログ27本 × 幅6種 → §3.9 の5項目 | env var ゲート | 5 assert × 162 組 |
| 結合(build 層) | 手書き最小 `.jsonl` → char 面ダンプ | **R1 の refactor** | fixture 15〜20 × 幅3 |
| 結合(render 層) | `TestBackend` → char 面 + 属性面 | **R2 の refactor** | 10〜15 |
| **ネイティブ台帳** | `--resume` 採取バイト列 → vt100 グリッド + CHA 列 vs conductor。**台帳外差分ゼロ**を判定 | 採取ハーネス | 幅ごと 3〜5 |
| seam | 動的フレーム除去 → 末尾 transcript 行で整列 → 上方向N行比較 | 修正A/B | 3〜5 |
| 状態遷移 | `open_reflow` の早期 return、ハイジャック条件 | `pty_manager` を trait 化 or 最小 App | 6〜8 |

### 4.1 テスタビリティ障害と必要な refactor

| # | 障害 | 対応 |
|---|---|---|
| **R1** | `build_lines(app: &mut App, width)` が `App` 全体依存（`build.rs:23`）。必要なのは `entries` / markdown cache / `theme` / `syntax_set` / `syntect_theme` の5つ | `BuildCtx` に束ねて分離。**golden 層もコーパス層もこれが前提。最初にやる。** |
| **R2** | `sweep` が `Instant::now()` 直参照（`render.rs:96`） | 進捗 `f64` を引数化 |
| R3 | 幅計算が散在（`helpers.rs:60`, `wrap.rs:17`, `build.rs:88,172`） | 単一入口に集約 |
| R4 | `truncate_to_width` が grapheme 非対応 | B1 の修正と同時に `unicode-segmentation` でクラスタ境界に |
| R5 | `scroll` が生の行番号 | アンカー方式（今回はテストで現状の破綻を記録するに留める） |

**syntect テーマの固定が必須**（`build.rs:66-67` がコードブロック色に効く）。`insta` の導入は不要（`std::fs` 比較 + `UPDATE_GOLDEN=1` で足りる）。

---

## 5. 【最重要】ネイティブ側から採取するデータ

### 5.1 結論 — **人手採取はほぼ不要**

`--resume` + `pty.fork` + **合成 `.jsonl`** の組み合わせで、**任意のコンテンツ種別の正解データを API 課金ゼロ・バイト単位で決定的に自動生成できる**（実測済み）。

採取ハーネス（実証済み、`scratchpad/altscreen_probe.py` / greta の `ptycap.py`）:

```python
import os, pty, fcntl, termios, struct, signal, time
pid, fd = pty.fork()
if pid == 0:
    os.chdir(CWD)                              # .jsonl の置き場を決める
    os.environ["TERM"] = "xterm-256color"
    os.environ["COLUMNS"], os.environ["LINES"] = str(COLS), str(ROWS)
    os.execv(CLAUDE, ["claude", "--resume", SESSION_UUID])   # + "--verbose"
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
# master を N 秒吸って killpg(SIGKILL)
```

合成 fixture の置き場: `~/.claude/projects/<cwd の / を - に置換した名前>/<uuid>.jsonl`

### 5.2 自動採取するもの（人手不要）

| # | 内容 | 幅 | 保存先 |
|---|---|---|---|
| A1 | assistant プレーンテキスト（短文・長文・空行入り） | 40/60/80/100/120/200 | `tests/golden/native/text.<w>.raw` |
| A2 | tool_use × 各ツール（Bash/Read/Edit/Write/Grep/Glob/Task/TodoWrite） | 同上 | `tool_use.<tool>.<w>.raw` |
| A3 | tool_result（1行 / 4行 / 5行 / 30行 / 空 / エラー） | 同上 | `tool_result.<n>.<w>.raw` |
| A4 | thinking（空 / 短 / 長） | 同上 | `thinking.<w>.raw` |
| A5 | user ターン（短文 / 長文 / 貼り付け複数行 / スラッシュコマンド） | 同上 | `user.<w>.raw` |
| A6 | markdown（見出し h1-h3 / 箇条書き / 番号付き / チェックボックス / 表 / 引用 / インラインコード） | 同上 | `md.<kind>.<w>.raw` |
| A7 | コードブロック（言語あり / なし / 幅超過行） | 同上 | `code.<w>.raw` |
| A8 | diff 出力を含む tool_result | 同上 | `diff.<w>.raw` |
| A9 | Unicode（CJK / ZWJ 絵文字 / 肌色修飾子 / 結合文字 / VS16 / Ambiguous / RTL / TAB） | 同上 | `unicode.<w>.raw` |
| A10 | 幅境界（幅ちょうど / −1 / +1 の語長） | 同上 | `boundary.<w>.raw` |

すべて `--verbose` 有無の2系統で採る。**A1〜A10 は合成 `.jsonl` で生成でき、実会話は不要。**

### 5.3 人手が要るもの

> #### ☑ H1【完了・2026-07-31】ライブ scrollback と `--resume` が一致するか → **PASS**
> 実端末（幅110）で `script -q /tmp/live.raw claude` に「`ls -la /tmp` を実行して」を1回依頼 → `/exit`、続けて `script -q /tmp/resume.raw claude --resume <id>` を採取。
> 確定後トランスクリプト領域をアンカー `I'll` から比較して **608 バイトがバイト単位で完全一致**。唯一の差分は thinking 要約行の動詞（live `✻ Baked for 7s` / resume `✻ Crunched for 7s`）で、Claude Code がランダムに選ぶ語。**構造・色・CHA 桁位置はすべて一致。**
> → `--resume` を正解データとして採用可。**マスキング対象に「thinking 要約の動詞」を追加すること。**
>
> #### ☑ H2【完了】verbose の使用有無 → **未使用**
> 正解データは**畳んだ表示**側で固定する。実測された畳み形: `Listed 1 directory (ctrl+o to expand)`（col3、灰 RGB(153,153,153)、件数のみ bold、`⏺` も `⎿` も付かない）。
>
> #### ☐ H3【任意・後回し可】極端な幅での実端末確認
> - **採取するもの**: スクショ 2枚
> - **再現手順**: 幅 40 桁と幅 200 桁で `claude --resume <id>`、日本語＋絵文字を含む応答が見える位置で撮影
> - **保存先**: `docs/native-capture/w40.png` / `w200.png`
> - **なぜ必要か**: グリフの**実描画幅**（`⏺` が1列か2列か）はバイト列からは分からず、フォント依存。台帳 D5/D8 の判断材料

### 5.4 最小セット

**H1 の1件だけあれば着手できる。** H2 は口頭で即答できる。H3 は台帳 D5/D8 を詰める段階まで不要。

A1〜A10 は自動なので、H1 が取れ次第まとめて生成する。

---

## 6. 依頼者に判断を仰ぐ点（優先度順）

| # | 論点 | 選択肢 |
|---|---|---|
| **Q1** | **成功基準の定義。** 台帳 D4/D5/D6/D7/D8 は**意図的にネイティブと違えている**箇所で、うち D5/D8 は実バグ（scrollback bleed）の修正として入った。「完全一致」はこれらの巻き戻しを含むか | (a) 完全一致を貫き、D5/D8 はネイティブ方式（絶対列指定）で作り直す／(b) 「明文化された意図的逸脱を除く構造的忠実」に再定義 |
| **Q2** | **どちらのネイティブ描画を正解とするか** | (a) `--verbose`（全行展開）／(b) 既定（`(ctrl+o to expand)` 畳み）／(c) conductor 側でも切替可能にする |
| **Q3** | **width−1 マージン（D8）の扱い** | (a) 維持 → 折り返し位置は恒久的に1列ズレる／(b) ネイティブ方式（グリフ後に絶対列で本文配置）に作り替えて撤廃／(c) risky なコードポイントを含むブロックのみ条件付きで適用 |
| **Q4** | **B1（`truncate_to_width` の off-by-one）を今直すか** | 直すのが正しいが golden 全更新になる。テスト基盤より先に直すか後か |
| **Q5** | **B6（`open_reflow` 失敗で scroll-up が無反応）** | parity 以前の可用性バグとして別 issue に切るか |

---

## 7. 実施順序

1. **H1 の採取**（依頼者）→ ライブ vs `--resume` の一致を確認
2. **R1 / R2 の refactor**（`BuildCtx` 分離、`sweep` の引数化）— これが無いと何も書けない
3. **§3.9 コーパス不変条件 fuzzing** — 期待出力ゼロで bleed 系を刈れる。最高 ROI
4. **B1〜B6 の欠陥に対する失敗テスト**を先に書く
5. **§5.2 の自動採取**で台帳 golden を生成 → §3 の [N] ケースを埋める
6. **seam oracle**（修正A/B 込み）
7. 残りの [P] / [G] ケース

---

## 付録: 採取済みアーティファクト

scratchpad（セッション限り、必要なら repo に移す）:
- ハーネス: `altscreen_probe.py`, `ptycap.py`, `ptycap2.py`（フェーズ別キー注入）, `replay.py`（CHA/CUP 解釈でグリッド復元）, `diffwidth.py`
- 生キャプチャ: `native100.raw`, `resume100.raw`, `verb.a/b.raw`, `tool.a/b.raw`, `w60/80/120.a/b.raw`, `scroll14.raw`, `long.raw`, `ctrlo.00/.01`
- 合成 fixture: `fixtures/*.jsonl`（3本）
- 幅テーブル: `rust_widths.txt`, `node_widths.txt`（全 1,112,064 コードポイント）
