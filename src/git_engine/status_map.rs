//! パス -> git status のルックアップ。Explorer のリフレッシュごとに1回だけ
//! 計算し、ファイルツリー(untracked/ignored の薄暗い表示)と Changed files
//! 一覧(ステージ状態の色分け)の両方で共有する。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use git2::{Repository, Status, StatusOptions, StatusShow};

/// Explorer のファイルツリーの薄暗い表示に使う、大まかな git 分類。
/// staged/unstaged は区別しない — そのより細かい区別は Changed files
/// 一覧が使う [GitStatusMap::status] 側にある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeGitState {
    Tracked,
    Untracked,
    Ignored,
}

/// git status の一発スナップショット。パス(libgit2 が報告する、worktree
/// ルートからの相対パス)でインデックスされる。フレームごとではなく
/// Explorer のリフレッシュごと(ファイルウォッチャーの発火 / 3秒ごとの
/// worktree ポーリング)に1回だけ構築する。
#[derive(Debug, Clone, Default)]
pub struct GitStatusMap {
    by_path: HashMap<String, Status>,
}

impl GitStatusMap {
    /// 指定した worktree の新しいスナップショットを読み込む。
    pub fn load(worktree_path: &Path) -> Result<Self> {
        let repo = Repository::discover(worktree_path)
            .with_context(|| format!("failed to discover repo from {}", worktree_path.display()))?;

        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(true);
        // 意図的に recurse_ignored_dirs(true) にしていない。これを有効に
        // すると、libgit2 は ignored ディレクトリを1つの折りたたまれた
        // エントリとして報告する代わりに、中の全ファイルを走査してしまう。
        // 実測では、target/ が21GBある親リポジトリで、有効時は2,771ms /
        // 122,407エントリ、無効時は約3.1msだった。これは3秒ごとの
        // worktree ポーリングタイマーから UI スレッドで同期的に実行される
        // ので、有効のままだとほぼ毎回のポーリングで UI が止まってしまう。
        // ビルド成果物が大量にあるリポジトリで再計測せずにこれを有効化
        // しないこと。
        let statuses = repo
            .statuses(Some(&mut opts))
            .context("failed to compute git status")?;

        let workdir = repo.workdir();
        let mut by_path = HashMap::with_capacity(statuses.len());
        for entry in statuses.iter() {
            let Some(path) = entry.path() else {
                continue;
            };
            let status = Self::drop_deletion_still_on_disk(entry.status(), workdir, path);
            if status.is_empty() {
                continue;
            }
            by_path.insert(path.to_string(), status);
        }
        Ok(Self { by_path })
    }

    /// 大文字小文字を区別しない FS ではケース違いの2エントリに実ファイルが1つしか
    /// 無く、libgit2 は余った方を削除として報告する(git 本体は clean)。
    fn drop_deletion_still_on_disk(status: Status, workdir: Option<&Path>, path: &str) -> Status {
        let still_there =
            status.is_wt_deleted() && workdir.is_some_and(|workdir| workdir.join(path).is_file());
        if still_there {
            status.difference(Status::WT_DELETED)
        } else {
            status
        }
    }

    /// path(worktree ルートからの相対パス)に対する git2 の生の status
    /// ビット。git status が何か報告していればそれを返す。None は、
    /// tracked かつ変更なしのファイル(git status はそれらを完全に省略
    /// する)と、折りたたまれた ignored ディレクトリの配下にあるパスの
    /// 両方をカバーする — このアクセサは親のプレフィックスを遡らない。
    /// それが必要な場合は [classify](Self::classify) を使うこと。
    pub fn status(&self, path: &str) -> Option<Status> {
        self.by_path.get(path).copied()
    }

    /// ファイルツリーの薄暗い表示のため、パスを tracked/untracked/ignored
    /// に分類する。path はファイルでもディレクトリでもよい(どちらでも
    /// 末尾スラッシュなし — tree entry の path フィールドをそのまま渡す)。
    ///
    /// status() と違い、これは親ディレクトリのプレフィックスも遡る:
    /// libgit2 は ignored ディレクトリを、中のファイル1つずつではなく
    /// 末尾スラッシュ付きの折りたたまれた1エントリ(例えば "target/")
    /// として報告するので、target/debug/foo のようなネストしたパスには
    /// それ自身のエントリがなく、祖先から Ignored を継承する必要がある。
    /// untracked ディレクトリではこれは不要 — recurse_untracked_dirs(true)
    /// がすでにファイル単位に展開してくれる。
    pub fn classify(&self, path: &str) -> TreeGitState {
        if let Some(status) = self.by_path.get(path) {
            return Self::classify_status(*status);
        }
        // path 自身が折りたたまれた ignored ディレクトリの場合もある
        // (tree entry としてのパスには末尾スラッシュがないが、libgit2 の
        // キーにはある)。
        if let Some(status) = self.by_path.get(&format!("{path}/")) {
            return Self::classify_status(*status);
        }
        for ancestor in Self::ancestor_dirs(path) {
            if let Some(status) = self.by_path.get(&ancestor) {
                // ignored な祖先(折りたたまれたディレクトリエントリ)だけが
                // 下位へ伝播する。tracked な祖先はこの特定のパスについて
                // 何も教えてくれない。ディレクトリ自体は git で
                // tracked/untracked になることがないため — さらに上の祖先が
                // 折りたたまれた ignored エントリである場合に備えて探索を
                // 続ける。
                if status.is_ignored() {
                    return TreeGitState::Ignored;
                }
            }
        }
        // git がまだ見たことのないディレクトリはそれ自身のエントリを
        // 持たない: recurse_untracked_dirs(true) は ignored ディレクトリ
        // のように折りたたむのではなく、中のファイルへ展開する。この
        // チェックなしにここへ到達すると、新規ディレクトリが通常の
        // tracked 色で描画される一方で中身は薄暗く表示されてしまう —
        // 親が「既知」で子が「新規」に見えるのは逆である。
        if self.has_descendants(path) && self.all_descendants_untracked(path) {
            return TreeGitState::Untracked;
        }
        TreeGitState::Tracked
    }

    /// path/ 配下に status エントリが1つでもあるか。
    fn has_descendants(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.by_path.keys().any(|k| k.starts_with(&prefix))
    }

    /// [has_descendants] と組み合わせて初めて意味を持つ。それ以外では空虚に true になる。
    fn all_descendants_untracked(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.by_path
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .all(|(_, s)| s.is_wt_new())
    }

    fn classify_status(status: Status) -> TreeGitState {
        if status.is_ignored() {
            TreeGitState::Ignored
        } else if status.is_wt_new() {
            TreeGitState::Untracked
        } else {
            TreeGitState::Tracked
        }
    }

    /// path の祖先ディレクトリパスを、末尾スラッシュ付き・近い順で返す。
    /// 例: "a/b/c.rs" -> ["a/b/", "a/"]。末尾スラッシュは libgit2 が
    /// 折りたたまれた ignored ディレクトリを報告する形式と一致する
    /// (実測で確認済み — status_map_classify_tests を参照)。
    fn ancestor_dirs(path: &str) -> impl Iterator<Item = String> + '_ {
        let mut current = path;
        std::iter::from_fn(move || {
            let slash = current.rfind('/')?;
            current = &current[..slash];
            Some(format!("{current}/"))
        })
    }
}

#[cfg(test)]
mod tests;
