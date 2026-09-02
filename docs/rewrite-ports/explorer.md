# explorer

旧テスト 16 本 → 新テスト 19 本 (移植 14 / 削除 2)

新の置き場: `crates/conductor-tui/src/panels/explorer/{mod,tree,render}.rs`。
リストの選択とスクロールは 2 区画で共有するので `crates/conductor-tui/src/list.rs`
(旧 `src/widget/list.rs` を丸ごと移設、テスト 11 本もそのまま) に置いた。

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| keys: エラーのバナーは下のペインを1行押し下げる | 移植 | バナーはエラー時にだけ1行使う (`banner_rows` が 0/1 を返し、`sync_layout` がその分だけ窓を下げる。旧の `Panes::split` は layout.rs の Region 分割に置き換わった) |
| keys: コメント一覧はdiffのエラーを無視する | 削除 | コメント一覧はフェーズ 3b。下区画が Changes だけになったので、バナーの有無で分岐する相手がいない |
| pointer: 変更ファイル一覧のクリックはツリーと同じ意味を持つ | 移植 | previewのタブは1枚だけで永続で開き直すと固定される (viewer 側。1 クリック = preview、Enter = 固定という契約はそのまま。`ExplorerPanel::click` が preview で開く) |
| render/changes: エラー時の見出しは本当の0件と区別できる | 移植 | エラーの見出しは本当の0件と区別できる |
| render/changes: エラー時の見出しも件数を残す | 移植 | 同上 (件数を含むことを同じ 1 本で確認) |
| render/changes: バナーはエラー時にだけちょうど1行使う | 移植 | バナーはエラー時にだけ1行使う (改行をスペースに潰すことも同じ 1 本で確認) |
| render/changes: gitの各状態は自分の色に解決する | 移植 | 変更ファイルの行は増減とviewedを添える に統合 + `stage_color` は旧のまま。5 ケースの表は色の対応が旧と同一なので、行の組み立て側で 1 本 |
| render/changes: どの状態も同じ幅で描かれる | 削除 | revidere の状態チップはフェーズ 6。幅を固定する相手が無い |
| render/changes: 当たり判定はratatuiが実際にバッジを置く場所と一致する | 削除 | 同上 |
| render/changes: 狭いパネルではバッジを出さない | 削除 | 同上 |
| tree: 走査はgitignoreされたディレクトリも含めて潜る | 移植 | 走査はgitignoreを含み重いディレクトリだけを飛ばす |
| tree: 走査は重いディレクトリは飛ばす | 移植 | 同上 (SKIP_DIRS は旧のまま持ち越し) |
| tree: ツリーの根とエントリは一緒に入れ替わる | 移植 | 根とエントリは一緒に入れ替わる + 根だけ差し替えるとエントリは捨てる (旧 `set_root` は根だけ書いてエントリを残していた。新は捨てる) |
| tree: ツリーの再読み込みはsummary表示を保つ | 移植 | タブを移ると読みかけの位置が戻りディスクから読み直す (viewer 側)。走査は Viewer に触らなくなったので、「読み直しても見ている場所が動かない」は Viewer 自身のタブの不変条件になった |
| tree: ツリーの再読み込みはdiff表示を保つ | 移植 | 同上 |
| tree: ツリーの再読み込みはmarkdownのスクロールを保つ | 削除 | markdown のレンダリング表示はフェーズ 5。保つ状態そのものが無い |

新規 (旧に無かった事実):

- 展開したディレクトリは走査し直しても開いたまま: 走査が svc のワーカーへ移り、展開集合を引数で渡すようになった
- ディレクトリは展開すると子が現れ畳むと隠れる: 遅延読み込みが 1 度きりであること
- revealは途中のディレクトリを開いて可視添字を返す
- 区画ごとにキーマップの層が変わる: `Workspace::key_context` が持ち主に訊く配線
- 変更一覧の移動は両端で止まる / 変更一覧のenterはdiffを添えて開く / viewedは押すたびに入れ替わる
- diffの入れ替えは選択をパスで持ち越す: 3 秒ポーリングで選択が飛ばないこと
- baseを解決できなければ理由をステータスに出す
- あいまい検索は前方一致を先に出す
- 変更なしと読み込み前は別の行になる / 見出しは窓に入り切らないときだけ件数を出す
- ホイールは選択を動かさず窓だけ送る
- (run.rs) enterで開いた1枚のタブがworktree切替まで生き残る: route → Effect → svc を通す合成テスト

API で変えた点:

- `Explorer` → `ExplorerPanel`。`FileTreeState` は `tree::FileTree` になり、`visible_indices` の
  `RefCell<Rc<Vec<usize>>>` キャッシュは捨てた (数百件の線形走査で、無効化を忘れる罠の方が高い)
- 走査は `tree::survey(root, expanded) -> tree::Snapshot` に括り出し、svc のワーカーで走らせる。
  git status とファイル名検索の全ファイル列も同じ 1 回の走査から出す (旧は別の再帰だった)
- `Intent` は無くなった。`update` が直接 `Vec<Effect>` を返す
- `DiffState` の持ち主が `App` から Explorer になった。Viewer へは `Effect::OpenFile` に
  `FileDiff` を添えて渡す
- `viewed` は今はメモリ上だけ。永続化は review_store 込みでフェーズ 3b

残したコメント (なぜ):

- `tree.rs` モジュール doc: 根・エントリ・git status を 3 つ揃えて入れ替える理由 (worktree 切替の窓)
- `tree.rs` `survey`: git status が空に落ちたときツリーが「全部 tracked」に見える害
- `mod.rs` `clamp`: 窓の高さを知っているのがここだけである理由
- `mod.rs` `open`: ツリーが出すのは中身であって diff ではないこと
- `render.rs` バナー: 「変更なし」と混同させないこと、改行を潰す理由
- `render.rs` `stage_color`: unstaged を先に見る理由 (WT_* と INDEX_* が同時に立つ)
- `render.rs` File 行: 添字アクセスにしない理由 (display_list とファイル vec のティックのずれ)
