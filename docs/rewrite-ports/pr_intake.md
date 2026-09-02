# pr_intake
旧テスト 9 本 → 新テスト 8 本 (うち 1 本は #[ignore])

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 先頭がダッシュのrefは拒む | 移植 | 先頭がダッシュのrefは拒む |
| 素の番号を受け付ける | 移植 | pr番号とurlをパースする (テーブル) |
| githubのurlを受け付ける | 移植 | pr番号とurlをパースする (テーブル) |
| でたらめな入力は拒む | 移植 | pr番号とurlをパースする (テーブル) |
| ghとgitのstderrを手の打てるエラーに直す | 移植 | 同名 (テーブルのまま) |
| エラー文は次の手が分かる形になっている | 移植 | 同名 |
| 既にあるworktreeにはghも通信も無しで入り直す | 移植 | 既にあるworktreeには通信無しで入り直す (fixture 共有) |
| 壊れたディレクトリでは手の打てるエラーで落ちる | 移植 | 同名 (fixture 共有) |
| 実在するprに対する取り込み (#[ignore]) | 移植 | 同名 |
| — | 新設 | レビューdbのauthorにはheadリポジトリの所有者が入る |

fixture: repo_with_existing_pr_dir(git_marker) が「PR 42 の worktree だけがある一時
リポジトリ」を組む。canonicalize は macOS の /tmp -> /private/tmp のため持ち越し。

API 変更:
- PrMeta / GhOwner を private に (gh の JSON の形はこのモジュールの内側)。
- FetchedPr::review_meta(pr_number) -> review_store::PrReviewMeta を新設。
  save_pr_review_meta が 7 引数から &PrReviewMeta に変わったため、gh の
  headRepositoryOwner.login -> author の対応を tui ではなくここに置く。
- それ以外 (local_branch_name / parse_pr_input / PrIntakeError / PrIntakeOutcome /
  intake_pr) は旧のまま。

残したコメント (なぜ):
- pr-<N> がハイフンな理由 (自分の pr/ 名前空間との衝突)
- 先頭 - の ref を git に渡すとオプションとして読まれ得る
- .git はファイルでもディレクトリでも通す (worktree では gitdir へのポインタ)
- ベース ref の取得はベストエフォート (失敗しても差分は出せる)
- DB 書き込みを呼び出し側に残す理由 (ReviewStore はスレッドを跨がない)
- 再入場で fast-forward しない (黙ってブランチを進めない)
