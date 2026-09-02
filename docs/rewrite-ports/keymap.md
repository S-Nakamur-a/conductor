# keymap
旧テスト 37 本 → 新テスト 12 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| edge_cases::keys_for_actionは正規の綴りを並べる | 移植 | 逆引きは正規形の綴りを返す (Worktree NavigateDown = ["down","j"]) |
| edge_cases::アクション名は全バリアントで往復する | 移植 | アクション名は全バリアントで往復する |
| edge_cases::shift付きの小文字は大文字に直されない | 移植 | チョードは正規化してから引く |
| edge_cases::macosのunicodeフォールバックのキーが解決する | 移植 | 既定のキーが解決する (÷ は削除、代わりに † を追加) |
| edge_cases::alt_shift付きの数字はalt付き数字に潰れない | 移植 | 割り当ての無いキーは素通しする |
| edge_cases::diffモードではenterとshift_enterが別物 | 移植 | 既定のキーが解決する |
| edge_cases::keys_for_actionは小文字の正規形を使う | 移植 | 逆引きは正規形の綴りを返す |
| edge_cases::割り当ての無いキーは素通しする | 移植 | 割り当ての無いキーは素通しする (CapsLock) |
| edge_cases::削除したlspのアクションはもう解釈されない | 移植 | 語彙から外したアクション名は解釈されない (+ update_and_restart / check_for_update / toggle_panel_overlay) |
| edge_cases::fキーは割り当てが外れている | 移植 | 割り当ての無いキーは素通しする |
| overrides::ユーザ設定はキーを足せる | 移植 | ユーザ設定は既定に重なる |
| overrides::ユーザ設定は既定のキーを覆う | 移植 | ユーザ設定は既定に重なる (g → grab_branch。旧の go_to_top は既定と同じで上書きを確かめていなかった) |
| overrides::ユーザの打ち消しは既定のキーを外す | 移植 | ユーザ設定は既定に重なる |
| overrides::パネル層での打ち消しも効く | 移植 | ユーザ設定は既定に重なる |
| overrides::知らないアクション名は警告になる | 移植 | ユーザ設定の問題は警告になる |
| overrides::旧形式は黙らずに報告する | 移植 | ユーザ設定の問題は警告になる |
| overrides::同じ層の中の衝突は警告になる | 移植 | ユーザ設定の問題は警告になる |
| overrides::知らない層に割り当てがあれば警告になる | 移植 | ユーザ設定の問題は警告になる |
| resolution::既定は警告なしで組み上がる | 移植 | 既定は警告なしで組み上がる |
| resolution::既定のアクション名は全部解決する | 移植 | 既定は警告なしで組み上がる (同じ関数で DEFAULT_KEYBINDS を直接パース) |
| resolution::要のキーは解決する | 移植 | 既定のキーが解決する / 割り当ての無いキーは素通しする |
| resolution::worktree切替とズームの別名が解決する | 移植 | 既定のキーが解決する |
| resolution::terminalが奪うのは実際に発火するアクションだけ | 移植 | ptyを持つ文脈が奪うのは発火するアクションだけ + ヘルプの逆引きはfires_in_terminalと一致する |
| resolution::terminalで使えるアクションはterminalで全部解決する | 移植 | ヘルプの逆引きはfires_in_terminalと一致する (手書きの 19 個ではなく Action::ALL を fires_in_terminal で両方向に照合) |
| resolution::editorが奪うのは抜けるキーとグローバルだけ | 移植 | ptyを持つ文脈が奪うのは発火するアクションだけ |
| resolution::viewerではctrl_escが追加で効く | 移植 | 既定のキーが解決する |
| resolution::文脈はグローバルへ落ちる | 移植 | 既定のキーが解決する (Terminal alt+l) / 割り当ての無いキーは素通しする (Terminal tab) |
| resolution::文脈ごとの上書きは文脈の中だけ | 移植 | 既定のキーが解決する (Worktree c / Explorer c) |
| resolution::worktreeのgit操作のキーが解決する | 移植 | 既定のキーが解決する / 割り当ての無いキーは素通しする (u/v/P) |
| resolution::shift_gは大文字の割り当てに解決する | 移植 | チョードは正規化してから引く |
| resolution::shift_tabは逆回りの巡回 | 移植 | チョードは正規化してから引く |
| resolution::ctrl_tabはworktreeを切り替える | 移植 | 既定のキーが解決する + チョードは正規化してから引く |
| resolution::viewerでのctrl_fはファイル名検索 | 移植 | 既定のキーが解決する |
| resolution::viewerでのcはコメント追加 | 移植 | 既定のキーが解決する |
| resolution::f10はどの文脈でもメニューバーを開く | 移植 | 既定のキーが解決する (Global/Terminal/Explorer) + 逆引きは正規形の綴りを返す (keys_in_layer) |
| resolution::revidere層が解決する | 移植 | 既定のキーが解決する |
| resolution::explorerの表示と解析のキーが解決する | 移植 | 既定のキーが解決する |
| (新規) | 追加 | 全ての文脈の層をユーザが上書きできる — KeyContext::PANELS の全 10 層に [layers.<name>] を書いて UnknownLayer が出ず解決すること |

削除 0 本。

バグ修正: 旧 PANEL_CONTEXTS (9 個) に Revidere が無く、chain() は "revidere" 層を
引くのに warn_unknown_layers は知らない層として扱っていた。ユーザが
[keybinds.layers.revidere] を書くと UnknownLayer 警告が出ることを実コードで確認。
KeyContext::PANELS (10 個) に統一し、上記の新規テストで固定。

API 変更:
- KeyMap::new(user) (旧 #[cfg(test)]) → impl Default for KeyMap (既定のみ)。
  ユーザ設定つきの構築は with_warnings だけ。
- Action::fires_in_terminal は pub(crate) → pub (crate 境界を越えるため。
  tui のルータが横取りの真実の源を参照できる)。
- KeyContext::PANELS を pub const に (旧 pub(crate) PANEL_CONTEXTS)。
- keymap_suite::ActionName を再エクスポート (from_name / name を tui が
  keymap-suite に依存せず呼べる)。
- KeyContext::forwards_to_pty (pub(crate)) を新設。Terminal | Editor の判定が
  map.rs に 2 回あったものを 1 箇所に。
- 捨てた Action: UpdateAndRestart, TogglePanelOverlay (CheckForUpdate は旧にも
  無かった)。default_keybinds.toml から alt+/ と ÷ の 2 行を落とした。

残したコメント (なぜ):
- warning.rs: keymap_suite::Warning を公開面に出さない理由 (#[non_exhaustive] と
  シーケンスの概念)
- map.rs parse_user_keybinds: suite との境界を型でなく TOML テキストにする理由
  (toml crate のバージョン差)
- map.rs collect_warnings: `_ => {}` がシーケンス警告を捨てている理由
- action.rs: RevidereShowOverview/Sections が toggle でない理由、FoldPrefix の
  2 打鍵目をハンドラで読む理由、PublishReview がパレット限定な理由
- action.rs fires_in_terminal: PTY 横取りの唯一の真実の源であること
- tests.rs: shift ヘルパ (端末が届ける形)、macOS グリフ、alt+shift の SHIFT 保持、
  CapsLock が KeyInput に変換できないこと

検証: crate 全体は他モジュールの書き直し中 (git_engine / review_store /
theme) でコンパイルできないため、scratchpad/keymap-check に crossterm /
toml / keymap-suite だけを依存に持つ検証用 crate を置き、#[path] で
keymap/mod.rs を取り込んで cargo test (12 passed) / cargo clippy
--all-targets -- -D warnings (exit 0) / rustfmt を通した。
