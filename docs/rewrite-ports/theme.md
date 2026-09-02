# theme
旧テスト 12 本 → 新テスト 11 本 (移植 12 / 削除 0、統合で 7 本に畳み、新規 4 本)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 補色は色相を180度回して往復する | 移植 | 補色は彩度と明度を保って色相を180度回す |
| 補色はrgb以外を変えない | 移植 (統合) | rgbでない色はどの演算もそのまま返す (complement/lighten/darken/vivify/lerp/perceptual_distance を Reset/Red/Indexed で一括) |
| lightenの両端と中点 | 移植 (統合) | lightenとdarkenの両端と中点 (darken の両端も追加) |
| lightenはrgb以外を変えない | 移植 (統合) | rgbでない色はどの演算もそのまま返す |
| 高コントラストはdarkテーマの薄いグレーを明るくする | 移植 (統合) | 高コントラストは薄いグレーと本文を背景から遠ざける (極性を符号にして 1 本、全 11 テーマ) |
| 高コントラストはlightテーマの薄いグレーを濃くする | 移植 (統合) | 同上 |
| lerpの両端と中点 | 移植 (統合) | lerpは両端と中点を通り範囲外のtをクランプする |
| lerpは範囲外のtをクランプする | 移植 (統合) | 同上 |
| lightテーマはリスト末尾の3つちょうど | 移植 (強化) | 組み込みはダーク8つの後にライト3つが並ぶ (旧は filter 後の順序しか見ておらず「末尾」を固定していなかった。split_at(8) で位置まで固定) |
| all_namesはdarkがlightより先 | 移植 (統合) | 同上 |
| 知らない名前は既定へ落ちる | 移植 (強化) | 知らない名前は既定のcatppuccin_mochaへ落ちる (light=false だけでなく name と Default の一致まで) |
| all_namesの全部がfrom_nameを往復する | 移植 | 全組み込み名はfrom_nameを往復する |
| (新規) | 追加 | 知覚距離は同一で0で黒と白がおよそ765 (widget/row.rs の HOVER_MIN_DISTANCE=120 がこのスケールに依存) |
| (新規) | 追加 | vivifyは色相を保ち無彩色だけfallbackの色相を借りる (NEUTRAL_SATURATION の「なぜ」を固定) |
| (新規) | 追加 | コメントの書き手ごとの背景は全テーマで異なる (struct のコメントで「必ず違う色味に」と書いてあったが誰も検査していなかった) |

API 変更:
- 公開面は旧と同一: `Theme` (全 pub フィールド、name/light 込み)、`Default`、`from_name`、`all_names`、`darken`/`lighten`/`complement`/`vivify`/`perceptual_distance`/`lerp`/`high_contrast`。
- `Theme::lerp` は残した。起動演出 (entrance/anim) 以外に、行 hover のフェードアウト (widget/row.rs)、フォーカス枠のグライド (app/focus.rs)、reflow 突入時の枠色遷移 (terminal/render/claude.rs) が使っている。
- registry.rs を廃止し、名前→コンストラクタの表 `BUILTIN` 1 つに畳んだ。`from_name` の match と `all_names` の配列という同じ一覧の二重管理をやめ、`all_names` は表から const で導く。
- `name` の `#[allow(dead_code)]` を外した (lib crate では pub フィールドは dead にならない)。
- 11 テーマのパレット値は 1 つも変えていない (Rgb 列を旧ファイルと diff して一致を確認)。フィールド順も同じ。
- OSC11 の輝度からの自動選択は theme ではなく term_caps (`auto_theme_for_background`) が持っており、theme 側は "catppuccin-latte" という名前を返される側。term_caps の移植時にそのまま。

残したコメント (なぜ):
- `muted`: 文字色に使わない。7 テーマで背景に近く solarized-dark では背景そのもの。薄くするなら hint (メモリの実測)。
- `comment_preview_bg`: comment_user_bg と必ず違う色味に。署名を読まずに書き手を判別できるのはこの差だけ。
- `code_bg` / `code_fg`: 基調色より一段暗く / 見出しと別系統に。
- `complement`: RGB 反転だと明るさまで反転して濁る。
- `NEUTRAL_SATURATION`: 彩度が低いと rgb_to_hsl の色相が事実上任意で、彩度を上げると色相を作り出してしまう。
- `vivify`: lighten/darken は極値付近で余地が尽きるが、こちらは常に動ける。
- `perceptual_distance`: スケール 0〜約765 (呼び出し側の下限値がこれに依存)。
- `high_contrast` の押し出し量: 薄いグレーは背景に最も近いので最も強く、アクセントは色が飛ばない程度に軽く。
- パレット内の公式トークン名 (`// Mauve`, `// base1` など) は値の出典なので残し、色を言葉で言い換えただけの注記 (`// ピンク`) と hex 一覧の docstring は消した。nord の reply_text が info と別色である理由は残した。
- hover を前景で表現する理由 (背景方式は 7 テーマで selected_bg_inactive と区別不能) は theme の性質ではなく行スタイルの決定なので、widget/row.rs 側 (tui 移植時) に残す。

検証:
- 他エージェントの git_engine/review_store/repo_path が書き込み中で crate 全体がコンパイルできないため、theme/ だけを scratchpad の単独 crate (ratatui 0.29 のみ) にコピーして `cargo test` 11/11 pass、`cargo clippy --all-targets -- -D warnings` clean、`rustfmt --check` clean を確認。
- 依存は ratatui のみ。追加なし。
