# icons
旧テスト 5 本 → 新テスト 5 本 (移植 5 / 削除 0 / 新設 1)

fixture は「全グリフの一覧」1 本 (`every_glyph`: サンプルファイル名 44 + dir 2 + 矢印 2 + UI 定数 21) に
統合し、旧 3 系統 (ファイル / UI / 矢印) に分かれていた幅・私用領域の検査を同じ一覧に当てる。

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| ファイルのアイコンは1カラム幅 | 移植 | 全グリフは1カラム幅 + 情報を持つアイコンはフォールバック側でも描く (旧は空を許さなかったので、その事実は後者が固定) |
| uiのグリフは1カラム幅 | 移植 | 全グリフは1カラム幅 (UI 定数はフォールバック空を許す) |
| 展開の矢印は1カラム幅 | 移植 | 全グリフは1カラム幅 + 情報を持つアイコンはフォールバック側でも描く |
| フォールバックは私用領域を避ける | 移植 | 同名。対象は every_glyph |
| nerdのグリフはbmpの私用領域に収まる | 移植 | 同名。罫線 2 種の除外も同じ |
| (新設) | 新設 | ファイル名の一致は拡張子より優先し大文字小文字を区別しない — 書き直しで `file_icon` を名前→拡張子の 2 関数に割ったので、その優先順を固定 |

API 変更:
- 公開 API (`IconSet`, `Glyph::{get,labeled}`, `IconRole::color(&Theme)`, `FileIcon::{glyph,role}`,
  `dir_icon`, `file_icon`, `expand_arrow`, 定数 21 個) は旧と同形。利用側 (src/ 61 箇所) はパス差し替えで済む。
- 内部: フォールバック字形は種別 (IconRole) と 1:1 だったので `IconRole::unicode_glyph` に寄せ、
  `FileIcon` は `{ nerd, role }` の 2 フィールドに。旧の CODE/MARKUP/... 6 定数と `nerd()` ヘルパは消えた。
- `FileIcon` に PartialEq/Eq を追加 (テストで比較するため)。
- 捨てる機能 (自己更新バッジ / 起動演出 / パネル番号バッジ) のグリフは旧にも無かった。21 定数すべて
  src/ に利用側があるので落としたものはない。

足りない依存: なし (ratatui / serde / unicode-width は既にある)。

検証: core は並走中の theme (placeholder) と text_input (tests.rs 未着) でまだビルドが通らないため、
scratchpad/core-iso に icons + 6 フィールドだけの仮 Theme を置いた単体 crate で
`cargo test icons::` 5 本 pass / `cargo clippy --all-targets -D warnings` / `cargo fmt --check` を確認。
本体は theme の Theme が `fg / success / warning / info / hint / error` を Color で持てばそのまま通る
(port-theme に確認依頼済み)。

残したコメント (なぜ):
- モジュール doc: Plane 15 以降を使わない理由 (端末の幅扱い) / 絵文字を使わない理由 (固定色・幅 2 の割れ)
- IconSet: Nerd Font の有無は端末に問い合わせられないので term_caps が同梱シンボルで決める
- Glyph: 描画時に文字セットを決める理由 (ツリーのエントリがアイコンを抱えて生き残る)
- Glyph::nerd_only: 空文字を返す契約と、呼び出し側がアイコンごと省ける理由
- IconRole: 種別ごとの色を新設せず意味色へ寄せる理由 (テーマ 11 個)
- IconRole::color: accent を使わない理由 / Code が本文色の理由
- expand_arrow: 塗り三角を使わない理由 (Emoji プロパティで幅 2)
- ADD_COMMENT: 塗り円を選んだ理由
