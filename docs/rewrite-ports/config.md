# config
旧テスト 41 本 → 新テスト 17 本 (移植 26 / 削除 7 / tui へ移した 8 — docs/rewrite-ports/viewer.md の syntax 節)

訂正 (フェーズ 4c): `[updates]` と `[ui] startup_animation` は捨てない。フェーズ 0 で
「削除」に振り分けたのは、自己更新と起動演出を捨てる前提だったため。どちらも残すと
決まったので、設定も読める側へ戻した。
下の表の該当行は取り消し線ではなく、この段落が上書きする。

新の置き場: crates/conductor-core/src/config/{mod,sections,snapshot,persist,tests}.rs

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| tests_config: 既定のconfigはtomlを往復する | 移植 | tomlを往復して一致する (既定 + 全フィールド非既定の 2 行、Config 全体を assert_eq) |
| tests_config: 空のtomlは既定値になる | 移植 | 空のtomlは既定値になる (Config 全体で比較するので diff/layout の個別 assert も含む) |
| tests_config: diff_viewのserde | 移植 | セクションごとに鍵を読む ([diff] 行) |
| tests_config: 削除したreview_promptの鍵は無視される | 移植 | 捨てた設定と知らない鍵は無視される (LEGACY_CONFIG fixture に [review] の 3 鍵を持ち越し) |
| tests_config: 削除したrichセクションは無視される | 移植 | 同上 ([rich] を fixture に持ち越し)。[ccusage] [viewer] word_wrap も同じ fixture で固定。**[updates] と [ui] startup_animation は fixture から外し、セクションごとに鍵を読む が読める側で固定する (4c)** |
| tests_config: チルダの展開 | 移植 | チルダの展開 (テーブル: ~/ は home に、絶対/相対はそのまま) |
| tests_config: ccusageの設定を読む | 削除 | [ccusage] は持ち込まない。「残っていても落ちない」側は 捨てた設定と知らない鍵は無視される が固定 |
| tests_config: updatesの設定を読む | **移植 (4c で復活)** | セクションごとに鍵を読む ([updates] 行)。既定値は 空のtomlは既定値になる、live/restart の所属は 全フィールドはliveかrestartのどちらか一方に属する (updates.* 2 行) |
| tests_config: keybindsを読む | 移植 | keybindsは生のテーブルのまま持つ |
| tests_config: 生成した既定のconfigは妥当なtoml | 移植 | 既定ファイルは読むと既定値になる (個別 assert でなく Config::default() と全体比較) |
| tests_config: high_contrastは既定offでtomlを往復する | 移植 | 3 本に分散: 既定は 空のtomlは既定値になる、読みは セクションごとに鍵を読む ([ui] 行)、live 判定は 全フィールドはliveかrestartのどちらか一方に属する (ui.high_contrast 行) |
| tests_config: uiセクションはtomlを往復する | 移植 | tomlを往復して一致する に畳んだ。全フィールド非既定の Config を往復して全体を比較するので、ui だけの往復はその部分集合 |
| tests_config: layoutセクションはtomlを往復する | 移植 | 同上。読みは セクションごとに鍵を読む ([layout] 行) |
| tests_config: 空のtomlでもlayoutは既定値になる | 移植 | 空のtomlは既定値になる (全体比較に含まれる) |
| tests_persist: high_contrastはuiセクションに挿入される | 移植 | セクション内の鍵のupsert (「前のセクションにある同名の鍵は触らない」行が既存行の後ろへの挿入を固定) + 無い設定ファイルには既定を生成してから書く |
| tests_persist: uiセクションが無ければ末尾に足す | 移植 | セクション内の鍵のupsert 「セクションが無ければ末尾に追記する」 |
| tests_persist: コメントアウトされた既定の上に実値を挿入する | 移植 | 同 「コメントアウトされた既定の上に挿す」 |
| tests_persist: 既にある値は置き換える | 移植 | 同 「既存の行を置き換える」 |
| tests_persist: layoutの3つの鍵を続けて書ける | 移植 | ある設定ファイルはコメントを保って書き換える (実ファイルに 4 鍵を続けて書き、隣の [ui] とコメントが残ることを確認) |
| tests_persist: 既にあるtheme行を置き換える | 移植 | セクション内の鍵のupsert 「既存の行を置き換える」「空白なしの代入も置き換える」 |
| tests_persist: コメントだけならuiヘッダの直後に挿入する | 移植 | 同 「後ろのセクションにある同名の鍵は触らない」 (ヘッダ直後・コメントの前に挿さることを完全一致で固定) |
| tests_persist: uiの後ろの他セクションを壊さない | 移植 | 同 「前のセクションにある同名の鍵は触らない」 (前後のセクションを完全一致で固定) |
| tests_persist: 末尾の改行は保たれる | 移植 | 同 「末尾に改行が無ければ足さない」「改行で終わらないファイルにも追記できる」 |
| tests_persist: uiヘッダ行の行末コメントを扱える | 移植 | 同 「行末コメント付きのヘッダも同じセクション」 |
| tests_persist: uiのサブセクションには当たらない | 移植 | 同 「サブセクションには当たらない」 |
| tests_persist: セクションヘッダの判定 | 移植 | セクションヘッダの判定 (テーブル) |
| tests_snapshot: 見た目のスナップショットはlayoutを含む | 移植 | 全フィールドはliveかrestartのどちらか一方に属する (layout.* 4 行が Live)。スナップショットのフィールドは非公開になったので直接は読まない |
| tests_snapshot: adopt_appearanceは往復して一致する | 削除 | 新設計では snapshot が adopt_appearance から導出されるので、adopt が写す集合と snapshot が追う集合は構成上同一。ずれようがない |
| tests_snapshot: 同じconfigのスナップショットは等しい | 削除 | PartialEq の反射律。依存 (derive) が保証している |
| tests_snapshot: liveフィールドの変更を1つずつ検出する | 移植 | 全フィールドはliveかrestartのどちらか一方に属する (Live 行 13 本) |
| tests_snapshot: liveだけの差ならhas_restart_changesはfalse | 移植 | liveフィールドを全部変えても再起動は要らない |
| tests_snapshot: restartフィールド1つでhas_restart_changesはtrue | 移植 | 全フィールドはliveかrestartのどちらか一方に属する (Restart 行 13 本) |
| tests_snapshot: 全フィールドがliveかrestartのどちらかに属する | 移植 | 同名 1 本。旧は「どちらか」だったが新は「どちらか一方 (両方はない)」まで固定。旧が抜かしていた ui.icons / keybinds / api.model / api.command / api.command_timeout_secs / terminal.inactive_scrollback / viewer.syntax_theme_file / layout.explorer_split_pct も表に載せた |
| tests_syntax_theme: 全テーマが同じ明暗のsyntectテーマに対応する | tui へ移植 | syntect / two-face は core に入れない。Theme::all_names との網羅チェックごと tui の syntax_theme に持ち越す |
| tests_syntax_theme: 組み込みテーマは全部が自前のsyntectテーマを持つ | tui へ移植 | 同上 |
| tests_syntax_theme: 主要言語で十分な割合のトークンに色が付く | tui へ移植 | 同上 |
| tests_syntax_theme: ui_themeはviewer_themeより優先される | 移植 + tui へ移植 | 固定している事実は theme_name の優先順位。core では theme_nameはui_themeを優先する で固定。syntect 経由の確認は tui 側に残す |
| tests_syntax_theme: ui_themeが無ければviewer_themeへ落ちる | 移植 + tui へ移植 | 同上 (None 行) |
| tests_syntax_theme: 構文テーマのidはテーマ名とファイルを追う | tui へ移植 | syntax_theme_id ごと tui へ |
| tests_syntax_theme: 知らないテーマ名でも落ちずに落ち着く | tui へ移植 | syntect の解決 |
| tests_syntax_theme: テーマファイルが無くても落ちずに落ち着く | tui へ移植 | syntect の解決 |

新規 (旧に無かった事実):
- 無ければ既定ファイルを書いて既定値を返す / 読み込み時にパスのチルダを展開する: load の実ファイル挙動 (旧は serde しか見ていなかった)
- 無い設定ファイルには既定を生成してから書く: persist 側の「無ければ既定に対して upsert」と .tmp が残らないこと
- 文字セットはserdeの綴りでtomlの文字列になる: persist_ui_icons が toml::Value 経由で "nerd" と書く前提

API 変更:
- 削除: `syntax_theme_id` / `syntect_theme_for` (tui へ)、`CcusageConfig` / `ReviewConfig`、`ViewerConfig::word_wrap`、`config::DiffView` の再エクスポート (`crate::diff_state::DiffView` を使う)
- **4c で復活: `UpdatesConfig` (`Config::updates`) と `UiConfig::startup_animation`。** 既定値は旧と同じ (`check_on_startup = true` / `check_interval_secs = 3600` / `startup_animation = true`)。`DEFAULT_CONFIG` にもコメント行を戻した。live/restart の所属は `updates.*` が Restart (起動時にしか読まない)、`ui.startup_animation` が Live (`adopt_appearance` が `ui` ごと写す)
- `generate_default_config() -> String` → `pub const DEFAULT_CONFIG: &str`
- `persist_layout_proportions(u16, u16, u16, u16)` → `persist_layout_proportions(&LayoutConfig)` (4 つの位置引数は順番を間違えても型で気づけなかった)
- 追加: `Config::load_from(&Path)` (`load()` はこれに委譲。テストの入口)
- `Config` と全セクションに `PartialEq`
- `AppearanceSnapshot` は `Config` の newtype (restart フィールドを既定値に潰した Config)。フィールドは非公開。tui 側の用途は `!=` だけなので影響なし
- `has_restart_changes` の判定を `adopt_appearance` から導出 (old に new の外観を写して new と比べる)。フィールドの列挙を 3 箇所 (snapshot / adopt / restart) から 1 箇所に減らした
- 挙動差: `ui.icons` が live になった (旧は snapshot にも restart にも無く、ファイルを直しても無視されていた)。起動時の自動判定は app.config に先に入れてから書くので、その書き込みは従来どおり no-op
- `persist_*` の 4 関数は共通の `persist_at(path, section, kvs)` に集約。`upsert_ui_theme` は削除 (テスト専用の薄いラップだった)
- `write_atomic` が親ディレクトリを作るようになった (旧は load だけが作っていて persist は作らなかった)
- バックアップの類は旧にも無い。安全策は一時ファイル + fsync + rename の原子的書き込みだけで、これは残した

足りない依存: なし (serde / toml / dirs / anyhow / log は既にある。テストは tempfile)

残したコメント (なぜ):
- mod.rs `theme_name`: ui.theme と viewer.theme を別々に読んで配色がずれた経緯 (3 行)
- snapshot.rs モジュール doc: live/restart の切り分けを adopt_appearance 1 本に集約している理由
- persist.rs `write_atomic`: std::fs::write がその場で切り詰めるので手編集のファイルが壊れる (2 行)
- persist.rs `persist_ui_icons`: テーマ (セッション限り) と違って書き戻す理由 = 判定材料が TERM_PROGRAM しかない (3 行)
- persist.rs `persist_layout_proportions`: 書き込みで watcher が起きるが no-op になる理由 (2 行)
- sections.rs `auto_resume_main`: main は寿命が長くセッションが積み重なる (2 行)
- sections.rs `ApiConfig`: provider 間にフォールバックが無いこと、{prompt} 無しなら stdin (コードからは読めない契約)

検証:
- 他エージェントの書きかけ (diff_state / symbol_index / claude_log の `mod tests;` 実体なし) で crate のテストビルドが通らないので、crate を scratchpad にコピーしてその 3 行だけ外して実行した
- `cargo test config::` 17 passed / `cargo clippy --all-targets -- -D warnings` config 由来の指摘なし / `rustfmt --check` クリーン
- `cargo check -p conductor-core` (非テスト) は本体でも通る
