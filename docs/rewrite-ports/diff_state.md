# diff_state
旧テスト 32 本 → 新テスト 21 本 (macOS 限定 1 本を含む)

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| 置き換えられた行は行内の差分を持つ | 削除 | similar crate の iter_inline_changes を直接見ているだけで自前コードを通らない。自前の attach_inline_segments は新設の 単語diffは並び順で対応する削除と追加の組にだけ付く が固定する |
| 大小だけのリネームは変更として出さない | 移植 | 大小だけのリネームは内容が同じときだけ隠す (テーブル 1 行目) |
| 大小のリネームでも編集があれば出す | 移植 | 大小だけのリネームは内容が同じときだけ隠す (テーブル 2 行目) |
| 畳み済みのディレクトリを畳んでも落ちない | 移植 | 折りたたみはディレクトリ行だけを変えファイル行では何もしない (no-op で表示リストも変わらないことまで固定) |
| 展開済みのディレクトリを開いても落ちない | 移植 | 同上 |
| パスから表示行の位置を引ける | 移植 | パスから表示行の位置を引ける (display_list を手で組まず rebuild_display_list を通す) |
| 変更パスの解決は別の綴りも受ける | 移植 | 変更パスの解決 (テーブル 15 行のうち 8 行) |
| diffの外のファイルは解決しない | 移植 | 変更パスの解決 (3 行) |
| あいまいな末尾一致は解決しない | 移植 | 変更パスの解決 (2 行) |
| 完全一致を接頭辞落としより優先する | 移植 | 変更パスの解決 (2 行) |
| revealは畳まれた親を開く | 移植 | revealは畳まれた親を開く (diff に無いパスは None も追加) |
| ベースrefはリモート追跡refを解決する | 移植 | ベースrefはローカルブランチが無くてもリモート追跡refとタグとoidで解決する (テーブル 6 行のうち 1 行) |
| リンクされたworktreeからもベースrefを解決する | 移植 | リンクされたworktreeからもリモート追跡refを解決する (git CLI 起動をやめ TestRepo::linked_worktree に) |
| ベースrefは軽量タグと注釈タグとoidを解決する | 移植 | 同テーブル 4 行 (軽量タグ / 注釈タグ / 完全 OID / 短縮 OID) |
| ベースrefはorigin付きへ落ちる | 移植 | 同テーブル 1 行 |
| 解決できないベースrefはエラーを返す | 移植 | 解決できないベースは利用者が書いた綴りで理由を返す (テーブル 5 行のうち 1 行) |
| エラー文でorigin接頭辞を二重にしない | 移植 | 同テーブル 1 行 (罠の ref origin/origin/weird も持ち越し) |
| gitと同じくタグをブランチより優先する | 移植 | gitと同じくタグをブランチより優先する (計画書 4.3 の 10) |
| 解決できないベースはheadへ落ちる | 削除 | ベースが解決できなくても手元の変更は一覧に残る が compute_changed_files を経由する load_diff で同じ事実 (files == [c.txt]) を固定している |
| ベースが解決できなくてもファイル一覧は残る | 移植 | ベースが解決できなくても手元の変更は一覧に残る (テーブル 2 行のうち 1 行。計画書 4.3 の 10) |
| headがmerge_baseと同じなら未コミット分だけ出る | 移植 | headがmerge_baseと同じなら未コミット分だけ出る (テーブル 1 行目) |
| 無音で0件になる不具合を再現する | 移植 | 無音で0件になる不具合を再現する |
| merge_baseが無関係でもファイル一覧は残る | 移植 | エラー文は 解決できないベースは利用者が書いた綴りで理由を返す の 1 行、一覧が残る方は ベースが解決できなくても手元の変更は一覧に残る の 1 行 |
| コミット前のheadでも落ちずにエラーを返す | 移植 | コミット前のheadでも落ちずにエラーを返す |
| ベースがheadなら作業ツリーの変更だけ出る | 移植 | headがmerge_baseと同じなら未コミット分だけ出る (テーブル 2 行目。未追跡 1 件を置いて「それだけ出る」を固定) |
| blobを指すベースrefはエラーを返す | 移植 | 解決できないベースは利用者が書いた綴りで理由を返す の 1 行 |
| 空文字のベースrefはエラーを返す | 移植 | 同テーブル 1 行 |
| コミット後に編集したファイルも1エントリのまま | 移植 | コミット後に編集したファイルも1エントリのまま |
| 読めないファイルを全行削除にでっち上げない | 移植 | 読めないファイルを全行削除にでっち上げない |
| 本当に消えたファイルは全行削除として出す | 移植 | 本当に消えたファイルは全行削除として出す |
| バイナリは行数無しで一覧に残る | 移植 | バイナリは行数無しで一覧に残る (計画書 4.3 の 9。doc を「Some でハンク 0」に直した) |
| 大小が衝突するエントリを削除として出さない (macOS) | 移植 | 大小が衝突するエントリを削除として出さない (fs_ignores_case は test_support に共通化) |
| (新規) | 追加 | ハンクには直上の関数ヘッダーが付く。旧にテストが無く、書き直した箇所なので固定した |
| (新規) | 追加 | 表示リストはディレクトリを先にファイルを後に深さ付きで並べる。ツリー構築を書き直したので出力の形を固定した |

API 変更:
- DiffViewMode を廃止。config と共有する DiffView (mod.rs) を直接使う。1:1 の写しだった
- DiffState から scroll と view_mode を削除。diff_state 内で読まれておらず explorer のカーソルと viewer の表示設定。DiffState::new(base_branch) の 1 引数に
- DiffListEntry::Summary {} を unit variant Summary に
- DiffState::expand_tabs を非公開に (src/ に利用者なし。viewer は自前の展開を持つ)
- モデル型に PartialEq / Eq を追加
- 関数ヘッダーの正規表現をファイルごとの Regex::new から LazyLock の静的テーブルに
- 大小のみリネームの照合で「実パスが異なるか」の再確認を削除 (同じパスが Deleted と Added の両方に出ることはない)
- リネーム検出 (find_similar) は旧 compute.rs にも無かったので入れていない。メモリの記述は git_engine 側の知見

残したコメント (なぜ):
- show_untracked_content が無いと未追跡の追加行数が 0 になる (compute.rs)
- resolve_base_commit: revparse でタグ > ブランチ、origin/ 補完、エラーには利用者の綴りを残す
- file_diff: 内容不変のデルタは Patch が None (大文字小文字を区別しない FS の stat 不一致)、バイナリは Some でハンク 0
- '=' '>' '<' は改行なし注記行
- attach_inline_segments: libgit2 は行単位までなので並び順で組にする
- deletions_still_on_disk: ケース違い 2 エントリに実ファイル 1 つで libgit2 が削除と報告 (git 本体は clean)、DiffOptions では直らない
- case_only_rename_indices: 大文字小文字を区別しない FS では同じファイル
- resolve_changed_path: 完全一致を先にする理由と、曖昧な末尾一致を採らない理由

test_support:
- git_engine/tests.rs の Tree / TestRepo を crate::test_support に昇格。Origin と with_remote 系は git_engine 固有なので同ファイルに残した (inherent impl の追加)
- fs_ignores_case も共通化

検証:
- 他エージェントの書きかけ (claude_log/tests.rs 未作成、symbol_index の借用エラーと clippy) で crate 全体のテストビルドが落ちる。scratchpad/core-iso にコピーしてその 2 モジュールの mod tests を外して実行
- cargo test diff_state:: 21/21、cargo test git_engine:: 28/28
- cargo clippy --tests -D warnings は自分の 6 ファイルに指摘なし (残る 5 件は symbol_index)
- cargo fmt -p は claude_log の欠損で動かないので rustfmt --edition 2024 を直接かけた
- 依存の追加なし
