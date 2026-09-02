# git_engine
旧テスト 22 本 → 新テスト 28 本 (移植 22 / 削除 0 / 新規 13。移植先は 15 本に統合)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| このリポジトリを開ける | 移植 | どこから開いてもmainのパスとconductor_dirはmain側を指す (サブディレクトリ行) |
| worktree一覧はmainを含む | 移植 | worktree一覧はmainを先頭にlinkedを続ける |
| コミットの無いリポジトリでも一覧が取れる | 移植 | 同名 |
| リンクされたworktreeからmainのパスを引く | 移植 | どこから開いても… (linked 行。conductor_dir の不変条件もここで固定) |
| mainリポジトリからmainのパスを引く | 移植 | どこから開いても… (main 行) |
| リモートurlはどの綴りでも同じhttpsに正規化する | 移植 | pr_urlはリモートの綴りに依らずgithubとgitlabで形が分かれる (公開 API 経由。gitlab 行と非対応 URL 行を追加) |
| grab状態はセッション無しで往復する | 移植 | grab状態は往復しlinked_worktreeからも同じファイルを見る |
| grab状態はセッション付きで往復する | 移植 | 同上 |
| 旧形式のgrab状態も読める | 移植 | 同上 (zsh の 3 行形式の行) |
| fetch_refspecはローカルブランチを作る | 移植 | 同名 |
| 知らないrefへのfetch_refspecは失敗を返す | 移植 | 同名 |
| 既存ブランチのworktree作成はそのブランチを出す | 移植 | 同名 (置き場所を tempdir 内に。旧は /tmp 直下に <name>-worktrees を作っていた) |
| 既にあるディレクトリへのworktree作成は拒む | 移植 | 同名 |
| 無いローカルブランチは作られる | 移植 | base_refは無ければ作られる |
| 既にあるローカルブランチは早送りされる | 移植 | base_refはoriginが先に進んでいれば早送りされる (origin の tip と一致するまで assert) |
| 分岐したブランチには触らない | 移植 | base_refは分岐していれば触らない |
| 未追跡のディレクトリは中身と同じく未追跡になる | 移植 | 分類は各フィクスチャが作られた状態を答える (newdir 4 行) |
| 追跡中のディレクトリは追跡中のまま | 移植 | 同上 (untouched.txt / nonexistent-dir 行) |
| 接頭辞を共有する兄弟は無視されない | 移植 | 同上 (build2/y.txt 行) |
| 分類は各フィクスチャが作られた状態を答える | 移植 | 同名 |
| 大小が衝突するエントリは削除扱いにしない | 移植 | 同名 (macOS のみ。cfg の理由もそのまま) |
| 本当の削除はちゃんと報告する | 移植 | 同名 |

新規 (旧に無かった事実の固定):
変更件数はファイルを1回だけ数えstagedだけ重複して数える (WorktreeInfo.staged の「なぜ」を固定) /
ブランチprefixは先頭の1つだけ落とす / base_refの解決はorigin_ローカル_headの順 (+ list_remote_branches) /
is_branch_merged_intoは同一か祖先なら真 / merge_into_mainはfast_forwardだけ行う /
cherry_pickは成功なら新コミットを作りコンフリクトならheadに戻す / list_branch_commitsは新しい順にlimit件 /
経過時間は最大の単位で丸める / 親ブランチは作成元を答え派生はその逆 /
最近触ったファイルはdirtyを先にコミット分を後に重複なく返す / remove_worktreeはエントリとディレクトリを消す /
消えたworktreeはstaleとして見つかりpruneできる / delete_branchはチェックアウトされていないブランチを消す

ビルダー: `TestRepo` (tempdir の main/ にリポジトリ、linked worktree は隣) と `Tree` (main にも
linked にも同じ file/add/commit/branch/checkout)、`Origin` (bare の origin)。`TestRepo` は `Tree` に Deref。

API 変更:
- `load_grab_state` は 4 要素タプルでなく `GrabState { branch, source_worktree, stash_branch, claude_session_id }`。
  `save_grab_state(&GrabState)`。`git_engine::GrabState` として re-export。
- `git_common_dir` は commondir ファイルを自前で読まず `Repository::commondir()`。
- `remote_url_to_https_base` は private に (テストは pr_url_for_branch 経由)。`ssh://host/owner/repo` (git@ 無し) が
  旧は ssh:// ごと残っていたのを直した。
- `list_branch_commits` の revwalk を TOPOLOGICAL | TIME に。同一秒のコミットが TIME だけだと順不同だった (テストで発覚)。
- `GitStatusMap` / `TreeGitState` を `git_engine::` 直下にも re-export (status_map:: も残る)。
- 3 箇所に複製されていた `git worktree add` の spawn を `git_worktree_add` に 1 本化。
- `detect_parent_via_reflog` の未使用引数 `_branch_oid` を削除。

残したコメント (なぜ):
- WorktreeInfo.staged が added/modified/deleted と重複する理由 (git add で動く唯一の信号)
- recurse_ignored_dirs を付けない理由 (実測 2,771ms / 122,407 件 vs 3.1ms、UI スレッドで走る)
- fetch が git CLI な理由 (libgit2 の credential が Keychain / gh auth を扱えない)
- worktree add が git CLI かつ spawn+wait な理由 (libgit2 の worktree API の脆さ、post-checkout hook のパイプ)
- has_tracked_changes が `git diff --quiet HEAD` な理由 (zsh の wt と判定を揃える)
- is_branch_merged_into が唯一の防御線な理由 (libgit2 の Branch::delete は not fully merged を拒まない)
- main_worktree_path の components() 正規化 (libgit2 が末尾スラッシュを返す)
- ensure_base_ref_available の scratch ref (チェックアウト中ブランチへの fetch は拒否される)
- pull の checkout_tree を ref 更新より先にする理由
- status_map の大小衝突 (libgit2 が余った方を削除扱いにする) と ignored ディレクトリの折りたたみ (実測)

補足:
- リード指示の「git2 Patch の実挙動 (バイナリは Some、show_untracked_content、find_similar)」は
  src/diff_state/compute.rs にあり、git_engine には無い。diff_state の移植で扱うこと。
- `BranchDetails.pr_loading` は UI 状態だが、指示どおり型を保った。
- 足りない依存: なし。cargo test -p conductor-core git_engine:: 28 passed、clippy --all-targets -D warnings クリーン、fmt 済み。
