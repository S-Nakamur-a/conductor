# Explorer 仕様目録

0 から書き直すための、既存 Explorer の振る舞いの網羅的な列挙。実装の書き写しではなく、
「何ができるか」の目録。パスは worktree ルートからの相対。

対象は `src/explorer/` の全ファイル (3,335 行) と、Explorer に関わる範囲での
`src/event/mouse/mod.rs`、`src/app/` からの呼び出し元。

Explorer は上下 2 ペイン。上がファイルツリー、下が「変更ファイル一覧 (diff list)」か
「レビューコメント一覧 (comment list)」のどちらか (`ExplorerBottomView`)。

## 設計にそのまま効く 3 点

1. **`KeyContext::Explorer` が `focus_on_diff_list` で変わらない** (`src/types.rs:40`)。
   レイヤーがペインを区別できないので、ハンドラが入口で再判定している
   (`src/explorer/input/tree.rs:20-33`)。結果、explorer レイヤーに割り当てた和音は
   どちらのペインからも発火する。
2. **`d` / `c` が `bottom_view` と `focus_on_diff_list` の両方を書いている**
   (`tree.rs:22-31`)。この 2 つのフィールドが 1 つの概念であることの証拠。
3. **キー入力が同期的なファイルシステム走査を起こす**。ツリーが空のとき、キー処理の
   入口で `app.refresh_viewer()` を呼ぶ (`tree.rs:15-17`)。

## 0. キーが Explorer に届くまで

`src/event/mod.rs` の 6 段バブリング。

- **stage 5** `stage_keymap` (`event/mod.rs:232-240`) が `app.focus.key_context()` に対して
  和音を解決する。`Focus::Explorer` なら常に `KeyContext::Explorer` (`types.rs:40`)。
  解決された `Action` が `dispatch_global_action` (`event/global.rs:9-190`) の扱うもので
  あれば、そこで消費されパネルには届かない。
- **stage 6** `stage_focus` (`event/mod.rs:246-248`) が `handle_explorer_key` を呼ぶ。
- `KeyMap::resolve` はレイヤー → グローバルのフォールバック (`keymap/mod.rs:21-26`)。
- 全画面のコメント一覧モーダルだけは **stage 2** で `handle_explorer_comment_list_key` へ
  直接入る (`event/mod.rs:168`)。

## 1. キー操作

レイヤー表: `src/default_keybinds.toml:130-157` (`explorer`)、`:159-177`
(`explorer_diff_list`)、`:179-202` (`explorer_comment_list`)。
ハンドラ: `src/explorer/input/{tree,diff_list,comment_list}.rs`。

### 1a. どちらのペインでも効く (stage 5 でグローバルとして消費される)

| キー | Action | 効果 | file:line |
|---|---|---|---|
| `tab` | `CycleFocusForward` | ツリー → 変更ファイル一覧 → Viewer (サブフォーカスが停留点になる) | `global.rs:41`, `app/focus.rs:151-167` |
| `shift+tab` | `CycleFocusBackward` | 逆順。後ろから Explorer に入ると下ペインに着く | `global.rs:45`, `app/focus.rs:170-186` |
| `C` | `OpenCommentList` | `comment_list_selected`/`_scroll` を 0 に戻し、全画面モーダルを開く | `global.rs:36-40` |
| `w` | `ShowRevidere` | `cmd_show_revidere()` | `global.rs:186` |
| `W` | `AnalyzeRevidere` | `cmd_confirm_analyze_revidere()` | `global.rs:178` |
| `alt+w` | `ForceAnalyzeRevidere` | `cmd_analyze_revidere(true)`。キャッシュを飛ばす | `global.rs:182` |
| `:` | `CommandPalette` | パレットを開く | `global.rs:20` |

### 1b. どちらのペインでも効き、Explorer 自身が処理する

サブパネルへ委譲する前に入口で判定 (`input/tree.rs:20-33`)。

| キー | Action | 効果 |
|---|---|---|
| `d` | `ShowDiffList` | `bottom_view = DiffList` **かつ** `focus_on_diff_list = true` (`tree.rs:22-26`) |
| `c` | `ShowCommentList` | `bottom_view = Comments` **かつ** `focus_on_diff_list = true` (`tree.rs:27-31`) |

### 1c. ファイルツリーペイン (`focus_on_diff_list == false`)

入口で副作用がある。**`file_tree` が空なら `app.refresh_viewer()` を呼ぶ** — キー入力に
同期のファイルシステム走査がぶら下がっている (`tree.rs:15-17`)。

| キー | Action | 効果 | file:line |
|---|---|---|---|
| `j` / `down` | `NavigateDown` | 次の**可視**インデックス (畳まれた部分木を飛ばす) | `tree.rs:53-55` |
| `k` / `up` | `NavigateUp` | 前の可視インデックス | `tree.rs:56-58` |
| `enter` | `Select` | ディレクトリ: 畳まれていれば `ensure_children_loaded` してから `toggle_dir`。ファイル: `viewer.open_file(root, path, tab_width)` → `rehighlight_viewer` → `build_file_comment_cache` → `set_focus(Viewer)` | `tree.rs:59-76` |
| `l` / `right` | `ExpandOrRight` | ディレクトリかつ畳まれていれば遅延読み込みしてから `expand_dir`。ファイルでは何もしない | `tree.rs:77-89` |
| `h` / `left` | `CollapseOrLeft` | `collapse_dir`。ファイル / 既に畳まれている場合は何もしない | `tree.rs:90-93` |
| `g` | `GoToTop` | 最初の可視インデックス | `tree.rs:94-98` |
| `G` | `GoToBottom` | 最後の可視インデックス | `tree.rs:99-103` |
| `/` | `SearchFilename` | `event::open_filename_search`。クエリと結果を消し、ツリーを走査して全ファイルのキャッシュを埋め直し、検索を走らせる | `tree.rs:104-106`, `event/overlay_helpers.rs:8-16` |

<!-- ここから先は未収集。2〜7 節を追記すること。 -->
## 2. マウス操作

入口は `src/event/mouse/mod.rs::handle_mouse_event`。Explorer 列に届くのは
`geom.column_at(col) == Column::Explorer` になったクリックだけで、実処理は
`src/explorer/mouse.rs::handle_explorer_column_click` (`mouse.rs:113-244`)。

### 2a. Explorer に届く前に消費される

順序が意味を持つ。上から順に判定し、当たった時点で `return`。

| 判定 | 条件 | file:line |
|---|---|---|
| ホバーモーダル (ポップアップ/参照リスト) | `handle_hover_modal_mouse` が true | `event/mouse/mod.rs:463-465` |
| ブロッキングオーバーレイ | `has_blocking_overlay`。**全ホバーを消してから** return (裏で光ったまま残さないため) | `event/mouse/mod.rs:469-478` |
| revidere 2 列ビュー | `focus == Revidere` かつ main_area 内 | `event/mouse/mod.rs:513-521` |
| メニューバー / worktree ストリップ / タイトルバー | クリック時、上から順 | `event/mouse/mod.rs:590-601` |
| revidere 状態チップ | `revidere_badge_hit` (後述) | `event/mouse/mod.rs:613-616` |
| パネル境界のドラッグ開始 | `divider_at` かつ `divider_draggable` | `event/mouse/mod.rs:622-628` |
| 埋め込みエディタ | `app.editor.is_some()` かつ `left_end <= col < viewer_end` | `event/mouse/mod.rs:634-637` |
| `[<=>]` 展開ボタン | `row == main_area.y` かつ `expand_button_at` | `event/mouse/mod.rs:646-654` |

**全画面コメント一覧モーダル (`ActiveOverlay::CommentList`) にマウス操作は無い。**
`has_blocking_overlay` が `overlays.active != None` を見るので、全イベントが
そこで捨てられる (`event/mouse/mod.rs:243`)。

`divider_draggable` は最大化中 (`expanded_panel.is_some()`) と、エディタが
Explorer+Viewer を 1 つの PTY に合体させている間の `ExplorerViewer` /
`ExplorerSplit` について false (`event/mouse/mod.rs:272-281`)。

### 2b. 左クリック — ファイルツリーペイン (`row < explorer_mid_y`)

入る時点で `set_focus(Focus::Explorer)` と `focus_on_diff_list = false` を書く
(`mouse.rs:118`, `:195`)。行の解決は `explorer_tree_row_at` (`mouse.rs:41-69`)。

| 対象 | 効果 | file:line |
|---|---|---|
| ディレクトリ行 | 畳まれていれば `ensure_children_loaded` してから `toggle_dir`。1 クリックで開閉 (ダブルクリック判定なし) | `mouse.rs:205-209` |
| ファイル行・シングル | `tree_selected` を更新 → `open_file_preview` → `rehighlight_viewer` → `build_file_comment_cache`。**フォーカスは Explorer に残る** | `mouse.rs:226-233` |
| ファイル行・ダブル | `open_file` (永続タブ) → 同上 → `set_focus(Viewer)` | `mouse.rs:222-238` |

シングル/ダブルの差は preview タブか永続タブか。コメント曰く「開いたまま溜まるのを
防ぐ」(`mouse.rs:220-221`)。ダブルクリック判定は `register_double_click_on` で、
**同一 idx への連続クリックであること**も要求する (`event/mouse/mod.rs:78-88`)。
`DOUBLE_CLICK_MS` 未満が条件。状態は `app.viewer.click.last_tree_click_time` /
`last_tree_click_idx`。

### 2c. 左クリック — 下ペイン (`row >= explorer_mid_y`)

入る時点で `focus_on_diff_list = true` を書く (`mouse.rs:122`)。

**下枠の `✨ Ask Claude All` ボタン** — `bottom_view == Comments` かつ
`row == 下枠の行` かつ右端から 19+1 セル以内 (`mouse.rs:124-134`)。押すと
`/conductor:address-conductor-comment` を Claude セッションへ送る。セッションが
入力待ちなら即送信、そうでなければ `deferred_prompts` に積む。どちらでも
`set_focus(TerminalClaude)` と成功ステータス。セッションが無ければ警告のみ
(`mouse.rs:10-32`)。**ラベル幅 19 はハードコードで、描画側と共有していない。**

**コメント一覧 (`bottom_view == Comments`)** — 行の解決は
`comment_list_scroll + (row - inner_y)` のインライン計算で、`diff_list_row_at`
のような共有関数を通らない (`mouse.rs:136-142`)。`comment_list_rows.len()` で
範囲チェック。`comment_list_selected` を更新し、
`navigate_to_comment_with_focus(app, comment_idx, is_double)` を呼ぶ。
シングル: 位置へジャンプしフォーカスはコメント側に残す。ダブル: ジャンプして
Viewer にフォーカスを移す (`mouse.rs:155-160`)。状態は
`app.viewer.click.last_comment_click_time` / `last_comment_click_idx`。

**差分リスト (`bottom_view == DiffList`)** — 行の解決は `diff_list_row_at`
(`mouse.rs:85-110`)。`display_list.len()` で範囲チェック後、`diff_list_selected`
を更新してから 3 分岐 (`mouse.rs:170-190`)。

| 行の種類 | 効果 |
|---|---|
| `DiffListEntry::Summary {}` | `viewer.enter_summary_view()` → `set_focus(Viewer)` |
| セクションヘッダー (`toggle_section(idx)` が true) | 開閉。畳んで行数が減ったら `diff_list_selected` を末尾へクランプ |
| ファイル (`resolve_file(idx).is_some()`) | `open_diff_file_at_selected()` → `set_focus(Viewer)`。コメントがあれば最初のコメントへ着地する |

**ここにダブルクリック判定は無い。**差分リストは常に 1 クリックで開き、常に
Viewer へフォーカスが移る (ツリーと非対称)。

### 2d. revidere 状態チップ

`revidere_badge_hit` (`event/mouse/mod.rs:258-266`)。`row == explorer_mid_y` —
つまり Changed files パネルの**上枠の行** — かつ `bottom_view == DiffList` かつ
`app.editor.is_none()`。列は描画側と同じ `render::revidere_badge_cols` から引く。
クリックで `cmd_revidere_badge_click` (`revidere/run.rs:216-226`) —
`Running` ならステータス表示のみ、`Fresh` なら `cmd_show_revidere`、
`None`/`Stale` なら `cmd_confirm_analyze_revidere`。

### 2e. ホイール

`src/event/mouse/scroll.rs::handle_mouse_scroll`。1 段 = 3 行 (`mod.rs:570-575`)。
Explorer 列 (`left_end <= col < explorer_end`) の中で `row >= explorer_mid_y` か
どうかで振り分ける (`scroll.rs:66-105`)。**選択は動かさず、スクロールだけ動く。**

| ペイン | 上限 | file:line |
|---|---|---|
| 差分リスト | `display_list.len() - 1`。空 (`file_count == 0`) なら何もしない | `scroll.rs:70-85` |
| ファイルツリー | `visible_indices().len() - tree_height` | `scroll.rs:86-104` |

差分リストの上限が `len() - 1` (ツリーのように高さを引かない) ので、**差分リストは
最後の 1 件だけが残る位置までスクロールできる。**ツリーは最終行がパネル下端に
来た位置で止まる。

横スクロール (`ScrollLeft`/`ScrollRight`) は Viewer 列限定で、Explorer には効かない
(`event/mouse/mod.rs:576-585`)。

### 2f. ホバー

`MouseEventKind::Moved` で毎回、Explorer の 2 リストのホバー行を書き直す
(`event/mouse/mod.rs:742-760`)。

- ツリー: `list_hover.explorer_tree.set(explorer_tree_row_at(...))`
- 差分リスト: `list_hover.diff_list.set(diff_list_row_at(...))`
- revidere チップ: `app.revidere.badge_hover = revidere_badge_hit(...)`

**クリックとホバーが同じ解決関数を通る**のが設計上の要。コメント曰く
「ハイライトされた行とクリックで開かれる行が食い違うことは構造的にあり得ない」
(`mouse.rs:39-40`, `:78-79`)。カーソルが外れれば同じ関数が `None` を返すので、
「離れた」検出は別途要らない (`event/mouse/mod.rs:742-746`)。

ホバーの型は `ListHover` (`explorer/hover.rs`) が `HoverRow` を 2 つ並べたもの。
`HoverRow` はフェードアウトを持つ (`list_row.rs:18-75`):

- `set` は**行が実際に変わったときだけ**フェードを開始する。静止中に同じ行を
  再セットしてもアニメーションをリスタートしない (`list_row.rs:38-41`)。
- `phase(row)` が `On` か `FadingOut(残り強度 0.0..1.0)` を返す。
- `is_animating()` はメインループの再描画ポンプ継続判定に使われる。

**コメント一覧にホバーは無い。**`ListHover` は `explorer_tree` と `diff_list`
の 2 つだけ (`hover.rs:10-15`)。

## 3. 1 行に何が載っているか

3 つのリスト = ファイルツリー (`render/file_tree.rs`)、変更ファイル一覧
(`render/diff_list.rs`)、コメント一覧 (`render/comment_list.rs`)。
行の組み立ての抽象化は `list_row.rs` にあるが、**共有できているのは 3 つ中 2 つ**
(ツリーと変更ファイル一覧) だけ。

### 3a. 3 つに共通する構造

| 要素 | 実装 | 3 リストの状況 |
|---|---|---|
| スクロール窓 | `.iter().enumerate().skip(scroll).take(inner_height)` | 3 つとも同じ形。ただし変更ファイル一覧だけ `take(list_height)` = `inner_height - banner_rows` (`diff_list.rs:166`) |
| 欠損に強い添字 | `.filter_map(...)` + `.get(idx)?` | 3 つとも。理由はコメントで明示 (`diff_list.rs:205-210`): 「display_list とファイル vector は異なるティックで再構築されるため、片方が古いままレンダリングされるフレームがありうる。行をスキップすればチラつきで済むが、インデックスアクセスだと描画処理の内側からアプリ全体を落としかねない」 |
| 描画前の `Clear` | `frame.render_widget(Clear, area)` | 3 つとも。「最後の項目より下の行に前フレームの文字が残らないよう」 (`file_tree.rs:165-168`, `diff_list.rs:304-306`, `comment_list.rs:274-276`) |
| 選択ハイライト | 選択時 `selected_fg`/`selected_bg` + BOLD、パネル非フォーカス時は `*_inactive` | 3 つとも同じ 4 トークン。**ただしコメント一覧は `row_style` を通さず自前で再実装** (`comment_list.rs:167-177`, `:241-251`) |
| 3 段のボーダー色 | 自ペインにフォーカス / Explorer カラムにはあるが他ペイン / カラム外 | 3 つとも。ツリーと変更ファイル一覧は `animated_border_color(Focus::Explorer)`、コメント一覧は静的な `theme.border_focused` (`comment_list.rs:34-40`) |
| タイトルの強調 | 自ペインにフォーカスなら `theme.fg` + BOLD、でなければ `theme.muted` | 3 つとも同じ (`file_tree.rs:81-85`, `diff_list.rs:120-124`, `comment_list.rs:59-63`) |
| `PanelChrome` | `PanelChrome::new(theme, title, panel_focused, border_color).into_block()` | 3 つとも。`with_expand_button` はツリーのみ |

### 3b. `row_style` / `decoration_style` が共有しているもの

`list_row.rs:99-157`。ツリーと変更ファイル一覧の 2 つが使う。

`row_style(theme, base_fg, selected, panel_focused, hover) -> Style`:

1. **selected が hover に勝つ** (`list_row.rs:106-118`)。「選択の方がより重要な状態であり、
   hover の色味で薄めるとポインタでなぞる間にどの行が選択されているか追いにくくなる」。
2. hover は**前景色のみ**。ADR D1 改訂版に従う。理由がコメントに残っている:
   「背景色で表現する方式も試したが却下した: 11 テーマ中 7 テーマで
   `selected_bg_inactive` と区別が付かなかった。これはまさに hover 中だが
   フォーカスされていない行が置かれる状態そのものである」(`list_row.rs:94-98`)。
3. hover の色は `hover_emphasis` (`list_row.rs:212-238`) が **base_fg から導出**する。
   固定の `theme.accent` を使っていた頃は「hover が嘘をつく」不具合があった:
   solarized-dark と gruvbox で `accent == warning` (ステージ済みの色) なので
   「未ステージの行を hover するとステージ済みの行とまったく同じ色に塗り替わって」
   いた (`list_row.rs:192-201`)。
4. 押し出し量は固定ではなく `HOVER_MIN_DISTANCE = 120.0` をクリアする最小量を
   20 ステップで探索する。固定量だと「theme.fg の行は約 53 しか動かなかったのに対し
   theme.hint (untracked) の行は約 237 動いた」= 4 倍弱くなっていた (`list_row.rs:159-168`)。
5. **hover には色以外の第 2 チャンネル (UNDERLINED) がある** (`list_row.rs:139-143`)。
   色だけだと 11 パレット × 全 base 色をカバーせねばならず、最頻出の `theme.fg` で
   最も弱くなるため。**`FadingOut` には持ち越さない** — 「下線は『ポインタがここにある』
   ことを示すものであり、離れた瞬間に真ではなくなる。またモディファイアは補間できない」
   (`list_row.rs:135-138`)。
6. `FadingOut(t)` は `Theme::lerp(base_fg, target, t)` で hover 色 → base 色。

`decoration_style(style)` = `style.remove_modifier(UNDERLINED)` (`list_row.rs:155-157`)。
行のうち**名前以外**(インデント、展開矢印、アイコン、行数) に使う。「下線は
『あなたが指しているのはこれだ』という印であり、その対象はファイルであって手前の
ツリー装飾ではない。インデントの下にも下線を引くと (中略) ポインタのアフォーダンス
ではなく行全体に渡るテキスト入力の下線のように見えていた」(`list_row.rs:148-154`)。
BOLD (選択) は残す。

**これが「1 行 = prefix span + icon span + name span + 付随 span」という
分割を全リストに強制している唯一の力**。span を分けている理由は 2 つあり、
(a) 下線を名前だけに限る (b) アイコンに種別色を乗せる。

### 3c. ファイルツリーの行 (`render/file_tree.rs:93-163`)

構成: `[prefix][glyph][name]` の 3 span。

| 要素 | 内容 | file:line |
|---|---|---|
| インデント | 深さ × 2 スペース。深さ 0-9 は `INDENT_CACHE` の定数、それ以上は `"  ".repeat` | `file_tree.rs:11-31` |
| 展開矢印 | ディレクトリのみ `expand_arrow(is_expanded, icon_set)`。ファイルは 2 スペース | `file_tree.rs:105-110` |
| アイコン | ディレクトリ: `dir_icon(is_expanded)`、ファイル: `entry.icon` | `file_tree.rs:111-116` |
| 名前 | `entry.name` (ベース名のみ) | `file_tree.rs:160` |
| git 状態の色 (base_fg) | `Untracked`/`Ignored` → `theme.hint`、`Tracked` かつディレクトリ → `theme.info`、`Tracked` かつファイル → `theme.fg` | `file_tree.rs:123-133` |

`theme.muted` を意図的に避けている: 「solarized-dark では背景と同じ RGB で
事実上見えなくなり、github-light ではボーダー色に見えてしまう」(`file_tree.rs:118-122`)。

**アイコン色の譲渡ルール**: アイコンは通常 `icon.role.color(theme)` で種別色を持つが、
**選択行または non-Tracked 行では行の色に譲る** (`file_tree.rs:147-155`)。
「選択の背景色の上で種別色が読める保証は 11 テーマぶんには無く、untracked/ignored の
減光はアイコンにも及ぶべきだから」。

**スクロールバー**: 3 リスト中このリストだけが持つ。`visible.len() > inner_height`
のときのみ (`file_tree.rs:172-184`)。

ツリーには +N -N もバッジも viewed 印も無い。

### 3d. 変更ファイル一覧の行 (`render/diff_list.rs:168-301`)

`DiffListEntry` の 3 バリアントで行の形が違う。

**`Directory`** — `[prefix (2sp + indent + arrow + icon)][name]` の 2 span。
base_fg は `theme.info` 固定。「ディレクトリのアイコン色は行の色 (theme.info) と
同じなので、ファイル行と違って span を分ける必要がない」(`diff_list.rs:185-187`)。

**`File`** — 最大 7 span。

| 要素 | 内容 | file:line |
|---|---|---|
| インデント | 先頭 2 スペース + 深さ × 2 スペース (ツリーの `INDENT_CACHE` は使わない) | `diff_list.rs:215-217` |
| アイコン | `file_icon(filename)` | `diff_list.rs:216-218` |
| 名前 | ベース名のみ (`path.rsplit('/').next()`) | `diff_list.rs:213` |
| **git ステージ色** | `Untracked` → `hint`、`Unstaged` → `error`、`Staged` → `warning`、`Committed` → `success` | `diff_list.rs:62-94` |
| **+N** | `format!(" +{added_lines}")`、色は `theme.diff_add` 固定 | `diff_list.rs:259-262` |
| **-N** | `format!(" -{deleted_lines}")`、色は `theme.diff_del` 固定 | `diff_list.rs:263-266` |
| **コメントバッジ** | `  💬{total}`。未解決が 1 件でもあれば `theme.accent`、全解決済みなら `theme.muted`。0 件なら span 自体を出さない | `diff_list.rs:341-371` |
| **viewed 印** | `viewed` に相対パスが入っていれば `  ✓` を `theme.success` で | `diff_list.rs:271-276` |

ステージ色の判定順序に注意 (`diff_list.rs:58-61`): 「編集して git add し、さらに編集する、
といった操作をすると WT_* と INDEX_* の両方のビットが立つことがある。この場合は
unstaged を優先させたいので WT_* のチェックを先に行う」。`GitStatusMap` にエントリが
無い = HEAD に対してクリーン = `Committed`。ステージ色が必要な理由もコメントにある:
「行数はベースからの合計なので、その内訳がコミット済みか手元の編集かはこの色でしか
分からない」(`diff_list.rs:220-223`)。

`counts_style` (`diff_list.rs:240-243`) は `decoration` の背景/修飾を保ったまま前景だけ
差し替えるクロージャ。「+added/-deleted はステージ状態に関わらず自前の前景色を保つため、
ラベルに焼き込まず別の span に分けている」。アイコンの色の譲渡は**選択行のときだけ**で、
ツリーと違い git 状態は見ない (`diff_list.rs:249-253`)。

**`Summary`** — `[  ▣ ][SUMMARY]` の 2 span。base_fg は `theme.accent`。
**非選択のときだけ手動で BOLD を足す** — 「`row_style` は選択時以外は BOLD を
適用しないため」(`diff_list.rs:288-292`)。

**エラーバナー** (`diff_list.rs:160-165`) はリストの先頭に差し込まれる `ListItem` で、
`display_list` の要素ではない。`  ⚠ {msg}`、色は `theme.error`、改行はスペースに潰す。
理由は 7 節に再掲。

### 3e. コメント一覧の行 (`render/comment_list.rs:89-272`)

`CommentListRow` の 2 バリアント。**このリストだけ `list_row.rs` を一切使わない。**

**`Comment`** — `[expand_indicator][status_marker][ ][kind_badge][location][ ][body][more_suffix][reply_suffix]`。

| 要素 | 内容 | file:line |
|---|---|---|
| 展開インジケータ | 返信があれば `expand_arrow(expanded, icon_set) + " "`、無ければ 2 スペース | `comment_list.rs:132-141` |
| ステータスマーカー | 解決済み `✓` / 未解決 `○` | `comment_list.rs:103` |
| 種別バッジ | `ui::review::kind_badge_span(comment.kind, ...)` (非選択時)、`kind_icon` の文字列 (選択時) | `comment_list.rs:102`, `:203` |
| 位置 | `{ベース名}:L{start}` または `:L{start}-{end}` | `comment_list.rs:105-116` |
| 本文 | **最初の 1 行のみ**、幅計算で切り詰め | `comment_list.rs:143-162` |
| `+N` | 本文の残り行数。「改行をスペースに潰すとコメントに構造があったことが分からなくなるため」 | `comment_list.rs:143-152` |
| `↩N` | 返信数。「返信数は行末に置き、目が追う場所 (位置情報と本文) の邪魔にならないようにする」 | `comment_list.rs:118-130` |

非選択行の色は 3 段 (`comment_list.rs:183-198`): マーカーは解決済み `muted` / 未解決
`warning`、位置と suffix は `muted`、本文は解決済み `muted` / 未解決 `fg`。
理由: 「解決済みの行はマーカーも含めて完全に後退させる。ミュートな本文の上に明るい ✓ が
乗ると、もう注意を払う必要のない行にこそ目が引き寄せられてしまうため」。

**`Reply`** — `    ↳ {author} {body}{more_suffix}`。author は `You` / `Claude`。
非選択時は author が `theme.info` + BOLD、本文が `theme.reply_text`。
「深いインデントと author の太字表示により、本文を読まなくてもスレッド構造が
分かるようにする」(`comment_list.rs:257-258`)。

**選択行は 1 本の `Span` に潰す** — `format!` で全要素を連結した文字列に 1 スタイル
(`comment_list.rs:178-181`, `:252-255`)。非選択行の span 分割とは別コードパス。
「選択中の行は視認性のため一律のハイライトを維持する」(`comment_list.rs:166`)。

### 3f. `list_row.rs` が共有できていること / できていないこと

**できていること** (ツリー ↔ 変更ファイル一覧の 2 リスト間):

- selection / focus / hover の**優先規則**。「各パネルで再導出させて食い違わせるのでは
  なく、一箇所に集約するため」(`list_row.rs:1-3`)。
- hover の**色の導出** (`hover_emphasis`) とフェード (`HoverRow`)。
- 下線を名前だけに限る規約 (`decoration_style`)。
- 上記をテストで固定している (`list_row.rs:240-527`)。特に
  `hover_never_repaints_a_row_as_another_meaningful_token` は全 11 テーマ × 5 意味色で
  hover が別トークンに化けないことを検証する。

**できていないこと:**

1. **コメント一覧が丸ごと外れている。**選択スタイル (4 トークン + BOLD) が
   `comment_list.rs` に 2 回コピーされている (`:167-177` と `:241-251`)。
   `row_style` の selected 分岐 (`list_row.rs:106-118`) と同じ内容。hover も無い。
2. **行の組み立て (span 列) は共有していない。**`row_style` が返すのは `Style` 1 つで、
   「prefix / icon / name / 付随」の分割は 3 ファイルそれぞれが手で書いている。
   結果としてインデントの作り方が 2 通りある (ツリーは `INDENT_CACHE`、
   変更ファイル一覧は `"  ".repeat(depth)`)。
3. **アイコン色の譲渡ルールが 2 通り。**ツリーは「選択行 **または** non-Tracked」
   (`file_tree.rs:147-151`)、変更ファイル一覧は「選択行のみ」(`diff_list.rs:249`)。
   同じ理由 (11 テーマで選択背景の上の種別色が読めない) を別々に書いている。
4. **BOLD の扱いが漏れる。**`row_style` は非選択で BOLD を付けないので、
   SUMMARY 行が呼び出し側で足し直している (`diff_list.rs:288-292`)。
5. **選択ハイライトが行全体に及ぶかどうかが揃っていない。**ツリーと変更ファイル一覧は
   span ごとに `Style` を配るので選択背景は文字のある範囲だけ。コメント一覧は
   選択時に 1 span に潰すのでやはり文字範囲だけ。どちらも「行末まで塗る」ではない。
6. **ホバー行の添字の基準が違う。**ツリーは**可視リストの添字** (`vis_idx`)、
   変更ファイル一覧は **`display_list` の添字** (`idx`)。`HoverRow` は `usize` を
   受けるだけなので型では守られていない。

## 4. ファイルツリー特有の振る舞い

起点は `src/explorer/tree.rs` (525 行) と `src/viewer/file_tree.rs` の
`FileTreeEntry`。ツリーは**フラットな `Vec<FileTreeEntry>` の pre-order** で、
木構造ではない。各エントリが `depth` を持ち、親子関係は「自分より深い連続する
エントリが子」という暗黙の規約でしか表現されていない。

### 4a. `FileTreeEntry` の 8 フィールド

`path` (root からの相対) / `name` (最後の要素) / `depth` / `is_dir` /
`is_expanded` / `children_loaded` / `icon` / `git_state` (`viewer/file_tree.rs:17-38`)。
`icon` は生成時に一度だけ計算するが「字形の選択は描画時まで遅延するので、
これは文字セットに依存しない」(`file_tree.rs:32-34`)。

### 4b. 走査 (`walk_dir`, `tree.rs:458-520`)

- **1 階層だけ読む。再帰しない。**子ディレクトリは `children_loaded: false` の
  折りたたみ状態で積まれる。「初回の走査と遅延展開の両方をこの 1 つの関数で
  まかなうのは意図的な設計。もともとは同一ロジックの別々のコピーだったが、
  git_status パラメータを両方に通す必要が出たとき、次の乖離が発生するのは
  時間の問題だった」(`tree.rs:453-457`)。
- **ソート**: ディレクトリが先、ファイルが後、それぞれファイル名の
  アルファベット順 (`tree.rs:476-484`)。
- **`MAX_DEPTH = 8`** (`tree.rs:343`)。これを超える深さは読まない。
- **`SKIP_DIRS`** — 19 個のハードコード配列 (`tree.rs:347-367`):
  `.git` / `node_modules` / `target` / `vendor` / `.next` / `dist` / `build` /
  `__pycache__` / `.cache` / `coverage` / `.venv` / `venv` / `bower_components` /
  `.tox` / `.mypy_cache` / `.pytest_cache` / `.turbo` / `.nuxt` / `.output`。
  「ファイル数が非常に多くなりがちで、対話的に閲覧する価値がほとんどないもの」。
- **`.gitignore` は走査には影響しない。**`SKIP_DIRS` と `MAX_DEPTH` だけが
  フィルタで、gitignore された内容は**表示される** — ただし後述の `git_state` で
  暗く塗られる。
- `read_dir` に失敗したディレクトリは黙って空扱い (`tree.rs:469-471`)。

### 4c. 遅延読み込み (`ensure_children_loaded`, `tree.rs:371-404`)

`is_dir` かつ `!children_loaded` のときだけ走る。`walk_dir` で直接の子を集め、
`idx + 1` の位置に `splice` で差し込む。**`children_loaded` は子が 0 個でも
true にする** (`tree.rs:388`) ので、空ディレクトリを毎回読み直さない。

**`tree_selected` の補正が必要**: 挿入位置以降を指していたら挿入数だけ足す
(`tree.rs:397-400`)。フラット Vec に途中挿入する構造の代償。

呼ばれるのは 4 箇所 — `enter`/`l` のキー (`input/tree.rs`)、マウスのディレクトリ
クリック (`mouse.rs:207`)、`load_file_tree` の展開状態復元 (`tree.rs:150`)、
`reveal_file_in_tree` の中間ディレクトリ (`tree.rs:327`)。

### 4d. 展開 / 折りたたみ

`toggle_dir` / `expand_dir` / `collapse_dir` (`tree.rs:208-239`)。3 つとも
`is_dir` を確認し、状態が実際に変わるときだけ `invalidate_visible_cache()` を呼ぶ。
**子の読み込みはしない** — 呼び出し側が先に `ensure_children_loaded` する規約。

`visible_indices()` (`tree.rs:251-278`) が畳まれた部分木を飛ばす。実装は
`skip_depth` を持った 1 パスの線形走査: 畳まれたディレクトリに当たったらその
`depth` を記録し、それより深いエントリを飛ばす。結果は `Rc<Vec<usize>>` で
キャッシュされ「同一フレーム内での繰り返し呼び出しは実質コストゼロ」。
無効化は `invalidate_visible_cache()` のみ — 「ツリー構造が変わるたび
(展開/折りたたみ、子の読み込み、ツリーの再読み込み) に必ず呼ぶこと」
(`tree.rs:241-245`)。**`&mut self` を要求する**ので、描画側も
`app.explorer.visible_indices()` のために可変借用が要る。

### 4e. git 状態 (`git_engine/status_map.rs`)

ツリーの色は `TreeGitState` の 3 値 — `Tracked` / `Untracked` / `Ignored`
(`status_map.rs:14-19`)。「staged/unstaged は区別しない — そのより細かい区別は
Changed files 一覧が使う `GitStatusMap::status` 側にある」。

スナップショットは `load_file_tree` ごとに 1 回だけ取る (`tree.rs:119-128`)。
**取得失敗時は空マップにフォールバックするがログを残す。**理由が長く書かれている:
「空のマップは無害なフォールバックではない — エントリが無いと、ツリー上は全て
Tracked、Changed files 上は全て Committed (緑) に見えてしまい、UI が
『未ステージの変更がある』の正反対を黙って主張してしまう」。git 管理外の
ディレクトリではこの経路が正当。実在リポジトリでの一時的失敗
(`index.lock` 競合など) と画面上は区別が付かないので「ログだけが両者を区別する手段」。

`classify` (`status_map.rs:102-135`) は `status` と違い**祖先を遡る**:

- libgit2 は ignored ディレクトリを `"target/"` のような**末尾スラッシュ付きの
  折りたたみ 1 エントリ**として報告するので、`target/debug/foo` は自分のエントリを
  持たず祖先から `Ignored` を継承する必要がある。
- untracked ディレクトリでは不要 — `recurse_untracked_dirs(true)` が既に
  ファイル単位に展開している。
- `has_descendants(path) && all_descendants_untracked(path)` の追加判定がある
  (`status_map.rs:125-133`)。「git がまだ見たことのないディレクトリはそれ自身の
  エントリを持たない (中略) このチェックなしにここへ到達すると、新規ディレクトリが
  通常の tracked 色で描画される一方で中身は薄暗く表示されてしまう —
  親が『既知』で子が『新規』に見えるのは逆である」。

`git_state` は**エントリ生成時に刻まれる** (`walk_dir` の `git_status.classify`,
`tree.rs:508`)。フレームごとの再評価ではない。遅延読み込みされた子は
`load_file_tree` のときのスナップショットを再利用する (`tree.rs:385`)。

### 4f. 再読み込み (`load_file_tree`, `tree.rs:88-205`)

**5 つの状態をまたいで保持する**: 展開済みディレクトリの集合、カーソルの指す
エントリのパス、開いていたファイル、スクロール位置 (暗黙)、全パス集合 (差分検出用)。

手順: (1) 退避 → (2) git status を取り直す → (3) `file_tree.clear()` + 走査 →
(4) 展開状態を復元 (遅延ディレクトリは `ensure_children_loaded` も走らせて
「リフレッシュ前と同じ見た目のツリー」にする) → (5) `prev_file` がまだ存在すれば
`reopen` に載せてカーソルを合わせる → (6) 開いているファイルにアンカーされて
いなければ `prev_selected_path` でカーソルを復元 → (7) 範囲クランプ →
(8) `entries_changed` を全パス列の比較で判定。

戻り値は `#[must_use]` の `TreeReload` (`tree.rs:18-29`) の 3 フィールド:

| フィールド | 意味 | 呼び出し側の責務 |
|---|---|---|
| `root_changed` | 根が変わった | 新しい根に無いファイルのタブを閉じる (`prune_tabs_to_root`)。同じ根への再走査で閉じてはならない — 「一時的に消えたファイルのタブまで勝手に閉じてしまう」 |
| `reopen` | 開いていたファイルの新しい相対パス | 「読んでいた位置と表示モード (unified diff / SUMMARY / markdown) を保ったまま」読み直す |
| `entries_changed` | 可視エントリ集合が変わった | 3 秒ポーリングがこれを見て、変化が無ければ再描画をスキップ |

**`ExplorerState` は `ViewerState` を知らない** (`tree.rs:15-17`, `:83-87`)。
`prev_file` は引数で受け取り、後始末は `TreeReload` を返して App の配線層に任せる。
`populate_filename_search_cache` も同じ形で `&mut FilenameSearchState` を受け取る
(`tree.rs:410-418`)。

### 4g. 根の所有 (`tree.rs:32-69`)

`root` は `FileTreeState` の `pub(in crate::explorer)` フィールドで、読むのは
`root()`、書くのは `load_file_tree` / `replace_tree` / `set_root` の 3 つだけ
(`mod.rs:28-33`)。理由: 「以前は根を持たず、ファイルを開くたびに呼び出し側が
『今どの worktree か』を引き直して渡していたので、表示中のツリーと開く先が
食い違っても誰も気付けなかった」。

`replace_tree` が根・エントリ・git status を**同時に**入れ替えるのも同じ理由:
「別々に書けるようにしておくと『根は新しいのにエントリは古い』状態が作れてしまい、
その瞬間のクリックは別ブランチの同名ファイルを静かに開く」(`tree.rs:48-51`)。

`set_root` は走査を伴わず根だけ差し替える。「根が空のまま相対パスを繋ぐと
カレントディレクトリ相対になり、意図しないファイルを黙って開く」(`tree.rs:40-41`)。

### 4h. reveal (`reveal_file_in_tree`, `tree.rs:287-338`)

相対パスを `/` で分割し、セグメントごとに**パス全体の線形探索** (`position`) を
行う。中間セグメントは `ensure_children_loaded` + 展開、最後のセグメントで
`tree_selected` を合わせる。エントリが見つからなければ途中で `return` (無音)。

スクロール調整は他の 2 箇所 (`adjust_tree_scroll`) と規則が違う:
**可視位置が窓の外にあるときだけ**動かし、着地点を `vis_pos - height/3` に置く
(`tree.rs:318-324`) — 上端に貼り付けず、上に 1/3 の余白を残す。

### 4i. フィルタ

**ツリーにフィルタは無い。**`/` (`SearchFilename`) はツリーを絞り込むのではなく、
別の全画面あいまい検索モーダルを開く (`event/overlay_helpers.rs:8-16`)。
そこで使うファイル一覧は `populate_filename_search_cache` が**ツリーとは独立に
ファイルシステムを歩き直して**作る (`tree.rs:410-448`) — `collect_all_file_paths`
は `SKIP_DIRS` と `MAX_DEPTH` は共有するが、ディレクトリを含めず、
折りたたみ状態も無視して全ファイルを拾う。

### 4j. ディレクトリの集約表示

**ツリーには無い。**`a/b/c` に中身が 1 つしかなくても 3 行として描かれる。
(集約されるのは変更ファイル一覧側の `DiffListEntry::Directory` — そちらは
`diff_state` が組み立てる。)

## 5. Explorer と App の結合

対象は `src/explorer/` 配下の全ファイル。Explorer 自身の状態
(`app.explorer` = `ExplorerState`) と、それ以外の App のフィールドを分ける。

### 5a. Explorer が所有する状態 (`ExplorerState` / `FileTreeState`)

`src/explorer/mod.rs:22-105`。13 フィールド。

| フィールド | 型 | 誰が書くか |
|---|---|---|
| `tree.root` | `PathBuf` | `load_file_tree` / `replace_tree` / `set_root` のみ (`mod.rs:28-33`) |
| `tree.file_tree` | `Vec<FileTreeEntry>` | `load_file_tree` / `replace_tree` / `ensure_children_loaded` / `toggle_dir` / `expand_dir` / `collapse_dir` |
| `tree.tree_selected` | `usize` | `input/tree.rs` 8 箇所、`mouse.rs:201`、`tree.rs` の復元/reveal |
| `tree.tree_scroll` | `usize` | `scroll.rs:18,20`、`tree.rs:322`、`event/mouse/scroll.rs:92-103` |
| `tree.cached_visible_indices` | `Option<Rc<Vec<usize>>>` | `visible_indices` / `invalidate_visible_cache` |
| `tree.git_status` | `GitStatusMap` | `load_file_tree` / `replace_tree` |
| `diff_list_selected` | `usize` | `input/diff_list.rs` 14 箇所、`mouse.rs:169,183` |
| `diff_list_scroll` | `usize` | `scroll.rs:30,32`、`event/mouse/scroll.rs:74-83` |
| `focus_on_diff_list` | `bool` | `input/tree.rs:24,29`、`input/diff_list.rs:15`、`input/comment_list.rs:46`、`mouse.rs:122,195` |
| `tree_height` | `usize` | **描画時のみ** `render/mod.rs:55` |
| `diff_list_height` | `usize` | **描画時のみ** `render/mod.rs:56` |
| `diff_banner_rows` | `usize` | **描画時のみ** `render/mod.rs:57` |
| `bottom_view` | `ExplorerBottomView` | `input/tree.rs:23,28` |
| `comment_list_selected` | `usize` | `input/comment_list.rs` 13 箇所、`mouse.rs:145` |
| `comment_list_scroll` | `usize` | `input/comment_list.rs:161,163`、`mouse.rs` 経由の読みのみ |
| `viewed` | `HashSet<String>` | **Explorer 内では書かない** — `app.toggle_path_viewed` (App 側) が書き、`render/diff_list.rs:271` が読む |

### 5b. App の他フィールドを読むだけのもの

| パス | 用途 | file:line |
|---|---|---|
| `app.focus` | パネルがフォーカスされているか | `render/mod.rs:30` |
| `app.theme` | 全描画 | `render/{mod,file_tree,diff_list,comment_list}.rs` |
| `app.expanded_panel` | `[<=>]` ボタンの状態 | `render/file_tree.rs:87` |
| `app.ui_tick` | revidere チップのアニメーション | `render/diff_list.rs:143` |
| `app.keymap` | `resolve(&key, KeyContext::*)` | `input/{tree,diff_list,comment_list}.rs` |
| `app.config.viewer.tab_width` | ファイルを開くとき | `input/tree.rs:68`、`input/comment_list.rs:181`、`mouse.rs:219` |
| `app.config.ui.icon_set()` | 字形の選択 | `render/*.rs` 7 箇所 |
| `app.config.layout.explorer_split_pct` | 上下分割比 | `render/mod.rs:35` |
| `app.list_hover.explorer_tree` | ツリーの hover | `render/file_tree.rs:134` |
| `app.list_hover.diff_list` | 差分リストの hover | `render/diff_list.rs:194,232,286` |
| `app.revidere.badge_hover` | チップの下線 | `render/diff_list.rs:138` |
| `app.diff_state.display_list` | 下ペインの行 | `input/diff_list.rs:10,26,39,47`、`mouse.rs:167,172,181` |
| `app.diff_state.files` | ファイル数と +N -N | `render/diff_list.rs:25,115,211` |
| `app.diff_state.error` | タイトルとバナー | `render/{mod,diff_list}.rs` |
| `app.review_state.comment_list_rows` | コメント一覧の行 | `input/comment_list.rs:12,27,86,110`、`mouse.rs:143`、`render/comment_list.rs:66` |
| `app.review_state.comments` | 本文・件数 | `input/comment_list.rs:32,99,115,176`、`render/comment_list.rs:42,99,217` |
| `app.review_state.reply_counts` | `↩N` と展開可否 | `input/comment_list.rs:33,116`、`render/comment_list.rs:118` |
| `app.review_state.expanded_comments` | 展開矢印 | `input/comment_list.rs:100`、`render/comment_list.rs:134` |
| `app.review_state.cached_replies` | 返信行 | `render/comment_list.rs:218` |
| `app.review_state.file_comments` | 現在行のコメント探索 | `input/viewer_actions.rs:44` |
| `app.review_store` | 返信の遅延ロード | `input/comment_list.rs:142`、`input/viewer_actions.rs:64` |
| `app.viewer.content.current_file` | コメント位置のプレフィル | `input/viewer_actions.rs:11` |
| `app.viewer.content.file_scroll` | 同上 (読み) | `input/viewer_actions.rs:23,40` |
| `app.viewer.search.search_active` / `.search_query` | 検索欄の描画 | `render/mod.rs:71,75` |
| `app.terminal.claude.active_session` | Ask Claude All | `mouse.rs:12` |
| `app.overlays.active` | 全画面モーダルの裏で動いているかの判定 | `input/comment_list.rs:17` |

### 5c. App の他フィールドに書き込むもの

| パス | 書く場所 | 何を |
|---|---|---|
| `app.overlays.active` | `input/comment_list.rs:19,154` | 全画面コメント一覧モーダルを閉じる |
| `app.review_state.input_mode` | `input/comment_list.rs:63`、`input/viewer_actions.rs:30` | 返信入力 / コメント追加入力へ |
| `app.review_state.input_buffer` | `input/comment_list.rs:62`、`input/viewer_actions.rs:28` | `clear()` / `set_text()` |
| `app.review_state.input_kind` | `input/viewer_actions.rs:29` | `CommentKind::Suggest` |
| `app.review_state.selected` | `input/comment_list.rs:65` | 返信先のコメント |
| `app.review_state.status_message` | `input/comment_list.rs:66`、`input/viewer_actions.rs` 8 箇所 | 案内・エラー文言 |
| `app.review_state.comment_detail_idx` / `_scroll` / `_active` | `input/comment_list.rs:136-138`、`input/viewer_actions.rs:70-72` | コメント詳細モーダルを開く |
| `app.review_state.cached_replies` | `input/comment_list.rs:145`、`input/viewer_actions.rs:67` | `insert` (遅延ロードの結果) |
| `app.viewer.content.file_scroll` | `input/comment_list.rs:185` | コメント行へジャンプ |
| `app.viewer.selection` | `input/comment_list.rs:189` | コメント行を選択状態に |
| `app.viewer.click.last_tree_click_time` / `_idx` | `mouse.rs:213-214` | ダブルクリック判定の状態 |
| `app.viewer.click.last_comment_click_time` / `_idx` | `mouse.rs:149-150` | 同上 |
| `app.terminal.deferred_prompts` | `mouse.rs:19` | Claude が入力待ちでないときのプロンプト保留 |

### 5d. App / 他モジュールのメソッドを呼んでいるもの

**`App` のメソッド (14 個):**

| メソッド | 呼び出し元 |
|---|---|
| `set_focus` | `input/tree.rs:73`、`input/diff_list.rs:43,54`、`input/comment_list.rs:195`、`mouse.rs:21,118,176,189,237` |
| `set_status` | `mouse.rs:22,27` |
| `refresh_viewer` | `input/tree.rs:16` (**キー入力の入口。同期のファイルシステム走査**) |
| `rehighlight_viewer` | `input/tree.rs:71`、`input/comment_list.rs:184`、`mouse.rs:232` |
| `open_diff_file_at_selected` | `input/diff_list.rs:53`、`mouse.rs:188` |
| `toggle_path_viewed` | `input/diff_list.rs:67` |
| `toggle_comment_expansion` | `input/comment_list.rs:96,102,122` |
| `request_delete_selected_review_item` | `input/comment_list.rs:49` |
| `toggle_selected_review_status` | `input/comment_list.rs:52` |
| `start_edit_selected_review_item` | `input/comment_list.rs:55` |
| `add_review_comment` | `input/viewer_actions.rs:137` |
| `animated_border_color` | `render/file_tree.rs:42,46`、`render/diff_list.rs:106,110` |
| `revidere_artifact_state` | `render/diff_list.rs:132` |
| `is_any_overlay_active` | `render/mod.rs:70` |

**他の状態オブジェクトのメソッド:**
`app.viewer.{open_file, open_file_preview, enter_summary_view, clear_selection,
show_raw_for_line_target, selected_range}` /
`app.diff_state.{collapse_section, expand_section, toggle_section, resolve_file}` /
`app.review_state.{build_file_comment_cache, selected_comment_idx}` /
`app.terminal.pty_manager.{is_waiting_for_input, write_chunked_to_session}` /
`app.review_store.get_replies` /
`crate::event::open_filename_search(app)` (`input/tree.rs:105` — **App 全体を渡す**)。

### 5e. Explorer の外にある Explorer 用の状態

`src/event/mouse/mod.rs` が Explorer のために書くもの。`src/explorer/` の外にあるが
Explorer の描画にしか使われない。

- `app.list_hover.explorer_tree` / `.diff_list` (`event/mouse/mod.rs:748-749`, `:758-759`)
- `app.revidere.badge_hover` (`event/mouse/mod.rs:753`)
- `app.explorer.focus_on_diff_list` (`mouse.rs:122,195` は explorer 側だが、
  呼び出し元は `event/mouse`)

### 5f. 総数

**Explorer が読んでいる `App` のトップレベルフィールドは 15 個** —
`explorer` / `viewer` / `diff_state` / `review_state` / `review_store` / `config` /
`theme` / `list_hover` / `revidere` / `terminal` / `overlays` / `focus` /
`expanded_panel` / `ui_tick` / `keymap`。

うち `explorer` (自分の状態、16 フィールド) を除いた**他パネルの状態が 14 個**。
それらをフィールドパスまで展開すると **45 パス** (メソッド呼び出しを除く。
`review_state` が 16、`viewer` が 9、`diff_state` が 3、`config` が 3、残りが 14)。
加えて **`App` のメソッドを 14 個**、他状態オブジェクトのメソッドを 15 個呼ぶ。

**切り離しの判断材料としての要点:**

- **描画が状態を書き戻す。**`render/mod.rs:55-57` が `tree_height` /
  `diff_list_height` / `diff_banner_rows` を書く。これらは入力処理 (スクロール、
  マウスの行解決) が必要とするので、描画が一度も走っていない状態では入力が正しく
  動かない。`render` の引数が `&mut App` なのはこのため。
- **`review_state` への依存が突出している (16 パス)。**下ペインの半分は
  レビューコメント一覧で、その状態は `review_state` にある。`input/viewer_actions.rs`
  (138 行) に至っては Explorer の描画にも入力にも関与せず、**Viewer から呼ばれる
  コメント操作**が置いてあるだけ (`input/mod.rs:7-9` が自認している)。
- **`viewer` への双方向依存。**Explorer は `viewer.open_file` を呼び、
  `viewer.click.last_*_click_*` にダブルクリック状態を書き、`viewer.search` を
  読んで検索欄を描く。逆に `tree.rs` は「ExplorerState は ViewerState を知らない」
  という規約を持ち `TreeReload` で戻す — **`tree.rs` だけがその規約を守っていて、
  `input/` と `mouse.rs` は守っていない。**
- **`app` 全体を渡す関数が 1 つある** (`crate::event::open_filename_search`)。
## 6. 画面に出るが状態を持たないもの

すべて描画のたびに組み立て直される文字列。どこにも保存されない。

### 6a. ペインのタイトル

| 文字列 | 条件 | file:line |
|---|---|---|
| ` {icon}Explorer ` | 全項目が 1 画面に収まるとき | `render/file_tree.rs:69` |
| ` {icon}Explorer ({選択の可視位置+1}/{可視総数}) ` | `visible.len() > inner_height` のとき | `render/file_tree.rs:62-67` |
| 上の末尾に `{PANEL_REVIEW アイコン}` を追加 | `app.revidere.has_review()` | `render/file_tree.rs:74-76` |
| ` {icon}Changed files ({files.len()}) ` | 通常 | `render/diff_list.rs:321-323` |
| ` {icon}Changed files ({files.len()}) — diff error ` | `diff_state.error.is_some()` | `render/diff_list.rs:321` |
| ` {icon}Comments ({未解決数}/{総数}) ` | 常時 | `render/comment_list.rs:49-52` |

タイトルアイコンは `PANEL_EXPLORER` / `PANEL_CHANGED` / `PANEL_COMMENTS` /
`PANEL_REVIEW`。いずれも `Glyph::nerd_only` なので **Unicode アイコンセットでは
空文字**になる (`icons.rs:283-295`)。

`Explorer` の件数は**選択位置 / 可視総数**、`Changed files` は**ファイル総数**
(`display_list` の行数ではない)、`Comments` は**未解決 / 総数**。3 つとも意味が違う。

`— diff error` サフィックスの存在理由 (`render/diff_list.rs:312-317`):
「何かが失敗してベースからの変更が欠けている場合と、本当に (0) である場合を
区別するため。これが無いと両者は同じ見た目になる」。「あえて "base error" とは
していない: base ref の解決失敗はよくある原因の一つに過ぎず、HEAD が解決できない
場合や merge-base が見つからない場合もここに含まれる」。テストで固定
(`render/diff_list.rs:378-400`)。

### 6b. 枠に載る要素

| 要素 | 位置 | 条件 | file:line |
|---|---|---|---|
| revidere 状態チップ ` {marker} review ` | 上枠・右寄せ | `bottom_view == DiffList` かつパネルが十分広い | `render/diff_list.rs:39-44,131-148` |
| ` ✨ Ask Claude All ` | 下枠・右寄せ | `bottom_view == Comments` のとき常時 | `render/comment_list.rs:55,71-77` |
| ` {first}-{last}/{total_rows} ` | 下枠 | コメント一覧が溢れているとき | `render/comment_list.rs:79-87` |
| `[<=>]` 展開ボタン | 上枠 | ツリーのみ (`with_expand_button`) | `render/file_tree.rs:87` |
| スクロールバー | 右端 | ツリーのみ、溢れているとき | `render/file_tree.rs:172-184` |

revidere チップの marker は 4 状態 (`ui/common/mod.rs:32-40`):
`Running` → スピナー、`Fresh` → `▤`、`Stale` → `!`、`None` → `○`。
**幅は常に 10 セル** (`REVIDERE_BADGE_W`)。「状態が変わっても幅は変わらない。
当たり判定はここから導いていて、描画された文字列を測っているわけではないので、
状態ごとに幅が動くと押せる場所がずれる」(`render/diff_list.rs:13-17`)。テストで固定
(`render/diff_list.rs:452-467`)。

`Ask Claude All` のラベル色は `Color::Rgb(180, 140, 255)` の**ハードコード**
(`render/comment_list.rs:74`) — テーマトークンを通らない唯一の色。

### 6c. 行に出る記号

| 記号 | 意味 | file:line |
|---|---|---|
| `▣` | 変更ファイル一覧の SUMMARY 行 | `render/diff_list.rs:295` |
| `SUMMARY` | 同上 | `render/diff_list.rs:298` |
| `⚠ {msg}` | 差分リストのエラーバナー | `render/diff_list.rs:162` |
| `+{N}` / `-{N}` | 追加行数 / 削除行数 | `render/diff_list.rs:260,264` |
| `💬{N}` (`COMMENT` グリフ) | コメント数バッジ | `render/diff_list.rs:365-368` |
| `✓` | viewed 印 (変更ファイル一覧) / 解決済み (コメント一覧) | `render/diff_list.rs:273`、`render/comment_list.rs:103` |
| `○` | 未解決コメント | `render/comment_list.rs:103` |
| `↩{N}` | 返信数 | `render/comment_list.rs:127` |
| `+{N}` | コメント本文の残り行数 | `render/comment_list.rs:149` |
| `↳ {You\|Claude}` | 返信行 | `render/comment_list.rs:253,261` |
| `!` / `?` | コメント種別 (Suggest / Question、Unicode セット) | `icons.rs:275-278` |
| `›` / `⌄` | 折りたたみ / 展開の矢印 (Unicode セット) | `icons.rs:243-250` |

### 6d. ステータスメッセージ

Explorer 起点で `set_status` / `status_message` に流れる文言。

| 文言 | レベル | 契機 | file:line |
|---|---|---|---|
| `Sent all comments to Claude` | Info | Ask Claude All 成功 | `mouse.rs:23` |
| `No active Claude Code session` | Warning | Claude セッションが無い | `mouse.rs:28` |
| `Reply to comment (Enter to send, Esc to cancel)` | — | `r` で返信入力へ | `input/comment_list.rs:67` |
| `Add comment: [s:\|q:]file:line body` | — | Viewer からコメント追加 | `input/viewer_actions.rs:31` |
| `Empty input, cancelled.` | — | 空入力 | `input/viewer_actions.rs:81` |
| `Format: file:line body  (e.g. src/main.rs:42 fix this)` | — | 区切りが無い / `:` が無い | `input/viewer_actions.rs:95,109` |
| `Comment body is empty.` | — | 本文が空 | `input/viewer_actions.rs:103` |
| `Invalid line number: '{s}'` | — | 行番号のパース失敗 (3 箇所) | `input/viewer_actions.rs:121,125,131` |

### 6e. 空のときのメッセージ

**無い。**3 リストとも、項目が 0 件のときは**タイトルの件数表示だけ**が
`(0)` や `(0/0)` になり、リスト本体は空欄になる。「変更なし」「コメントはまだ
ありません」に相当する案内文はどこにも無い。これが `— diff error` サフィックスが
必要になった理由でもある (6a 参照)。

### 6f. ファイル名検索の入力欄

`render/search_box.rs`。Explorer 領域の**下から 2 行目**に 1 行で重ねる
(`search_box.rs:16-18`)。`/{カーソル前}█{カーソル後}` を `theme.search_match_fg` で
描き、オーバーレイが出ていなければ端末カーソルも置く。
**ただし表示条件は `app.viewer.search.search_active`** — Explorer 自身の検索状態
ではなく Viewer の検索状態を見ている (`render/mod.rs:71`)。

## 7. 直感に反する振る舞い

0 から書き直すときに落としやすいもの。コメントが残している「なぜ」ごと引用する。

### 7-1. `diff_banner_rows` — 画面行をインデックスに戻すとき差し引く 1 行

差分リストのエラーバナーは**リストの先頭に描かれるが `display_list` の要素では
ない**。そのため画面上の各エントリは、インデックスが示す位置よりバナー分だけ
下にずれる。

> このバナーは display_list の一部ではないので選択もできず、ナビゲーションキーが
> 扱うインデックスもずらさない — コストはリストの高さ 1 行分だけ。
> (`explorer/mod.rs:74-78`)

> バナー自体の上のセルはどのエントリの上でもない: これがないと、メッセージを
> クリックしたときにたまたま一番上にスクロールされていた項目が開いてしまう。
> (`mouse.rs:107-108`)

寸法は `diff_list_banner_rows(has_error)` が単一の情報源で、**3 箇所が合わせる**
必要がある (`render/diff_list.rs:327-336`): レンダラ (何行分のエントリが収まるか)、
スクロールのページサイズ、マウスの行→インデックス変換。「以前はこれらがずれて
しまうことがあり、1 行のずれがクリック時に別のファイルを静かに開いてしまっていた」。
テストで固定 (`render/diff_list.rs:402-409`, `event/mouse/tests.rs:302-313`)。

`diff_list_row_at` がバナーのオフセットを引数で取るのも意図的:
「このオフセットは両方の呼び出し側 (クリックとホバー) が必要とするため、
呼び出し側ではなくここに置いている」(`mouse.rs:82-84`)。

### 7-2. ツリーの下枠を弾かないと、画面に出ていないファイルが開く

`explorer_tree_row_at` は `row >= explorer_mid_y.saturating_sub(1)` で**2 行**
弾く。バグの経緯がそのままコメントに残っている:

> explorer_mid_y は「変更されたファイル」パネルの上枠なので、ファイルツリー自体の
> 下枠はその 1 行上にある。両方とも弾く必要がある: ツリーは height - 2 行の
> コンテンツを描画するので、枠を通してしまうと scroll + inner_height — 実際に
> 描画された最後の行のさらに 1 つ先 — が返ってしまう。
>
> 通常この問題は隠れている。divider_at がリサイズ用にこの 2 行を先に取ってしまう
> からだが、divider_draggable はパネルが最大化されている間 false を返す。そのため
> Explorer を最大化した状態で水平線をクリックするとここに落ちてきて、画面に出て
> いないファイルが開いてしまうバグがあった。兄弟にあたる diff_list_row_at は
> 元々自分の下枠を除外していたので、この非対称性がバグの正体だった。
> (`mouse.rs:50-59`)

**教訓の一般形**: ヒットテスタは「上に何かが被さっているから大丈夫」に依存しては
ならない。被せる側 (`divider_draggable`) は最大化や埋め込みエディタで無効化される。

テストが「パネルが実際に描画する行数」に結び付けている
(`event/mouse/tests.rs:242-265`): 「単に『行 N はインデックス M に対応する』という
だけのアサーションは、その関数がたまたまやっていることをなぞるにすぎない」。

### 7-3. クリックとホバーは同じ解決関数を通らなければならない

`explorer_tree_row_at` / `diff_list_row_at` の両方がクリックハンドラと
ホバートラッカーから呼ばれる。

> これにより、ハイライトされた行とクリックで開かれる行が食い違うことは構造的に
> あり得ない。(`mouse.rs:39-40`)

同じ規約が revidere チップにもある (`event/mouse/mod.rs:254-256`) し、
`revidere_badge_cols` は「描画側もクリック側もこれで揃って諦めるので、見えない
チップは押せない」(`render/diff_list.rs:19-20`)。

**ただしコメント一覧だけこの規約の外にある。**行の解決が `mouse.rs:142` に
インラインで書かれていて、ホバーも無い。

### 7-4. 変更ファイル一覧をクリックすると、ツリーのカーソルも動く

`open_diff_file_at_selected` が `reveal_file_in_tree(&file_path)` を呼ぶ
(`app/review.rs:54`)。つまり下ペインでファイルを開くと、上ペインで途中の
ディレクトリが展開され、`tree_selected` と `tree_scroll` が動く。
Explorer の 2 ペインは独立していない。

同じ関数はさらに `expand_threads_for_file` と `build_unified_diff_view` を呼び、
着地点を決める: 「ファイルにレビューコメントがあれば最初のコメントへ着地させ
(レビュアーがすぐ気付けるようにする)、なければ最初の変更箇所へ着地させる」
(`app/review.rs:59-60`)。

### 7-5. コメント一覧のスクロールのページ幅は `diff_list_height`

`input/comment_list.rs:159` が `app.explorer.diff_list_height` をページ幅に使う。
**`comment_list_height` というフィールドは存在しない。**下ペインは差分リストと
コメント一覧で領域を共有しているので実測値としては正しいが、
`diff_list_height` は `render/mod.rs:56` で**バナー行を引いた後**の値である。
`shows_error_banner` が `bottom_view == DiffList` を条件に含む
(`render/mod.rs:49-51`) おかげで、コメント一覧表示中は引かれない — この 2 つの
条件が噛み合って初めて正しい。片方だけ変えると 1 行ずれる。

さらに `scroll.rs` の `adjust_diff_list_scroll` と同じロジックが
`input/comment_list.rs:157-164` に**手で複製**されている。

### 7-6. エラーバナーは 1 行に潰さなければならない

> 改行はスペースに潰す。複数行の ListItem はここで確保した 1 行より多くの行を
> 静かに消費してしまい、List ウィジェットはパネル端で溢れた分を切り捨てるだけ
> だから。(`render/diff_list.rs:156-159`)

`msg.replace('\n', " ")` (`render/diff_list.rs:162`)。これを忘れると
`diff_banner_rows` の 1 という前提が崩れ、7-1 のずれが復活する。

### 7-7. git status が取れなかったときの空マップは「無害なフォールバック」ではない

> 空のマップは無害なフォールバックではない — エントリが無いと、ツリー上は全て
> Tracked、Changed files 上は全て Committed (緑) に見えてしまい、UI が
> 「未ステージの変更がある」の正反対を黙って主張してしまう。git 管理外の
> ディレクトリを開いた場合はこの経路を正当に通る (発見すべきリポジトリが
> 無いのだから)。一方、実在するリポジトリ内での一時的な失敗 (並行して走る git
> コマンドが index.lock を握っている、など) は画面上見分けがつかないので、
> ログだけが両者を区別する手段になる。(`tree.rs:110-118`)

**画面には何も出ない。`log::warn!` だけ**が両者を区別する。

### 7-8. hover は行の色を「自分の色相のまま」強めなければならない

固定色 (`theme.accent`) に寄せる実装は**嘘をついていた**。

> Changed files リストでは前景色が git のステージ状態を符号化しており、
> solarized-dark と gruvbox の両方で accent == warning (ステージ済みの色) である
> — そのため *未ステージ* の行を hover すると *ステージ済み* の行とまったく
> 同じ色に塗り替わってしまっていた。hover がユーザに working tree について
> 誤った情報を伝えていたことになる。github-light では accent == info で同様の
> 問題があり、hover したファイルがディレクトリに見えてしまっていた。
> (`list_row.rs:192-201`)

回帰テストが全 11 テーマ × 5 意味色で検証する
(`list_row.rs:380-417`, `hover_never_repaints_a_row_as_another_meaningful_token`)。

**押し出し量も固定にできない**: 「以前の lighten(base, 0.45) と比較すると、
theme.fg の行は約 53 しか動かなかったのに対し theme.hint (untracked) の行は
約 237 動いた: hover は最も頻繁に発火する箇所でちょうど 4 倍弱くなっており、
これが『控えめ』ではなく『あてにならない』と受け取られる原因になっていた」
(`list_row.rs:163-168`)。

**寄せ先の候補が 2 つあるのも必然**: 「既に明るい方のターゲットに位置している
行の色は、theme.fg が白へ向かって余地がないのとまったく同じように、そちらへ
向かう余地がない。2 つ目のターゲットは反対側にあるので、どちらか一方には
常に余地がある」(`list_row.rs:171-177`)。

### 7-9. hover の背景色は使えない / 下線はフェードに持ち越さない

> 背景色で表現する方式も試したが却下した: 11 テーマ中 7 テーマで
> selected_bg_inactive と区別が付かなかった。これはまさに hover 中だが
> フォーカスされていない行が置かれる状態そのものである。(`list_row.rs:95-98`)

> FadingOut には意図的に持ち越さない: 下線は「ポインタが *ここにある*」ことを
> 示すものであり、離れた瞬間に真ではなくなる。またモディファイアは補間できないので、
> どのみちフェードの途中のどこかで唐突に消えるしかない。(`list_row.rs:135-137`)

`fading_out_starts_lit_and_ends_at_base` テストの理由も明示的:
「lerp の 2 つの色引数を入れ替えてもコンパイルは通りアニメーションもする —
ただし、ポインタが離れた後に行が明るくなるという逆の動きが再生されてしまう」
(`list_row.rs:355-358`)。

### 7-10. 下線はインデントに引いてはいけない

> インデントの下にも下線を引くと、ネストされた行の下線がクリック可能な要素より
> ずっと左から始まってしまい、ポインタのアフォーダンスではなく行全体に渡る
> テキスト入力の下線のように見えていた。(`list_row.rs:150-153`)

これが「1 行を prefix / icon / name の span に割る」構造の理由。
`decoration_style` は BOLD (選択) は残す — 「さもないとプレフィックス部分が
それが属する名前より薄く描画されてしまう」(`list_row.rs:504-506`)。

### 7-11. hover の再セットでフェードを再開してはいけない

> マウス移動イベントのたびに同じ行をセットし直しても (ポインタが静止している間に
> よく起きる) フェードアニメーションをその都度リスタートしてはならない。
> (`list_row.rs:38-41`)

`HoverRow::set` の冒頭 `if self.row == row { return; }` がそれ。

### 7-12. オーバーレイが開くとき、先にホバーを消す

> ここで return すると、オーバーレイが開いている間は背景のパネルが Moved イベントを
> 受け取らなくなる。Moved ハンドラはマウスがそこから離れたときにツリー/差分リストの
> 行ハイライトやジャンプ下線、ホバーポップアップを自然にクリアする役目を持つが、
> それが働かなくなるということ。なので先にここでクリアしておき、モーダルの裏に
> 何も光ったまま残らないようにする。(`event/mouse/mod.rs:471-476`)

「イベントを消費する」だけでは足りず、`app.clear_all_hover()` が必要。

### 7-13. revidere チップは境界より先に判定しなければ押せない

> revidere の状態チップは Explorer の横境界と同じ行にあるので、境界より先に見る。
> そうしないと 10 セルぶんが常に境界に食われて押せない。チップは右枠の内側にあり、
> 縦の境界のセルとは重ならない。(`event/mouse/mod.rs:610-612`)

チップの幅が状態で変わらないのも同じ系統: 「当たり判定はここから導いていて、
描画された文字列を測っているわけではないので、状態ごとに幅が動くと押せる場所が
ずれる」(`render/diff_list.rs:15-16`)。テストが ratatui の右寄せ実測と突き合わせる
(`render/diff_list.rs:469-505`): 「この 2 つは別々に計算されているので、ratatui の
右寄せの寸法が変われば、クリックだけ 1 セルずれる、という壊れ方をする」。

### 7-14. ツリーとコメント一覧は「同じ 1 クリック」で挙動が違う

| リスト | シングルクリック | ダブルクリック |
|---|---|---|
| ツリー・ファイル | preview タブで開く、**フォーカスは Explorer に残る** | 永続タブ、Viewer へフォーカス |
| ツリー・ディレクトリ | 開閉 | (判定なし) |
| 変更ファイル一覧 | **常に**開いて Viewer へフォーカス | (判定なし) |
| コメント一覧 | ジャンプするがフォーカスは残る | ジャンプして Viewer へ |

ツリーの preview/永続の使い分けの理由: 「開いたまま溜まるのを防ぐ」
(`mouse.rs:220-221`)。**変更ファイル一覧にはこの区別が無く、シングルクリックで
永続タブが増える。**

ダブルクリック判定は `register_double_click_on` で**同じ idx への連続クリック**も
要求する (`event/mouse/mod.rs:78-88`)。`register_double_click` は「必ず先に実行
するので、*last はどちらにせよ更新される」— 短絡評価にしてはいけない。

### 7-15. 全画面コメント一覧は下ペインと同じハンドラで動く

`handle_explorer_comment_list_key` が両方を処理し、
`app.overlays.active == ActiveOverlay::CommentList` で区別する
(`input/comment_list.rs:17`)。モーダルを閉じる条件が非自明:

> Select が実際に位置へジャンプしたときだけモーダルを閉じる — 返信を持つ
> コメントへの Select はその場でスレッドを開くだけなので、その場合はモーダルを
> 開いたままにしておく必要がある。(`input/comment_list.rs:22-24`)

`close_after` は**アクションを実行する前**に計算される (`input/comment_list.rs:25-42`)
— 実行後だと `comment_list_rows` が変わって判定できない。

**そして全画面モーダルにマウス操作は無い** (2a 参照)。

### 7-16. `d` / `c` は下ペインからも効き、必ずフォーカスを下ペインへ移す

サブパネルへ委譲する**前**に判定する (`input/tree.rs:19-33`)。`bottom_view` と
`focus_on_diff_list` を**両方**書くので、下ペインで既に差分リストを見ている状態で
`d` を押しても副作用がある (フォーカスが下ペインに固定される)。
`KeyContext::Explorer` が `focus_on_diff_list` で変わらない (`types.rs:40`) ことの
帰結 — 冒頭の「設計にそのまま効く 3 点」の 1 と 2。

### 7-17. キー入力がファイルシステム走査を起こす

```rust
if app.explorer.tree.file_tree.is_empty() {
    app.refresh_viewer();
}
```
(`input/tree.rs:15-17`)。**どのキーでも**、ツリーが空なら同期の走査が走る。
`Esc` でも `q` でも。

### 7-18. 根とエントリは同時に差し替えなければならない

> 別々に書けるようにしておくと「根は新しいのにエントリは古い」状態が作れてしまい、
> その瞬間のクリックは別ブランチの同名ファイルを静かに開く (worktree 切り替えは
> 走査を裏に回すので、この隙間は実在する)。(`tree.rs:48-51`)

> 根が空のまま相対パスを繋ぐとカレントディレクトリ相対になり、意図しないファイルを
> 黙って開くので、ツリーを空にする側は必ず [set_root] も呼ぶ。(`tree.rs:40-41`)

`root_changed` を返す理由も同じ系統: 「同じ根への再走査では触ってはならない
(一時的に消えたファイルのタブまで勝手に閉じてしまう)」(`tree.rs:55-56`)。

### 7-19. `.get()` を使うのはパニック回避のためであって、行儀の問題ではない

> インデックスアクセスではなく .get を使う: display_list とファイル vector は
> 異なるティックで再構築されるため、片方が古いままレンダリングされるフレームが
> ありうる。行をスキップすればチラつきで済むが、インデックスアクセスだと描画処理の
> 内側からアプリ全体を落としかねない。上のファイルツリーも同様の対応をしている。
> (`render/diff_list.rs:205-210`)

`jump_to_changed_file` も同じ理由でカーソルをクランプする
(`app/review.rs:97-100`): 「古い diff_list_selected (リフレッシュでリストが縮んだ
場合など) が下の後方スキャンでリスト範囲を超えてはならない。超えると
display_list[i] がパニックする」。

### 7-20. `walk_dir` が 1 階層しか読まないのは意図的

> 初回の走査と遅延展開の両方をこの 1 つの関数でまかなうのは意図的な設計。
> もともとは同一ロジックの別々のコピーだったが、git_status パラメータを両方に
> 通す必要が出たとき、次の乖離が発生するのは時間の問題だった。(`tree.rs:453-457`)

その代償が `ensure_children_loaded` の `tree_selected` 補正 (`tree.rs:397-400`):
フラット Vec の途中に `splice` するので、挿入位置以降を指すカーソルを手で押し下げる。

### 7-21. `classify` は祖先を遡るが `status` は遡らない

> libgit2 は ignored ディレクトリを、中のファイル 1 つずつではなく末尾スラッシュ
> 付きの折りたたまれた 1 エントリ (例えば "target/") として報告するので、
> target/debug/foo のようなネストしたパスにはそれ自身のエントリがなく、祖先から
> Ignored を継承する必要がある。untracked ディレクトリではこれは不要 —
> recurse_untracked_dirs(true) がすでにファイル単位に展開してくれる。
> (`status_map.rs:95-101`)

> ignored な祖先 (折りたたまれたディレクトリエントリ) だけが下位へ伝播する。
> tracked な祖先はこの特定のパスについて何も教えてくれない。(`status_map.rs:114-117`)

さらに「git がまだ見たことのないディレクトリ」への追加判定:

> このチェックなしにここへ到達すると、新規ディレクトリが通常の tracked 色で
> 描画される一方で中身は薄暗く表示されてしまう — 親が「既知」で子が「新規」に
> 見えるのは逆である。(`status_map.rs:125-130`)

**ツリーは `classify`、変更ファイル一覧は `status` を使う** — 同じ `GitStatusMap`
から違う粒度を引いている。

### 7-22. WT_* を INDEX_* より先に見る

> 編集して git add し、さらに編集する、といった操作をすると WT_* と INDEX_* の
> 両方のビットが立つことがある。この場合は unstaged を優先させたいので WT_* の
> チェックを先に行う。(`render/diff_list.rs:58-61`)

テストの理由付け: 「作業ツリーの編集の方がより新しく重要な状態であり、"staged" と
表示すると staged の上にさらに uncommitted な変更があることが隠れてしまう」
(`render/diff_list.rs:518-522`)。

### 7-23. `theme.muted` は薄暗い表示に使えない

> ここで theme.muted を意図的に避けているのは、solarized-dark では背景と同じ RGB で
> 事実上見えなくなり、github-light ではボーダー色に見えてしまうため。
> (`file_tree.rs:118-122`)

代わりに `theme.hint`。同種の注意が revidere の色にもある: 「muted は複数のテーマで
見えなくなるので使わない」(`ui/common/mod.rs:42`)、さらに「この色は必ず素の背景の上で
使う。選択中の worktree チップのような塗りの上に重ねてはいけない — 全テーマで
accent と selected_bg が同じ色なので、実行中の印が背景と完全に同色になって消える」
(`ui/common/mod.rs:44-46`)。

### 7-24. アイコンの種別色は選択行では捨てる — ただし規則が 2 つある

> 選択の背景色の上で種別色が読める保証は 11 テーマぶんには無く、untracked/ignored の
> 減光はアイコンにも及ぶべきだからである。(`file_tree.rs:144-146`)

ツリーは「選択行 **または** non-Tracked」で譲る (`file_tree.rs:147-151`)。
変更ファイル一覧は「選択行のみ」(`diff_list.rs:249`)。同じ理由から出発して
条件が違う。

### 7-25. 解決済みコメントのマーカーを明るくしてはいけない

> 解決済みの行はマーカーも含めて完全に後退させる。ミュートな本文の上に明るい ✓ が
> 乗ると、もう注意を払う必要のない行にこそ目が引き寄せられてしまうため。
> (`comment_list.rs:184-187`)

### 7-26. コメント本文は 1 行目だけ + `+N`

> 改行をスペースに潰すとコメントに構造があったことが分からなくなるため、+N で
> 残りの行数を示す。(`comment_list.rs:143-145`)

7-6 (エラーバナーは潰す) と**逆の判断**をしている。バナーは行数の契約があるので
潰すしかなく、コメントは行数の契約が無いので情報を残せる。

### 7-27. コメントへのジャンプは必ず raw 表示にする

> コメントはソース行に紐づくので source を表示する: markdown レンダリングだと
> 本文の先頭に飛ばされ、選択箇所が見えなくなってしまう。(`input/comment_list.rs:186-187`)

`show_raw_for_line_target()` (`input/comment_list.rs:188`)。

### 7-28. ホイールは選択を動かさない (Worktree パネルとは逆)

Explorer のホイールは `tree_scroll` / `diff_list_scroll` だけを動かす
(`event/mouse/scroll.rs:66-105`)。同じ関数の中で **Worktree パネルは
`row_selected` を動かす** (`scroll.rs:51-65`)。

さらに 2 ペインでクランプ規則が違う: ツリーは `visible_count - tree_height`
(最終行が下端に来たら止まる)、差分リストは `display_list.len() - 1`
(最後の 1 件だけが残る位置まで送れる)。

### 7-29. hover は「見えていないリスト」にも記録される

`Moved` ハンドラは `bottom_view` を見ずに `diff_list_row_at` を呼び、
`list_hover.diff_list` を毎回書く (`event/mouse/mod.rs:756-759`)。
コメント一覧を表示している間もマウスの下の「差分リスト行」が記録され続ける。
描画されないので画面には出ないが、`is_animating()` は真になり得る。

### 7-30. `SUMMARY` 行だけ BOLD を手で足す

> 非選択の SUMMARY 行は hover の有無に関わらず太字にする。row_style は選択時以外は
> BOLD を適用しないため。(`render/diff_list.rs:288-290`)

共有ヘルパーが「選択でないなら BOLD なし」を決め打ちしているせいで、
呼び出し側が例外を足し戻している。

### 7-31. Tab はサブフォーカスを停留点にする

> Explorer 列は独立した 2 つのパネル — ファイルツリーと変更ファイル一覧 — を持つので、
> Tab はそれぞれを個別の停止点として訪れ (ファイルツリー → 変更ファイル → Viewer)、
> 次へ進む前にサブフォーカスを切り替える。(`app/focus.rs:147-150`)

さらに方向で着地点が違う: 「他のどこからであれ Explorer 列に着地したときは、
常にファイルツリー (上のパネル) から始まる」(`app/focus.rs:161-162`) /
「Viewer 側から Explorer 列に入ると、(一番近い) 変更ファイルパネルに着地するので、
さらに Tab で戻るとツリーに到達する」(`app/focus.rs:181-183`)。

**`focus_on_diff_list` は Explorer の外 (`app/focus.rs`) からも書かれる。**

### 7-32. ファイル名検索欄は Viewer の検索状態で出る

`render/mod.rs:71` の条件は `app.viewer.search.search_active`。Explorer 領域の
下から 2 行目に描かれるのに、状態は Viewer 側。一方 `/` キー
(`Action::SearchFilename`) が開くのは `viewer.filename_search` の全画面モーダルで、
これは**別物**。同じパネルに 2 種類の検索 UI が同居していて、片方は Explorer 領域に
描かれ、もう片方は最上位に描かれる。

後者を最上位に描く理由: 「このパネルが幅ゼロまで畳まれていても (Viewer 最大化中
など) 見えたままにするため」(`render/mod.rs:81-83`)。

### 7-33. `Ask Claude All` のラベル幅がハードコードで二重管理

描画側は `const ASK_CLAUDE_ALL_LABEL: &str = " ✨ Ask Claude All ";`
(`render/comment_list.rs:55`)、当たり判定側は `let ask_label_w = 19_u16;`
(`mouse.rs:128`)。定数を共有していない。revidere チップ (7-13) が
`revidere_badge_cols` で描画側と当たり判定を揃えているのと**対照的**。

### 7-34. 描画が入力用の状態を書き戻す

`render/mod.rs:55-57` が `tree_height` / `diff_list_height` / `diff_banner_rows` を
毎フレーム書く。スクロールのページ幅とマウスの行解決がこれを読む。
つまり**一度も描画されていない状態では、キー入力もマウスも正しく動かない**。
`ExplorerState::default()` が `tree_height: 20` / `diff_list_height: 20` という
架空の値で初期化しているのはそのため (`mod.rs:96-97`)。

`banner_rows` をここから公開する理由も明示されている: 「どのビューが表示中かを
唯一知っているここから公開する」(`render/mod.rs:45-48`)。

### 7-35. 上下分割の比率計算は 2 箇所で一致していなければならない

> マウスの当たり判定を描画と一致させるため LayoutCache の explorer_mid_y と同じ
> 計算にすること。(`render/mod.rs:33-34`)

`Layout::vertical([Percentage(tree_pct), Percentage(100 - tree_pct)])` と
`LayoutCache::explorer_mid_y` が別々に計算される。コメントによる規約でしか
繋がっていない。

