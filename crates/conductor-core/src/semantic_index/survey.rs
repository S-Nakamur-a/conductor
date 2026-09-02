//! ツリーを歩いて分かること — どこが索引ルートで、その内容の鍵は何か — と、その結果を
//! 使った索引の読み込み・全ルートの生成。
//!
//! どれもツリーを歩く (実測で列挙 149ms、鍵 1 本あたり最大 110ms、109 ルート全部の鍵で
//! 0.6 秒) ので、呼び出し側が背景で走らせる。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sheaf_core::{IndexSource, Store};

use super::history::{self, Trigger};
use super::main_conductor_dir;
use super::roots::{self, IndexRoot};

/// ツリーを歩いて分かること。索引ルートと、それぞれの内容の鍵。
pub struct Survey {
    /// 調べたツリー。取り込む側が、その間に移っていないか見るのに使う。
    pub tree: PathBuf,
    pub roots: Vec<(IndexRoot, String)>,
}

/// `tree_root` の索引ルートを列挙し、鍵を計算する。
///
/// 鍵を出すのは `reading` を含むルート、既に成果物が置いてあるルート、`wanted` に名指し
/// されたルートだけ。実在するリポジトリでは 109 本あり、全部の鍵を出すと 0.6 秒かかる。
pub fn survey(
    tree_root: &Path,
    conductor_dir: Option<&Path>,
    reading: Option<&Path>,
    wanted: &[IndexRoot],
) -> Survey {
    let found = roots::discover(tree_root);
    let owning = reading.and_then(|rel| roots::owning_index(found.iter(), rel));
    let roots = found
        .iter()
        .enumerate()
        .filter(|(i, at)| {
            Some(*i) == owning
                || wanted.contains(at)
                || conductor_dir.is_some_and(|dir| at.has_any_generation(dir))
        })
        .map(|(_, at)| (at.clone(), at.content_key(tree_root, &found)))
        .collect();
    Survey {
        tree: tree_root.to_path_buf(),
        roots,
    }
}

/// main worktree に置いてある索引を、`tree_root` のツリーに向けてロードする。1 本も投入
/// できなければ `None`。索引ルートは `tree_root` から引き直す — 索引の中の相対パスをツリーの
/// どこへ接ぎ木するかは、ツリーの側にあるルートで決まるため。
///
/// 索引ルートの調査も一緒に返す。どちらもツリーを歩くので、背景の 1 回で済ませる。
pub fn survey_and_load(
    repo_root: &Path,
    tree_root: &Path,
    reading: Option<&Path>,
    wanted: &[IndexRoot],
) -> (Survey, Option<Store>) {
    let conductor_dir = main_conductor_dir(repo_root);
    let survey = survey(tree_root, conductor_dir.as_deref(), reading, wanted);
    let store = conductor_dir.and_then(|dir| {
        let sources: Vec<IndexSource> = survey
            .roots
            .iter()
            .filter_map(|(at, key)| at.source(&dir, key))
            .collect();
        if sources.is_empty() {
            return None;
        }
        Store::load(&sources, tree_root).ok()
    });
    (survey, store)
}

/// 置いてある索引を読むだけ。検査の口。
///
/// 読んでいるファイルを渡さないので、索引がまだ無いルートの鍵は出ない。
#[cfg(test)]
pub(crate) fn load(repo_root: &Path, tree_root: &Path) -> Option<Store> {
    survey_and_load(repo_root, tree_root, None, &[]).1
}

/// 索引ルート 1 本ぶんの成果。
pub struct Built {
    pub index: PathBuf,
    pub documents: usize,
    /// 出自を言えない document の数。索引には載っているのに鮮度を検査できないもの。
    pub missing_provenance: usize,
}

/// [`build_index`] の顛末。1 本も作れなかったときだけ `Err` になるので、`failures` が空で
/// ないことは残りが作れたことと両立する。
pub struct BuildOutcome {
    pub built: Vec<Built>,
    pub failures: Vec<String>,
}

/// ツリーの索引ルートをすべて索引して置く。`conductor index` の実体。
///
/// 1 本が失敗しても残りは作る。道具は言語ごとに別なので、scip-go が入っていないことを理由に
/// Rust の索引まで諦めるのは筋が違う。
pub fn build_index(repo_root: &Path) -> anyhow::Result<BuildOutcome> {
    let dir = main_conductor_dir(repo_root)
        .ok_or_else(|| anyhow::anyhow!("{} が git リポジトリではない", repo_root.display()))?;
    let found = roots::discover(repo_root);
    if found.is_empty() {
        anyhow::bail!(
            "{} に索引の作り方が分からない (Cargo.toml / go.mod / tsconfig.json のどれも無い)",
            repo_root.display()
        );
    }

    let mut outcome = BuildOutcome {
        built: Vec::new(),
        failures: Vec::new(),
    };
    for at in &found {
        let key = at.content_key(repo_root, &found);
        let target = at.target(&dir, repo_root, &key);
        let index = target.index.clone();
        let at_start = Instant::now();
        let before = at.newest_provenance(&dir);
        let result = sheaf_core::generate_once(target, at.lang.producer());
        at.prune(&dir);
        history::append(
            &dir,
            &history::Entry {
                root: &at.subroot,
                lang: at.lang.tag(),
                trigger: Trigger::Cli,
                cause: None,
                waited: Duration::ZERO,
                took: at_start.elapsed(),
                outcome: (&result).into(),
                sources: match &result {
                    sheaf_core::Outcome::Ready { .. } => {
                        history::source_delta(before.as_ref(), at, &dir, &key)
                    }
                    _ => history::Sources::Unknown,
                },
                changed_during: 0,
            },
        );
        match result {
            sheaf_core::Outcome::Ready { store } => outcome.built.push(Built {
                index,
                documents: store.len(),
                missing_provenance: store.missing_provenance(),
            }),
            sheaf_core::Outcome::Failed(why) | sheaf_core::Outcome::Unavailable(why) => {
                outcome.failures.push(format!("{:?}: {why}", at.lang))
            }
            sheaf_core::Outcome::Busy => outcome
                .failures
                .push(format!("{:?}: ほかのプロセスが索引を作っている", at.lang)),
        }
    }

    if outcome.built.is_empty() {
        anyhow::bail!("{}", outcome.failures.join("\n"));
    }
    Ok(outcome)
}
