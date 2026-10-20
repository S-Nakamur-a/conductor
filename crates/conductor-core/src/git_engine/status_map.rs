//! パスから git status を引く一発スナップショット。Explorer のリフレッシュごとに
//! 1 回だけ読み、ファイルツリーの薄暗い表示と Changed files の色分けで共有する。

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use git2::{Index, Repository, Status};

/// ファイルツリーの薄暗い表示のための大まかな分類。staged / unstaged の区別は
/// [GitStatusMap::status] 側。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeGitState {
    Tracked,
    Untracked,
    Ignored,
}

/// 作業ツリー全体を歩かずに ignored かどうかだけを答える。
///
/// 一覧に ignored を混ぜると作業ツリー全体の走査になる (17 万ファイルで
/// `git status --ignored` が 4.0 秒) ので、ここで 1 件ずつ答える (0.3ms)。
pub struct IgnoreOracle {
    repo: Option<Repository>,
    index: Option<Index>,
}

impl IgnoreOracle {
    pub fn open(worktree_path: &Path) -> Self {
        let repo = Repository::discover(worktree_path).ok();
        let index = repo.as_ref().and_then(|repo| repo.index().ok());
        Self { repo, index }
    }

    /// 追跡済みのファイルに gitignore は効かない。ルールだけで答えると `git add -f`
    /// したファイルを ignored と誤判定する (`status_should_ignore` が true を返す)。
    /// index が読めなければルールだけで答えるため、その誤判定が残る。
    pub fn classify(&self, path: &str) -> TreeGitState {
        if self.is_tracked(path) {
            return TreeGitState::Tracked;
        }
        match &self.repo {
            Some(repo) if repo.status_should_ignore(Path::new(path)).unwrap_or(false) => {
                TreeGitState::Ignored
            }
            _ => TreeGitState::Tracked,
        }
    }

    /// コンフリクト中のエントリは stage 1/2/3 しか持たないので、stage 0 だけ見ると
    /// 解決中に一番見たいファイルが ignored として暗く出る。
    fn is_tracked(&self, path: &str) -> bool {
        self.index.as_ref().is_some_and(|index| {
            let path = Path::new(path);
            index.get_path(path, 0).is_some() || index.get_path(path, 2).is_some()
        })
    }
}

/// `git2::Index` は Debug を持たないが、これを抱える FileTree は derive している。
impl std::fmt::Debug for IgnoreOracle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IgnoreOracle")
            .field("opened", &self.repo.is_some())
            .finish()
    }
}

/// worktree ルートからの相対パスをキーにした status。ignored は入っていない
/// ([IgnoreOracle] が 1 件ずつ答える)。
#[derive(Debug, Default)]
pub struct GitStatusMap {
    by_path: HashMap<String, Status>,
}

impl GitStatusMap {
    pub fn load(worktree_path: &Path) -> Result<Self> {
        Ok(Self {
            by_path: porcelain(worktree_path)?,
        })
    }

    /// git status が報告した生のビット。変更なしの tracked ファイルは None。
    /// 畳まれた未追跡ディレクトリの配下は Changed files に 1 行ずつ出るので、
    /// 未追跡だけは祖先から継承する (None だとそこだけ clean と同じ色になる)。
    pub fn status(&self, path: &str) -> Option<Status> {
        if let Some(status) = self.by_path.get(path) {
            return Some(*status);
        }
        ancestor_dirs(path).find_map(|ancestor| {
            self.by_path
                .get(&ancestor)
                .filter(|status| status.is_wt_new())
                .copied()
        })
    }

    /// path (ファイルでもディレクトリでも、末尾スラッシュ無し) を分類する。
    ///
    /// git は中身が全部未追跡のディレクトリを `newdir/` の 1 エントリに折りたたむので、
    /// 配下のパスは祖先から Untracked を継承する。追跡済みのファイルを含むディレクトリは
    /// 折りたたまれず未追跡ファイルだけが個別に出るため、ディレクトリ自身は Tracked。
    pub fn classify(&self, path: &str) -> TreeGitState {
        if let Some(status) = self
            .by_path
            .get(path)
            .or_else(|| self.by_path.get(&format!("{path}/")))
        {
            return classify_status(*status);
        }
        if ancestor_dirs(path).any(|ancestor| {
            self.by_path
                .get(&ancestor)
                .is_some_and(|status| status.is_wt_new())
        }) {
            return TreeGitState::Untracked;
        }
        TreeGitState::Tracked
    }
}

/// `git status` を呼ぶ。libgit2 の `statuses` は fsmonitor も untrackedCache も見ず、
/// git 本体の 10 倍以上かかる。
///
/// `-uall` は付けない (4.2 秒。未追跡が 1 件も無くても全走査する)。畳まれたディレクトリは
/// [GitStatusMap::classify] が継承で開く。
fn porcelain(worktree_path: &Path) -> Result<HashMap<String, Status>> {
    let out = Command::new("git")
        // 付けないと status が index を書き戻し、利用者や Claude Code が同じ worktree で
        // 打った git add / commit を index.lock で弾く。ウォッチャー毎に走るので当たる。
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(worktree_path)
        // --no-renames が無いと git はリネームを 1 レコードに畳み、元のパスが引けなく
        // なる。Changed files の一覧はリネームを検出しないので元のパスの行を出し、
        // status が None だとそこが clean と同じ色になる。パーサもこの形に依存している。
        .args(["status", "--porcelain", "-unormal", "-z", "--no-renames"])
        // これらは -C に勝つので、プロセス環境に入っていると別リポジトリの status が返る。
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .with_context(|| format!("cannot run git status in {}", worktree_path.display()))?;
    if !out.status.success() {
        bail!(
            "git status failed in {}: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_porcelain(&out.stdout))
}

/// `XY<空白>PATH` を NUL で区切った並び。
///
/// 同じパスに 2 レコード来ることがある (`git rm --cached` で `D ` と `??`) ので、
/// 後勝ちにせずビットを畳む。
fn parse_porcelain(stdout: &[u8]) -> HashMap<String, Status> {
    let mut by_path: HashMap<String, Status> = HashMap::new();
    for record in stdout.split(|byte| *byte == 0) {
        if record.len() < 4 {
            continue;
        }
        let status = status_bits(record[0], record[1]);
        if !status.is_empty() {
            *by_path
                .entry(String::from_utf8_lossy(&record[3..]).into_owned())
                .or_insert(Status::empty()) |= status;
        }
    }
    by_path
}

/// `R` と `C` は来ない (`--no-renames`)。
fn status_bits(index: u8, worktree: u8) -> Status {
    // 未マージは UU 以外に AA (両方追加) と DD (両方削除) もあり、U を含まない。
    let unmerged = index == b'U'
        || worktree == b'U'
        || matches!((index, worktree), (b'A', b'A') | (b'D', b'D'));
    if unmerged {
        return Status::CONFLICTED;
    }
    let staged = match index {
        b'M' => Status::INDEX_MODIFIED,
        b'A' => Status::INDEX_NEW,
        b'D' => Status::INDEX_DELETED,
        b'T' => Status::INDEX_TYPECHANGE,
        _ => Status::empty(),
    };
    let unstaged = match worktree {
        b'?' => Status::WT_NEW,
        b'M' => Status::WT_MODIFIED,
        b'D' => Status::WT_DELETED,
        b'T' => Status::WT_TYPECHANGE,
        _ => Status::empty(),
    };
    staged | unstaged
}

fn classify_status(status: Status) -> TreeGitState {
    if status.is_wt_new() {
        TreeGitState::Untracked
    } else {
        TreeGitState::Tracked
    }
}

/// 末尾スラッシュ付き・近い順。"a/b/c.rs" -> ["a/b/", "a/"]。
fn ancestor_dirs(path: &str) -> impl Iterator<Item = String> + '_ {
    let mut current = path;
    std::iter::from_fn(move || {
        let slash = current.rfind('/')?;
        current = &current[..slash];
        Some(format!("{current}/"))
    })
}
