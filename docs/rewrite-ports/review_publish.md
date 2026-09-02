# review_publish
旧テスト 9 本 → 新テスト 4 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| diff上の単一行のコメントは残す | 移植 | diffのハンクに収まるコメントだけを残す (テーブル) |
| 両端がハンクに入る範囲は残す | 移植 | 同上 |
| diffの外の行は落とす | 移植 | 同上 |
| diffに無いファイルは落とす | 移植 | 同上 |
| 標準のprのurlからownerとrepoを取る | 移植 | prのurlからownerとrepoを取る (テーブル) |
| github以外のurlは拒む | 移植 | 同上 |
| 単一行のコメントはstart_lineを付けない | 移植 | 範囲のコメントだけがstart_lineを持つ |
| 範囲のコメントはstart_lineとend_lineを付ける | 移植 | 同上 |
| コメントが無ければghを呼ばずに成功する | 移植 | 同名 |

URL のテーブルには owner/repo が空のケースを 2 件足した (旧コードにその判定が
あるのにテストが無かった)。

API 変更:
- PublishConfirm を削除。フィールドは PublishRequest + skipped と同じで、
  y/n オーバーレイという UI の都合でしか存在しなかった。呼び出し側は
  filter_publishable の (comments, skipped) をそのまま持てばよい。
- 他 (PublishComment / PublishRequest / PublishOutcome / filter_publishable /
  owner_repo_from_pr_url / publish) は旧のまま。
- テストの fixture は DiffState::new(base) の新シグネチャ (DiffViewMode 引数なし) に追従。

残したコメント (なぜ):
- 差分外のコメントが 1 件でも混ざると一括投稿が丸ごと 422 になる
- commit_id を明示する理由 (API の既定は投稿時点の HEAD で、同時の push と競合)
- 422 判定が stderr の部分一致な理由 (gh のエラー整形をパースせずに済む中で最も壊れにくい)
- ボディを stdin へ流す理由 (コメント本文は長く複数行)
- 一括が line / side を受け付けるか確かめきれず、コメント単位へフォールバックする
