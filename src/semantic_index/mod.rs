//! sheaf-core の [`Store`] を conductor に橋渡しする層。
//!
//! `.conductor/` は main worktree にしか無いので、`load` は `repo_root` から
//! `commondir()` を辿って解決する。照合先のツリー (`tree_root`) は選択中の
//! worktree で、これとは別物。
//!
//! 出自 (生成時点で実際にディスクにあった内容のハッシュ) を外から申告するのは、
//! SCIP 索引がソース本文を持たないため。コミットを出自にすると、作業ツリーを
//! 索引したときに未追跡ファイルが永久に鮮度の検査を通らなくなる。

mod bridge;
mod history;
pub(crate) mod roots;

pub use bridge::Bridge;
pub use sheaf_core::Regenerated;

use history::Trigger;
use roots::IndexRoot;
use sheaf_core::{IndexSource, Regenerator, Slot, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 終わった生成 1 世代。
///
/// `manual` を添えるのは、編集ごとの作り直しで status を埋めずに、手で頼んだ
/// ものだけ結果を出すため。
pub struct Finished {
    pub outcome: Regenerated,
    pub manual: bool,
}

/// いま読んでいるファイルに対して索引がどこまで答えられるか。
///
/// 「まだ無い」「対象外」「古い」を 1 つの `bool` に潰すと、作っている最中に
/// 古いと言うことになる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reading {
    /// 前回と同じファイル。何も起きていない。
    Unchanged,
    /// 索引が今の内容を説明している。
    Indexed,
    /// 今の内容の索引はあるのに、このファイルが載っていない。
    ///
    /// producer がそのファイルを索引しなかったか、生成中に動いて出自から落ちたか、
    /// producer を起動できないかのいずれか。内容が動いただけなら `Building`。
    Stale,
    /// このルートを索引しているところ。
    Building,
    /// 索引はあるが、まだ読み込めていない。答えは次の周に持ち越す。
    Loading,
    /// 索引の対象ではない。
    NotIndexed,
}

/// 索引ルート 1 本と、その作り直し係。
struct Root {
    at: IndexRoot,
    /// 調査した時点のツリーの内容から決まる鍵。成果物の名前に入る。
    ///
    /// `None` は「編集が入って、まだ引き直せていない」。鍵が古いまま生成すると、出来た
    /// 索引が読む側の探す名前と食い違うので、鍵の無いルートは生成を始めない。
    key: Option<String>,
    regenerator: Regenerator,
    /// 走っている 1 世代の顛末。記録に残すためだけに持つ。
    run: Run,
}

impl Root {
    fn is_working(&self) -> bool {
        self.regenerator.is_pending() || self.regenerator.is_running()
    }

    /// 既に待っている/走っている世代のきっかけは上書きしない (最初に頼んだものがその理由)。
    fn request(&mut self, trigger: Trigger, cause: Option<PathBuf>) {
        if !self.is_working() {
            self.run = Run::asked(trigger, cause);
        }
        match trigger {
            Trigger::Manual => self.regenerator.request_now(),
            _ => self.regenerator.request(),
        }
    }

    /// 前の生成から、この producer が読むファイルがどれだけ動いたか。
    fn source_delta(&self, dir: &Path, key: &str) -> history::Sources {
        source_delta(self.run.before.as_ref(), &self.at, dir, key)
    }

    fn record(&mut self, dir: &Path, key: &str, outcome: &Regenerated) {
        // 生成に至らなかったものは、比べる相手がそもそも入れ替わっていない。
        let sources = match outcome {
            Regenerated::Ready { .. } => self.source_delta(dir, key),
            _ => history::Sources::Unknown,
        };
        self.log_with(dir, sources, outcome.into());
        // 生成中に変更が来ていた場合とロックを取れなかった場合、sheaf は待機に
        // 戻してもう一度走らせる。その世代のきっかけを引き継がないと、記録が
        // 「きっかけ不明・待ち時間 0 秒」になる。
        self.run = if self.regenerator.is_pending() {
            match self.run.next_cause.take() {
                Some(cause) => Run::asked(Trigger::Change, Some(cause)),
                // ロックを取れずにやり直すだけなので、元のきっかけのまま。
                None => Run::asked(
                    self.run.trigger.unwrap_or(Trigger::Change),
                    self.run.cause.clone(),
                ),
            }
        } else {
            Run::default()
        };
    }

    fn log_with(&self, dir: &Path, sources: history::Sources, outcome: history::Outcome<'_>) {
        history::append(
            dir,
            &history::Entry {
                root: &self.at.subroot,
                lang: self.at.lang.tag(),
                trigger: self.run.trigger.unwrap_or(Trigger::Change),
                cause: self.run.cause.as_deref(),
                waited: self.run.waited(),
                took: self.run.took(),
                outcome,
                sources,
                changed_during: self.run.changed_during.len(),
            },
        );
    }
}

/// 生成 1 世代ぶんの計測。
#[derive(Default)]
struct Run {
    trigger: Option<Trigger>,
    /// きっかけになったファイル。ツリーのルートからの相対パス。
    cause: Option<PathBuf>,
    asked_at: Option<Instant>,
    started_at: Option<Instant>,
    /// この生成に意味があったかを言うために、producer が立つ直前の出自の表を取っておく。
    before: Option<HashMap<PathBuf, String>>,
    /// 走っている間に変わったファイル。空でなければ、その索引は置いた時点で既に古い。
    /// 件数ではなくファイルで持つ — 監視は 1 回の保存で複数のイベントを上げるので、
    /// 生の件数を出すと編集 2 回が 6 に見える。
    changed_during: std::collections::HashSet<PathBuf>,
    /// 次の世代のきっかけ。引き継がないと記録が「きっかけ不明・待ち時間 0 秒」になる。
    next_cause: Option<PathBuf>,
}

impl Run {
    fn asked(trigger: Trigger, cause: Option<PathBuf>) -> Self {
        Run {
            trigger: Some(trigger),
            cause,
            asked_at: Some(Instant::now()),
            ..Default::default()
        }
    }

    fn waited(&self) -> std::time::Duration {
        let (asked, started) = (self.asked_at, self.started_at);
        match (asked, started) {
            (Some(a), Some(s)) => s.duration_since(a),
            (Some(a), None) => a.elapsed(),
            _ => std::time::Duration::ZERO,
        }
    }

    fn took(&self) -> std::time::Duration {
        self.started_at.map(|s| s.elapsed()).unwrap_or_default()
    }
}

/// 索引 (sheaf-core の [`Store`]) の有無と、その作り直しを保持する。
///
/// `Store` は 1 つで、ツリーの中の索引ルートすべてを束ねたもの。作り直しはルートごとに
/// 独立して走る (道具も成果物も別なので、片方の失敗をもう片方に伝播させない)。
#[derive(Default)]
pub struct SemanticIndex {
    slot: Slot,
    /// いま列挙してある索引ルートと、その列挙元のツリー。ツリーが変われば引き直す。
    roots: Vec<Root>,
    tree: PathBuf,
    /// いま読んでいるファイル。同じものを繰り返し告げられたときに何もしないため。
    reading: Option<PathBuf>,
    /// main worktree の `.conductor/` と、それを引いた元のリポジトリ。内側の `None` は
    /// 「git リポジトリでない」。
    ///
    /// リポジトリを覚えておくのは、成果物の名前がリポジトリをまたいで同じ
    /// (`index.rust.scip`) なため。引き直さないと切り替え先の索引を切り替え元へ書き込む。
    conductor_dir: Option<(PathBuf, Option<PathBuf>)>,
}

impl SemanticIndex {
    /// `tree_root` に向いている索引。向いていなければ `None`。
    pub fn store(&self, tree_root: &Path) -> Option<&Store> {
        self.slot.get(tree_root)
    }

    /// いまこのファイルを読んでいる、と伝える。それを含む索引ルートに索引がまだ無ければ
    /// 1 本作らせる。索引ルートは実在するリポジトリで 109 本になることがあり、まとめて
    /// 作ると数十分かかるので、読むところから順に作る (全部なら `conductor index`)。
    ///
    /// 毎フレーム呼ばれる前提で、前回と同じファイルなら即座に返る。開く経路が 12 箇所
    /// あるので、そのどこかを通し忘れるより毎周見るほうが落ちない。
    pub fn note_open(&mut self, rel: &Path, repo_root: &Path, tree_root: &Path) -> Reading {
        // ツリーが動いていれば、いまの索引ルートは前のツリーのもの。調査が届くまで
        // 答えを確定させない。確定させると、同じ相対パスのファイルを開いたまま
        // worktree を切り替えたときに前のツリーの答えが残る。
        if self.tree != tree_root {
            return Reading::Loading;
        }
        if self.reading.as_deref() == Some(rel) {
            return Reading::Unchanged;
        }
        let settle = |me: &mut Self, answer| {
            me.reading = Some(rel.to_path_buf());
            answer
        };
        let Some(index) = self.owning_root(rel) else {
            return settle(self, Reading::NotIndexed);
        };
        let Some(dir) = self.conductor_dir(repo_root).map(Path::to_path_buf) else {
            return settle(self, Reading::NotIndexed);
        };
        // 鍵がまだ無い。調査が届いてから決める。
        let Some(key) = self.roots[index].key.clone() else {
            return Reading::Loading;
        };
        // 既にあるものを作り直さない。開くたびに走らせると producer が止まらなくなる。
        if !self.roots[index].at.has_generation(&dir, &key) {
            self.roots[index].request(Trigger::Open, Some(rel.to_path_buf()));
            return settle(self, Reading::Building);
        }
        if self.roots[index].is_working() {
            return settle(self, Reading::Building);
        }
        // 索引の読み込みは別スレッドなので、worktree を切り替えた直後は間に合って
        // いない。ここで「説明できている」と答えて答えを確定させると、古いことを
        // 言うべき唯一の場面 (別のツリーへ移った直後) で必ず黙ることになる。
        let Some(store) = self.slot.get(tree_root) else {
            return Reading::Loading;
        };
        // 索引はあるが、このファイルの今の内容は説明できていない。黙って構文層に
        // 落ちるので、言わないと「ジャンプが甘い」としか見えない。
        let answer = if store.is_current(rel) {
            Reading::Indexed
        } else {
            Reading::Stale
        };
        settle(self, answer)
    }

    /// いま読んでいるファイルの索引ルートを作り直す (画面からの頼み)。どの索引ルートにも
    /// 属さなければ `false`。
    pub fn rebuild_reading(&mut self) -> bool {
        let Some(rel) = self.reading.clone() else {
            return false;
        };
        let Some(index) = self.owning_root(&rel) else {
            return false;
        };
        self.roots[index].request(Trigger::Manual, Some(rel.clone()));
        true
    }

    /// ファイルが変わったことを伝える。作り直すのは、それを含む索引ルート 1 本だけ。
    pub fn note_change(&mut self, changed: &Path, tree_root: &Path) {
        if self.tree != tree_root {
            return;
        }
        let Ok(rel) = changed.strip_prefix(tree_root) else {
            return;
        };
        let Some(index) = self.owning_root(rel) else {
            return;
        };
        let at = tree_root.join(&self.roots[index].at.subroot);
        let root = &mut self.roots[index];
        // 内容が動いたので鍵も動く。引き直すまで生成を始めない。始めると、
        // 出来た索引が前の内容の名前で置かれ、次に読むときに一致しない。
        root.key = None;
        let was_working = root.is_working();
        root.regenerator.note_change(changed, &at);
        if !root.is_working() {
            return;
        }
        if root.regenerator.is_running() {
            // この世代の索引には入らない変更。置いた時点で既に古い。
            root.run.changed_during.insert(rel.to_path_buf());
            root.run.next_cause = Some(rel.to_path_buf());
        } else if !was_working {
            root.run = Run::asked(Trigger::Change, Some(rel.to_path_buf()));
        }
    }

    fn owning_root(&self, rel: &Path) -> Option<usize> {
        let all: Vec<IndexRoot> = self.roots.iter().map(|r| r.at.clone()).collect();
        owning_root_of(&all, rel)
    }

    /// 作り直しを 1 周進める。毎フレーム呼ばれる。待つものが無いうちに置き場所を
    /// 組み立てないのは、そこに git2 のリポジトリオープンが要るため。
    pub fn tick_regeneration(&mut self, repo_root: &Path, tree_root: &Path) -> Option<Finished> {
        if !self.roots.iter().any(|r| r.is_working()) {
            return None;
        }
        let dir = self.conductor_dir(repo_root)?.to_path_buf();
        // 1 周で返すのは 1 本ぶん。生成はロックで直列化されているので、
        // 同じ周に 2 本が終わることはほとんど無い。
        self.roots.iter_mut().find_map(|root| {
            // 鍵が引き直せていないルートは進めない (置く名前と中身が食い違う)。
            let key = root.key.clone()?;
            // この内容の索引はもう置いてある。producer を起こしても同じものが出る。
            // 編集で行ったり来たりするだけで 14 秒 / 2.3GiB を払わないための門。
            if root.regenerator.is_pending() && root.at.has_generation(&dir, &key) {
                root.log_with(&dir, history::Sources::Unknown, history::Outcome::Reused);
                root.regenerator.abort();
                root.run = Run::default();
                return None;
            }
            let target = root.at.target(&dir, tree_root, &key);
            let outcome = root.regenerator.tick(&target);
            // producer が立ったのはこの tick の中なので、前後で見て時刻を取る。
            if root.regenerator.is_running() && root.run.started_at.is_none() {
                root.run.started_at = Some(Instant::now());
                // producer は最後に出自の表を置き換えるので、いま読めば前の世代のもの。
                root.run.before = root.at.newest_provenance(&dir);
            }
            outcome.map(|outcome| {
                let manual = root.run.trigger == Some(Trigger::Manual);
                root.record(&dir, &key, &outcome);
                // 世代を残すのは行き来のためで、際限なく残す意味は無い。
                root.at.prune(&dir);
                Finished { outcome, manual }
            })
        })
    }

    /// 索引ルートの調査が要るか。要るなら、鍵を出しておくべきルート。
    ///
    /// 列挙も鍵の計算もツリーを歩くので (実測でそれぞれ 149ms と最大 110ms)、UI スレッド
    /// ではやらない。名指しで返すのは [`survey`] が鍵を出す相手を自分で選ぶためで、選から
    /// 漏れたルートはいつまでも「調査が要る」と言い続けることになる。
    pub fn needs_survey(&self, tree_root: &Path) -> Option<Vec<IndexRoot>> {
        if self.tree != tree_root {
            return Some(Vec::new());
        }
        let keyless: Vec<IndexRoot> = self
            .roots
            .iter()
            .filter(|r| r.key.is_none())
            .map(|r| r.at.clone())
            .collect();
        (!keyless.is_empty()).then_some(keyless)
    }

    /// 背景で調べた索引ルートを取り込む。調べている間にツリーが動いていれば捨てる —
    /// 取り込むと、いま見ていないツリーの鍵で生成を始めることになる。
    pub fn install(&mut self, survey: Survey, tree_root: &Path) {
        if survey.tree != tree_root {
            return;
        }
        if self.tree != tree_root {
            self.tree = tree_root.to_path_buf();
            self.reading = None;
            // 走っている生成は前のツリーを索引している。Regenerator を捨てれば
            // Drop がプロセスグループごと止める。
            self.roots.clear();
        }
        for (at, key) in survey.roots {
            match self.roots.iter_mut().find(|r| r.at == at) {
                // 走っている生成はそのまま続ける。鍵だけ差し替えると、置かれる索引の
                // 名前と中身が食い違うので、走っていない間にだけ入れる。
                Some(root) if !root.regenerator.is_running() => root.key = Some(key),
                Some(_) => {}
                None => self.roots.push(Root {
                    regenerator: Regenerator::new(at.lang.producer()),
                    at,
                    key: Some(key),
                    run: Run::default(),
                }),
            }
        }
    }

    fn conductor_dir(&mut self, repo_root: &Path) -> Option<&Path> {
        if self
            .conductor_dir
            .as_ref()
            .is_none_or(|(at, _)| at != repo_root)
        {
            self.conductor_dir = Some((repo_root.to_path_buf(), main_conductor_dir(repo_root)));
        }
        self.conductor_dir.as_ref()?.1.as_deref()
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.roots.iter().any(|r| r.regenerator.is_pending())
    }

    /// 走っている生成を止める。止めた時点までの producer の時間は捨てるので、記録に残す。
    pub fn abort_regeneration(&mut self, repo_root: &Path) {
        let dir = self.conductor_dir(repo_root).map(Path::to_path_buf);
        for root in &mut self.roots {
            if let (true, Some(dir)) = (root.regenerator.is_running(), dir.as_deref()) {
                root.log_with(dir, history::Sources::Unknown, history::Outcome::Aborted);
            }
            root.regenerator.abort();
            root.run = Run::default();
        }
    }

    /// 別のツリーを見に行くことになったなら、読み直しを待たずに捨てる。
    pub fn invalidate_if_retargeted(&mut self, tree_root: &Path) {
        self.slot.retarget(tree_root);
    }

    /// 背景ロードの結果を取り込む。取り込まなかったときは `false` を返すので、
    /// 呼び出し側はそれを見て読み直しを起こす。
    pub fn accept(&mut self, requested: &Path, current: &Path, store: Option<Store>) -> bool {
        self.slot.accept(requested, current, store)
    }
}

/// ツリーの索引ルートをすべて索引して置く。`conductor index` の実体。
///
/// 1 本が失敗しても残りは作る。道具は言語ごとに別なので、scip-go が入っていない
/// ことを理由に Rust の索引まで諦めるのは筋が違う。
pub fn build_index(repo_root: &Path) -> anyhow::Result<()> {
    let dir = main_conductor_dir(repo_root)
        .ok_or_else(|| anyhow::anyhow!("{} が git リポジトリではない", repo_root.display()))?;
    let found = roots::discover(repo_root);
    if found.is_empty() {
        anyhow::bail!(
            "{} に索引の作り方が分からない (Cargo.toml / go.mod / tsconfig.json のどれも無い)",
            repo_root.display()
        );
    }

    let mut failures = Vec::new();
    for at in &found {
        let key = at.content_key(repo_root, &deeper_than(&found, at));
        let target = at.target(&dir, repo_root, &key);
        let index = target.index.clone();
        let at_start = Instant::now();
        let before = at.newest_provenance(&dir);
        let outcome = sheaf_core::generate_once(target, at.lang.producer());
        at.prune(&dir);
        history::append(
            &dir,
            &history::Entry {
                root: &at.subroot,
                lang: at.lang.tag(),
                trigger: Trigger::Cli,
                cause: None,
                waited: std::time::Duration::ZERO,
                took: at_start.elapsed(),
                outcome: (&outcome).into(),
                sources: match &outcome {
                    sheaf_core::Outcome::Ready { .. } => {
                        source_delta(before.as_ref(), at, &dir, &key)
                    }
                    _ => history::Sources::Unknown,
                },
                changed_during: 0,
            },
        );
        match outcome {
            sheaf_core::Outcome::Ready { store } => println!(
                "{} に索引を置いた ({} document、うち出自を言えないもの {})",
                index.display(),
                store.len(),
                store.missing_provenance()
            ),
            sheaf_core::Outcome::Failed(why) | sheaf_core::Outcome::Unavailable(why) => {
                failures.push(format!("{:?}: {why}", at.lang))
            }
            sheaf_core::Outcome::Busy => {
                failures.push(format!("{:?}: ほかのプロセスが索引を作っている", at.lang))
            }
        }
    }

    if failures.len() == found.len() {
        anyhow::bail!("{}", failures.join("\n"));
    }
    for why in &failures {
        eprintln!("索引を作れなかった {why}");
    }
    Ok(())
}

/// ツリーを歩いて分かること。索引ルートと、それぞれの内容の鍵。
///
/// 歩くのに実測 149ms + ルート 1 本あたり最大 110ms かかるので、背景で作って
/// [`SemanticIndex::install`] で取り込む。
pub struct Survey {
    /// 調べたツリー。取り込む側が、その間に移っていないか見るのに使う。
    pub tree: PathBuf,
    pub roots: Vec<(IndexRoot, String)>,
}

/// `tree_root` の索引ルートを列挙し、鍵を計算する。鍵を出すのは `reading` を含むルート、
/// 既に成果物が置いてあるルート、`wanted` に名指しされたルートだけ — 実在するリポジトリ
/// では 109 本あり、全部の鍵を出すと 0.6 秒かかる。
pub fn survey(
    tree_root: &Path,
    conductor_dir: Option<&Path>,
    reading: Option<&Path>,
    wanted: &[IndexRoot],
) -> Survey {
    let found = roots::discover(tree_root);
    let owning = reading.and_then(|rel| owning_root_of(&found, rel));
    let roots = found
        .iter()
        .enumerate()
        .filter(|(i, at)| {
            Some(*i) == owning
                || wanted.contains(at)
                || conductor_dir.is_some_and(|dir| at.has_any_generation(dir))
        })
        .map(|(_, at)| {
            let key = at.content_key(tree_root, &deeper_than(&found, at));
            (at.clone(), key)
        })
        .collect();
    Survey {
        tree: tree_root.to_path_buf(),
        roots,
    }
}

/// `at` の中にある、同じ言語のより深い索引ルート (`at` から見た相対パス)。
fn deeper_than(all: &[IndexRoot], at: &IndexRoot) -> Vec<PathBuf> {
    all.iter()
        .filter(|other| other.lang == at.lang && other.subroot != at.subroot)
        .filter_map(|other| other.subroot.strip_prefix(&at.subroot).ok())
        .map(Path::to_path_buf)
        .collect()
}

/// `rel` を索引に載せるルートの位置。同じ言語のルートが入れ子なら深いほうが持つ
/// ([`Store::load`] の衝突の解き方に合わせる)。
fn owning_root_of(all: &[IndexRoot], rel: &Path) -> Option<usize> {
    let lang = roots::Language::of_file(rel)?;
    all.iter()
        .enumerate()
        .filter(|(_, at)| at.lang == lang && rel.starts_with(&at.subroot))
        .max_by_key(|(_, at)| at.subroot.components().count())
        .map(|(i, _)| i)
}

/// main worktree に置いてある索引を、`tree_root` のツリーに向けてロードする。1 本も
/// 投入できなければ `None`。索引ルートは `tree_root` から引き直す — 索引の中の相対パスを
/// ツリーのどこへ接ぎ木するかは、ツリーの側にあるルートで決まるため。
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
pub fn load(repo_root: &Path, tree_root: &Path) -> Option<Store> {
    survey_and_load(repo_root, tree_root, None, &[]).1
}

fn source_delta(
    before: Option<&HashMap<PathBuf, String>>,
    at: &IndexRoot,
    dir: &Path,
    key: &str,
) -> history::Sources {
    let Some(before) = before else {
        return history::Sources::First;
    };
    let Some(after) = at.provenance(dir, key) else {
        return history::Sources::Unknown;
    };
    let mine = |path: &Path| roots::Language::of_file(path) == Some(at.lang);
    let mut delta = history::SourceDelta::default();
    for (path, hash) in &after {
        if !mine(path) {
            continue;
        }
        match before.get(path) {
            None => delta.added += 1,
            Some(was) if was != hash => delta.modified += 1,
            Some(_) => {}
        }
    }
    delta.removed = before
        .keys()
        .filter(|p| mine(p) && !after.contains_key(*p))
        .count();
    history::Sources::Delta(delta)
}

/// `repo_root` がリンクされた worktree でも `commondir()` は常に main の `.git` を
/// 指すので、その親が main のルートになる。
fn main_conductor_dir(repo_root: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::open(repo_root).ok()?;
    Some(repo.commondir().parent()?.join(".conductor"))
}

/// 名前しか根拠が無い答えを、その言語のファイルに限るための判定。
///
/// tree-sitter の索引は名前でしか引けないので、`.go` の `rollbar` が `.tsx` の
/// `const rollbar` に当たる。分類できない拡張子は通す — 落とすと、いま答えている
/// ものまで黙って消える。
pub fn same_language(asking: &Path, candidate: &Path) -> bool {
    match (
        roots::Language::of_file(asking),
        roots::Language::of_file(candidate),
    ) {
        (Some(here), Some(there)) => here == there,
        _ => true,
    }
}

/// 種別を、ホバーの見出しに置く 1 語にする。綴りを Rust の宣言キーワードに寄せてあるのは、
/// ホバーの本文に索引が書いた宣言がそのまま並ぶため。読めない種別は空にして見出しごと出さない。
pub fn kind_label(kind: sheaf_core::SymbolKind) -> &'static str {
    use sheaf_core::SymbolKind::*;
    match kind {
        Function => "fn",
        Method => "method",
        Struct => "struct",
        Class => "class",
        Enum => "enum",
        EnumMember => "variant",
        Field => "field",
        Trait => "trait",
        Interface => "interface",
        Package => "package",
        TypeAlias => "type",
        AssociatedType => "assoc type",
        ImplBlock => "impl",
        Module => "mod",
        Constant => "const",
        Static => "static",
        Variable => "let",
        Parameter => "param",
        SelfParameter => "self",
        TypeParameter => "type param",
        Unknown => "",
    }
}

#[cfg(test)]
mod tests {
    use super::roots::Language;
    use super::*;

    /// 索引ルートの目印。これが無いツリーは索引の対象にならないので、
    /// 索引を置く検査ではソースと一緒にこれも置く。
    const CARGO_TOML: (&str, &str) = ("Cargo.toml", "[package]\nname = \"demo\"\n");

    /// Rust の索引を作る道具。出自の表の読み書きは道具ごとに照合されるので、
    /// 検査でも本番と同じものを渡す。
    fn producer() -> std::sync::Arc<dyn sheaf_core::Producer> {
        Language::Rust.producer()
    }

    /// 鍵は置き場所の親のツリーから出す。実際の内容の鍵で置かないと、読む側が探す名前と
    /// 食い違って、置いたはずの索引が見つからない。
    fn artifact(dir: &Path, ext: &str) -> PathBuf {
        let tree = dir.parent().expect(".conductor の親がツリー");
        let at = IndexRoot {
            subroot: PathBuf::new(),
            lang: Language::Rust,
        };
        dir.join(format!("index.rust.{}.{ext}", key_in(tree, &at)))
    }

    /// commit_sha でコミットした状態のリポジトリを tempdir に作り、(repo_root, commit_sha) を返す。
    fn init_repo_with_commit(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }

        let mut index = repo.index().unwrap();
        for (rel, _) in files {
            index.add_path(Path::new(rel)).unwrap();
        }
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        let commit_id = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        (dir, commit_id.to_string())
    }

    #[test]
    fn no_scip_and_no_hashes_file_is_none() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        assert!(load(dir.path(), dir.path()).is_none());
    }

    #[test]
    fn missing_scip_file_is_none() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        std::fs::write(artifact(&conductor_dir, "hashes"), "").unwrap();
        assert!(load(dir.path(), dir.path()).is_none());
    }

    /// 引数を無視してひたすら sleep するだけの producer。生成が走っている最中を
    /// 安定して作るために使う。
    struct SlowProducer(PathBuf);

    impl sheaf_core::Producer for SlowProducer {
        fn command(&self, _out: &Path) -> Vec<String> {
            vec![self.0.to_string_lossy().into_owned()]
        }
    }

    #[test]
    fn tick_regeneration_は生成が走っていても即座に返る() {
        // 「バックグラウンドでやっている」ことの回帰ガード。索引の読み込みや
        // 生成の待ち合わせがここに紛れ込むと、呼び出し元(イベントループ)を止める。
        use std::time::{Duration, Instant};

        // Rust のツリーとして認識されないと生成そのものが起きない (target を参照)。
        let (dir, _commit) = init_repo_with_commit(&[
            ("src/lib.rs", "fn f() {}\n"),
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ]);
        std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();

        let script = dir.path().join("slow-producer.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        // ツリーを既に引いてあることにしないと、sync_roots が rust-analyzer を持つ
        // Regenerator に差し替えてしまう。
        let mut semantic = SemanticIndex {
            tree: dir.path().to_path_buf(),
            roots: vec![Root {
                at: IndexRoot {
                    subroot: PathBuf::new(),
                    lang: Language::Rust,
                },
                regenerator: sheaf_core::Regenerator::new(std::sync::Arc::new(SlowProducer(
                    script,
                ))),
                key: Some("0123456789ab".to_string()),
                run: Run::default(),
            }],
            ..Default::default()
        };
        semantic.note_change(&dir.path().join("src/lib.rs"), dir.path());
        // 編集で鍵が落ちる。鍵の無いルートは生成を始めないので、背景の調査が
        // 届いた状態にしておく。
        semantic.roots[0].key = Some("0123456789ab".to_string());

        // 静穏時間が経つのを待って、生成が実際に走っている状態を作る。
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let outcome = semantic.tick_regeneration(dir.path(), dir.path());
            assert!(
                outcome.is_none(),
                "30 秒 sleep する producer がもう終わっているはずがない"
            );
            if !semantic.is_pending() {
                break;
            }
            assert!(Instant::now() < deadline, "生成が始まらない");
            std::thread::sleep(Duration::from_millis(100));
        }

        let at = Instant::now();
        let outcome = semantic.tick_regeneration(dir.path(), dir.path());
        let elapsed = at.elapsed();

        assert!(outcome.is_none());
        assert!(
            elapsed < Duration::from_millis(200),
            "tick が生成を待ってしまっている: {elapsed:?}"
        );

        semantic.abort_regeneration(dir.path());
    }

    #[test]
    fn 索引ルートの無いツリーでは生成を起こさない() {
        // どの目印も無いツリーに道具を向けても意味が無い。しかも認識できない対象に
        // 対して終了コード 0 で空の索引を書くことがあるので、起こさないこと自体が答え。
        let (dir, _commit) = init_repo_with_commit(&[("main.go", "package main\n")]);
        let mut semantic = SemanticIndex::default();
        semantic.note_change(&dir.path().join("main.go"), dir.path());

        assert!(
            semantic.tick_regeneration(dir.path(), dir.path()).is_none(),
            "目印が無いのに生成が始まった"
        );
    }

    #[test]
    fn go_のツリーには_scip_go_を向ける() {
        // ここが Rust 決め打ちだと、Go のリポジトリは索引が 1 本も無いまま
        // tree-sitter の名前一致に落ち続ける。画面には出ないので気づけない。
        let (dir, _commit) =
            init_repo_with_commit(&[("go.mod", "module demo\n"), ("main.go", "package main\n")]);

        let mut semantic = surveyed(dir.path(), Some("main.go"));
        semantic.note_change(&dir.path().join("main.go"), dir.path());

        assert!(semantic.is_pending(), "go.mod があるのに生成を待っていない");
        let argv = semantic.roots[0]
            .at
            .lang
            .producer()
            .command(Path::new("/o"));
        assert_eq!(argv[0], "scip-go");
    }

    /// conductor が Go のツリーに scip-go を向け、その索引で `Exact` に答えるまでを一続きで
    /// 見る。読み取り側は sheaf のテストが見ているので、ここで見たいのは host 側の配線だけ。
    /// scip-go が無ければ飛ばさずに落とす — 飛ばすと配線が壊れていても緑になる。
    #[test]
    fn go_のツリーを索引して定義に飛べる() {
        let (dir, _commit) = init_repo_with_commit(&[
            ("go.mod", "module example.com/app\n\ngo 1.21\n"),
            (
                "pkg/greet/greet.go",
                "package greet\n\nfunc Greet() string {\n\treturn \"hi\"\n}\n",
            ),
            (
                "main.go",
                "package main\n\nimport \"example.com/app/pkg/greet\"\n\nfunc main() {\n\tprintln(greet.Greet())\n}\n",
            ),
        ]);
        let root = dir.path();
        build_index(root).expect("Go のツリーを索引できない");

        // 生成 1 件につき 1 行。あとから「いつ・どこを・どれだけかけて」を追える。
        let log = std::fs::read_to_string(root.join(".conductor/index-history.log")).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("trigger=cli"), "{log}");
        assert!(log.contains("result=ok documents=2"), "{log}");

        let store = load(root, root).expect("置いた索引を読めない");
        let rel = Path::new("main.go");
        let source = std::fs::read_to_string(root.join(rel)).unwrap();
        let (line, text) = source
            .lines()
            .enumerate()
            .find(|(_, t)| t.contains("greet.Greet()"))
            .unwrap();
        let col = text.find("Greet()").unwrap();

        let mask = crate::symbol_index::CodeMask::compute(&source, "main.go");
        let index = crate::symbol_index::SymbolIndex::new(root.to_path_buf());
        let bridge = Bridge {
            abs_path: &root.join(rel),
            source: &source,
            mask: &mask,
            index: &index,
        };
        assert_eq!(
            sheaf_core::definition_at(&store, &bridge, rel, line as u32, col as u32),
            sheaf_core::Definition::Exact(vec![sheaf_core::Location {
                path: PathBuf::from("pkg/greet/greet.go"),
                line: 2,
                col: 5,
            }])
        );
    }

    /// このツリーに対するそのルートの鍵。[`survey`] と同じ出し方をしないと、置いた索引が見つからない。
    fn key_in(tree: &Path, at: &IndexRoot) -> String {
        let found = roots::discover(tree);
        at.content_key(tree, &deeper_than(&found, at))
    }

    /// 背景の調査を済ませた `SemanticIndex`。note_open はこれが無いと
    /// `Loading` のまま何もしない。
    fn surveyed(tree: &Path, reading: Option<&str>) -> SemanticIndex {
        let mut semantic = SemanticIndex::default();
        let conductor = tree.join(".conductor");
        let survey = survey(tree, Some(&conductor), reading.map(Path::new), &[]);
        semantic.install(survey, tree);
        semantic
    }

    /// 索引ルートが 2 本ある Go のツリー。`.conductor/` も掘っておく。
    fn nested_go_tree() -> tempfile::TempDir {
        let (dir, _commit) = init_repo_with_commit(&[
            ("go.mod", "module demo\n"),
            ("main.go", "package main\n"),
            ("services/api/go.mod", "module demo/api\n"),
            ("services/api/api.go", "package api\n"),
        ]);
        std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();
        dir
    }

    #[test]
    fn 読んでいるファイルの索引ルートだけに索引を作らせる() {
        // 実在するリポジトリで索引ルートは 109 本になる。まとめて作ると数十分。
        let dir = nested_go_tree();
        let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));

        semantic.note_open(Path::new("services/api/api.go"), dir.path(), dir.path());

        let pending: Vec<_> = semantic
            .roots
            .iter()
            .filter(|r| r.regenerator.is_pending())
            .map(|r| r.at.subroot.clone())
            .collect();
        assert_eq!(pending, vec![PathBuf::from("services/api")]);
    }

    #[test]
    fn 索引が既にあるルートには作り直しを頼まない() {
        // 開くたびに頼むと、大きなリポジトリでは producer が止まらなくなる。
        let dir = nested_go_tree();
        let at = IndexRoot {
            subroot: PathBuf::from("services/api"),
            lang: Language::Go,
        };
        let conductor_dir = dir.path().join(".conductor");
        let target = at.target(&conductor_dir, dir.path(), &key_in(dir.path(), &at));
        write_index_for(&target.index, &["api.go"]);
        sheaf_core::write_provenance(&target.hashes, &*at.lang.producer(), &Default::default())
            .unwrap();

        let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));
        semantic.note_open(Path::new("services/api/api.go"), dir.path(), dir.path());

        assert!(
            !semantic.roots.iter().any(|r| r.regenerator.is_pending()),
            "索引があるのに作り直しを頼んだ"
        );
    }

    #[test]
    fn 鍵を失ったルートは名指しで調べ直される() {
        // 調査は鍵を出す相手を自分で選ぶ (109 本ぶんの鍵は 0.6 秒かかる)。選から漏れたルートは
        // 「調査が要る」と言い続け、背景の調査が毎フレーム走ることになる。
        let dir = nested_go_tree();
        let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));
        semantic.note_change(&dir.path().join("services/api/api.go"), dir.path());

        let wanted = semantic
            .needs_survey(dir.path())
            .expect("鍵が無いのに調べ直しを求めていない");
        assert!(!wanted.is_empty(), "鍵の要るルートを名指ししていない");

        // 読んでいるファイルを渡さなくても、名指しなら鍵が付くこと。
        semantic.install(survey(dir.path(), None, None, &wanted), dir.path());
        assert!(
            semantic.needs_survey(dir.path()).is_none(),
            "調べ直したのに鍵が付いていない"
        );
    }

    #[test]
    fn 索引ルートが複数あればすべて畳んで読む() {
        // 1 世代が作るのは 1 ルートぶん。それをそのまま投入すると、他のルートの
        // 索引が黙って落ちて、そこは以後ずっと構文層で答えることになる。
        let dir = nested_go_tree();
        let conductor_dir = dir.path().join(".conductor");
        for (subroot, docs) in [("", ["main.go"]), ("services/api", ["api.go"])] {
            let at = IndexRoot {
                subroot: PathBuf::from(subroot),
                lang: Language::Go,
            };
            let target = at.target(&conductor_dir, dir.path(), &key_in(dir.path(), &at));
            write_index_for(&target.index, &docs);
            sheaf_core::write_provenance(&target.hashes, &*at.lang.producer(), &Default::default())
                .unwrap();
        }

        let store = load(dir.path(), dir.path()).expect("置いた索引を読めない");
        assert_eq!(store.len(), 2, "索引ルートのどちらかが落ちた");
    }

    #[test]
    fn 入れ子のルートの編集は外側の索引を起こさない() {
        // go.mod はモジュールの境界なので、外側の索引に内側のパッケージは入らない。
        // 起こすと、変わっていない索引を作り直すだけになる。
        let dir = nested_go_tree();
        let mut semantic = surveyed(dir.path(), Some("services/api/api.go"));

        semantic.note_change(&dir.path().join("services/api/api.go"), dir.path());

        let pending: Vec<_> = semantic
            .roots
            .iter()
            .filter(|r| r.regenerator.is_pending())
            .map(|r| r.at.subroot.clone())
            .collect();
        assert_eq!(pending, vec![PathBuf::from("services/api")]);
    }

    #[test]
    fn 索引に載らないファイルの変更では作り直さない() {
        let dir = nested_go_tree();
        let mut semantic = surveyed(dir.path(), None);

        semantic.note_change(&dir.path().join("README.md"), dir.path());

        assert!(!semantic.roots.iter().any(|r| r.regenerator.is_pending()));
    }

    /// 入れ子の索引ルートで作った索引が、ツリーのルートから見た正しいパスに飛ぶ。索引の中の
    /// 綴りは索引ルート相対 (`handler/handler.go`) なので、接ぎ木を誤ると存在しないパスへ飛ぶ。
    #[test]
    fn 入れ子の索引ルートの中で定義に飛べる() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        for (rel, content) in [
            ("go.mod", "module example.com/app\n\ngo 1.21\n"),
            ("main.go", "package main\n\nfunc main() {}\n"),
            ("services/api/go.mod", "module example.com/api\n\ngo 1.21\n"),
            (
                "services/api/handler/handler.go",
                "package handler\n\nfunc Handle() string {\n\treturn \"ok\"\n}\n",
            ),
            (
                "services/api/main.go",
                "package main\n\nimport \"example.com/api/handler\"\n\nfunc main() {\n\tprintln(handler.Handle())\n}\n",
            ),
        ] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        let root = dir.path();
        build_index(root).expect("2 本の索引ルートを索引できない");

        let store = load(root, root).expect("置いた索引を読めない");
        let rel = Path::new("services/api/main.go");
        let source = std::fs::read_to_string(root.join(rel)).unwrap();
        let (line, text) = source
            .lines()
            .enumerate()
            .find(|(_, t)| t.contains("handler.Handle()"))
            .unwrap();
        let col = text.find("Handle()").unwrap();

        let mask = crate::symbol_index::CodeMask::compute(&source, "main.go");
        let index = crate::symbol_index::SymbolIndex::new(root.to_path_buf());
        let bridge = Bridge {
            abs_path: &root.join(rel),
            source: &source,
            mask: &mask,
            index: &index,
        };
        assert_eq!(
            sheaf_core::definition_at(&store, &bridge, rel, line as u32, col as u32),
            sheaf_core::Definition::Exact(vec![sheaf_core::Location {
                path: PathBuf::from("services/api/handler/handler.go"),
                line: 2,
                col: 5,
            }])
        );
    }

    /// 索引を投入済みの `SemanticIndex`。
    fn loaded(repo_root: &Path) -> SemanticIndex {
        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");
        let mut semantic = surveyed(repo_root, None);
        assert!(semantic.accept(repo_root, repo_root, Some(store)));
        semantic
    }

    #[test]
    fn 索引が今の内容を説明できているときは何も言わない() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());
        let mut semantic = loaded(dir.path());

        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
            Reading::Indexed
        );
    }

    #[test]
    fn 内容の変わったツリーを読んだら作りに行く() {
        // 索引は内容ごとに名前が分かれるので、内容が動けばその内容の索引はまだ無い。
        // 待つのではなく作りに行かないと、worktree を移るたびに構文層のまま据え置かれる。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
        let mut semantic = loaded(dir.path());
        semantic.install(
            survey(
                dir.path(),
                Some(&dir.path().join(".conductor")),
                Some(Path::new("src/lib.rs")),
                &[],
            ),
            dir.path(),
        );

        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
            Reading::Building
        );
        assert!(
            semantic.roots.iter().any(|r| r.is_working()),
            "いまの内容の索引が無いのに作りに行っていない"
        );
    }

    /// 置き場所に残っている Rust の索引の本数。
    fn generation_count(conductor_dir: &Path) -> usize {
        std::fs::read_dir(conductor_dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("index.rust.") && name.ends_with(".scip")
            })
            .count()
    }

    #[test]
    fn 一度作った内容の索引は戻ってきても作り直さない() {
        // 索引を 1 本しか持たないと、内容の違う worktree を行き来するたびに上書きし合い、
        // 戻るたびに 14 秒 / 2.3GiB を払うことになる。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let conductor_dir = dir.path().join(".conductor");
        let lib = dir.path().join("src/lib.rs");

        place_index(dir.path());
        std::fs::write(&lib, "pub fn greet() {}\nfn other() { greet(); }\n").unwrap();
        place_index(dir.path());
        assert_eq!(
            generation_count(&conductor_dir),
            2,
            "内容が違うのに同じ名前で上書きしている"
        );

        std::fs::write(&lib, SOURCE).unwrap();
        let mut semantic = surveyed(dir.path(), Some("src/lib.rs"));
        let store = load(dir.path(), dir.path()).expect("戻った内容の索引を読めない");
        assert!(semantic.accept(dir.path(), dir.path(), Some(store)));

        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
            Reading::Indexed
        );
        assert!(
            !semantic.roots.iter().any(|r| r.is_working()),
            "前に作った内容に戻っただけなのに作り直した"
        );
    }

    #[test]
    fn 世代は上限まで残して古いものから落とす() {
        // 残す意味があるのは行き来する worktree のぶんだけ。際限なく残すと、
        // 二度と一致しない索引がディスクを食う。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let conductor_dir = dir.path().join(".conductor");
        let lib = dir.path().join("src/lib.rs");
        for n in 0..6 {
            std::fs::write(&lib, format!("pub fn greet() {{}}\n// {n}\n")).unwrap();
            place_index(dir.path());
        }
        assert_eq!(generation_count(&conductor_dir), 6);

        let at = IndexRoot {
            subroot: PathBuf::new(),
            lang: Language::Rust,
        };
        at.prune(&conductor_dir);
        assert_eq!(generation_count(&conductor_dir), 4);
    }

    #[test]
    fn 索引が説明できないファイルを読んでいることを伝える() {
        // 索引はいまの内容のものなのに、このファイルだけ載っていない。作り直しても同じものが
        // 出るので、黙って構文層に落ちる。言わないと「ジャンプが甘い」としか見えない。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        write_index(&artifact(&conductor_dir, "scip"));
        // 出自を 1 件も申告しない索引。鍵はツリーから出るので一致したままになる。
        write_hashes(&artifact(&conductor_dir, "hashes"), &[]);
        let mut semantic = loaded(dir.path());

        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
            Reading::Stale
        );
        assert!(!semantic.roots.iter().any(|r| r.is_working()));
    }

    #[test]
    fn 索引をまだ読み込めていないうちは答えを確定させない() {
        // 索引の読み込みは別スレッドで、worktree 切替の直後は間に合っていない。ここで確定
        // させると、古いことを言うべき唯一の場面で必ず黙る。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        write_index(&artifact(&conductor_dir, "scip"));
        write_hashes(&artifact(&conductor_dir, "hashes"), &[]);
        let mut semantic = surveyed(dir.path(), None);

        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
            Reading::Loading
        );

        let store = load(dir.path(), dir.path()).unwrap();
        assert!(semantic.accept(dir.path(), dir.path(), Some(store)));
        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path()),
            Reading::Stale,
            "読み込み待ちの周で答えを確定させてしまっている"
        );
    }

    #[test]
    fn 手で頼まれたら読んでいるルートを作り直す() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());
        let mut semantic = loaded(dir.path());
        semantic.note_open(Path::new("src/lib.rs"), dir.path(), dir.path());

        assert!(semantic.rebuild_reading());
        assert!(semantic.roots.iter().any(|r| r.regenerator.is_pending()));
    }

    #[test]
    fn 同じパスのまま別のツリーへ移ったら調査が届くまで答えない() {
        // 「前回と同じファイル」の早期リターンがツリーの照合より前にあると、worktree を移った
        // あとも前のツリーの索引ルートで答え続ける。誤りに見えない誤答になる。
        let (rust, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let (plain, _commit) = init_repo_with_commit(&[("src/lib.rs", SOURCE)]);
        let mut semantic = surveyed(rust.path(), Some("src/lib.rs"));

        semantic.note_open(Path::new("src/lib.rs"), rust.path(), rust.path());
        assert_eq!(semantic.roots.len(), 1);

        assert_eq!(
            semantic.note_open(Path::new("src/lib.rs"), plain.path(), plain.path()),
            Reading::Loading,
            "別のツリーなのに前のツリーの索引ルートで答えた"
        );
        assert_eq!(
            semantic.needs_survey(plain.path()),
            Some(Vec::new()),
            "ツリーが変わったのに調べ直しを求めていない"
        );

        // 調査が届けば、目印の無いツリーには索引ルートが無いこと。
        semantic.install(
            survey(plain.path(), None, Some(Path::new("src/lib.rs")), &[]),
            plain.path(),
        );
        assert!(semantic.roots.is_empty());
    }

    #[test]
    fn リポジトリを切り替えたら成果物の置き場所も引き直す() {
        // 成果物の名前はリポジトリをまたいで同じ (index.rust.scip) なので、置き場所を引き直さないと
        // 切り替え先の索引を切り替え元へ書き込み、相手の索引を上書きする。
        let (first, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let (second, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        let mut semantic = SemanticIndex::default();

        // macOS の /var は /private/var への symlink で、git2 は解決した側を返す。
        let expected = |repo: &Path| repo.canonicalize().unwrap().join(".conductor");

        semantic.note_open(Path::new("src/lib.rs"), first.path(), first.path());
        assert_eq!(
            semantic.conductor_dir(first.path()),
            Some(expected(first.path()).as_path())
        );

        semantic.note_open(Path::new("src/lib.rs"), second.path(), second.path());
        assert_eq!(
            semantic.conductor_dir(second.path()),
            Some(expected(second.path()).as_path()),
            "切り替え元の .conductor を指したまま"
        );
    }

    #[test]
    fn 読んでいるファイルが無ければ手の作り直しは断る() {
        let mut semantic = SemanticIndex::default();
        assert!(!semantic.rebuild_reading());
    }

    #[test]
    fn missing_hashes_file_is_none() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", "fn f() {}\n")]);
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        // 本物の SCIP を置いても、出自の表が無ければ None を返すこと。壊れた索引を置くと
        // 出自を見ずに失敗しても None になり、検査したいことを検査できない。
        write_index(&artifact(&conductor_dir, "scip"));
        assert!(load(dir.path(), dir.path()).is_none());
    }

    const SOURCE: &str = "pub fn greet() {}\nfn caller() { greet(); }\n";
    const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";

    /// `Store` はシンボル文字列だけで定義を引くので、複数ファイルを 1 つの索引に入れる
    /// ときは同じ SYMBOL を使い回せない (別ファイルの定義まで拾う)。
    fn write_index_for(path: &Path, rels: &[&str]) {
        use protobuf::{EnumOrUnknown, Message, MessageField};
        use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};

        let documents = rels
            .iter()
            .map(|rel| {
                let symbol = format!("{SYMBOL} {rel}");
                let occurrence = |range: Vec<i32>, roles: i32| Occurrence {
                    range,
                    symbol: symbol.clone(),
                    symbol_roles: roles,
                    ..Default::default()
                };
                Document {
                    relative_path: rel.to_string(),
                    language: "rust".to_string(),
                    occurrences: vec![
                        occurrence(vec![0, 7, 12], 1),
                        occurrence(vec![1, 14, 19], 0),
                    ],
                    ..Default::default()
                }
            })
            .collect();
        let index = Index {
            metadata: MessageField::some(Metadata {
                text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
                ..Default::default()
            }),
            documents,
            ..Default::default()
        };
        std::fs::write(path, index.write_to_bytes().unwrap()).unwrap();
    }

    fn write_index(path: &Path) {
        write_index_for(path, &["src/lib.rs"]);
    }

    /// `fn caller() { greet(); }` の greet の位置を、索引に向けて引く。
    fn definition_of_greet_at(
        store: &Store,
        tree_root: &Path,
        rel: &str,
    ) -> sheaf_core::Definition {
        let abs = tree_root.join(rel);
        let source = std::fs::read_to_string(&abs).unwrap();
        let mask = crate::symbol_index::CodeMask::compute(&source, rel);
        let index = crate::symbol_index::SymbolIndex::new(tree_root.to_path_buf());
        let bridge = Bridge {
            abs_path: &abs,
            source: &source,
            mask: &mask,
            index: &index,
        };
        sheaf_core::definition_at(store, &bridge, Path::new(rel), 1, 14)
    }

    fn definition_of_greet(store: &Store, tree_root: &Path) -> sheaf_core::Definition {
        definition_of_greet_at(store, tree_root, "src/lib.rs")
    }

    /// 「生成時点でディスクにあった内容」を申告する体で、コミット済みかは問わない。
    fn place_index(repo_root: &Path) {
        let conductor_dir = repo_root.join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        write_index(&artifact(&conductor_dir, "scip"));
        let content = std::fs::read(repo_root.join("src/lib.rs")).unwrap();
        write_hashes(
            &artifact(&conductor_dir, "hashes"),
            &[("src/lib.rs", sheaf_core::blob_hash(&content))],
        );
    }

    /// 書式は sheaf の持ち物なので `write_provenance` に書かせる。手で綴ると、読み書きの
    /// どちらかが変わったときに表が黙って読まれなくなる。
    fn write_hashes(path: &Path, entries: &[(&str, String)]) {
        let expected = entries
            .iter()
            .map(|(rel, hash)| (PathBuf::from(rel), hash.clone()))
            .collect();
        sheaf_core::write_provenance(path, &*producer(), &expected).unwrap();
    }

    #[test]
    fn indexed_tree_answers_with_exact() {
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let store = load(dir.path(), dir.path()).expect("索引と出自の申告が揃っている");
        assert_eq!(
            definition_of_greet(&store, dir.path()),
            sheaf_core::Definition::Exact(vec![sheaf_core::Location {
                path: PathBuf::from("src/lib.rs"),
                line: 0,
                col: 7,
            }])
        );
    }

    #[test]
    fn linked_worktree_finds_the_index_at_the_main_worktree() {
        // リンクされた worktree の Repository::workdir() はリンク先自身を指すので、
        // repo_root にそれをそのまま渡すと、main 側にしか無い .conductor/ が
        // 見つからない。commondir() を辿って main を解決できているかを確かめる。
        let (dir, commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let wt_parent = tempfile::tempdir().unwrap();
        let wt_path = wt_parent.path().join("linked-wt");
        let status = std::process::Command::new("git")
            // ユーザのグローバル/システム git 設定から隔離し、テスト対象と
            // 無関係な理由で失敗しないようにする。
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-C",
                dir.path().to_str().unwrap(),
                "worktree",
                "add",
                "-b",
                "wt-branch",
                wt_path.to_str().unwrap(),
                &commit,
            ])
            .status()
            .unwrap();
        assert!(status.success(), "git worktree add failed");

        let store = load(&wt_path, &wt_path).expect("main 側の索引が見つかるはず");
        assert!(matches!(
            definition_of_greet(&store, &wt_path),
            sheaf_core::Definition::Exact(_)
        ));
    }

    #[test]
    fn other_tree_with_the_same_content_still_answers() {
        // worktree の形。内容が同じファイルは索引を使い回せる。これが成り立たないと
        // worktree ごとに索引を作ることになり、この設計の意味が無くなる。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join("src")).unwrap();
        std::fs::write(other.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
        std::fs::write(other.path().join("src/lib.rs"), SOURCE).unwrap();

        let store = load(dir.path(), other.path()).expect("索引と出自の申告が揃っている");
        assert!(matches!(
            definition_of_greet(&store, other.path()),
            sheaf_core::Definition::Exact(_)
        ));
    }

    #[test]
    fn other_tree_with_a_changed_file_does_not_answer() {
        // 同じ worktree の形だが、そのファイルが編集されている。索引の言う 0 行目は
        // もう greet の定義ではないので、確信度つきで答えてはいけない。
        //
        // 聞く行(1行目)は両方のツリーで同じにしてある。ここを動かすと、問い合わせ位置が
        // 別の語にずれて「語が無い」で落ちるだけになり、鮮度を検査しないまま緑になる。
        let (dir, _commit) = init_repo_with_commit(&[CARGO_TOML, ("src/lib.rs", SOURCE)]);
        place_index(dir.path());

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join("src")).unwrap();
        std::fs::write(other.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
        std::fs::write(
            other.path().join("src/lib.rs"),
            "pub fn hello() {}\nfn caller() { greet(); }\n",
        )
        .unwrap();

        let store = load(dir.path(), other.path()).expect("索引と出自の申告が揃っている");
        assert!(!matches!(
            definition_of_greet(&store, other.path()),
            sheaf_core::Definition::Exact(_)
        ));
    }

    /// 実際に置かれている索引で、git2 のツリー走査と投入が通ることを見る。
    /// 合成した索引では、実索引の Document 数(345)やパスの綴りまでは検査できない。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_loads_from_the_repository_it_was_generated_for() {
        let repo_root = std::env::var("CONDUCTOR_TEST_REPO")
            .expect("CONDUCTOR_TEST_REPO に .conductor/ へ索引を置いたリポジトリのパスを渡すこと");
        let repo_root = Path::new(&repo_root);

        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");
        println!(
            "{} Document / ルート外 {} / 保持 {:.1}MB",
            store.len(),
            store.outside_root(),
            store.retained_bytes() as f64 / 1048576.0,
        );
        assert!(!store.is_empty());
        assert_eq!(store.outside_root(), 0, "ツリー外を指す Document がある");
    }

    /// 実索引で、行を囲んでいるシンボルが取れる。合成フィクスチャでは producer が
    /// enclosing_range を実際に書くかを検査できない。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_answers_what_encloses_a_line() {
        let repo_root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
        let repo_root = Path::new(&repo_root);
        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");

        let rel = Path::new("src/hover_info.rs");
        let source = std::fs::read_to_string(repo_root.join(rel)).unwrap();
        let declaration = source
            .lines()
            .position(|l| l.contains("pub fn build_hover_info"))
            .expect("目印の関数がある");
        let inside = declaration + 3;

        let sheaf_core::Enclosures::Exact(found) =
            sheaf_core::enclosures_at(&store, rel, inside as u32)
        else {
            panic!("索引が囲みを答えなかった");
        };
        println!(
            "{} 行を囲むもの {} 件: {:?}",
            inside + 1,
            found.len(),
            found
                .iter()
                .map(|e| (e.declaration.line + 1, e.first_line + 1, e.last_line + 1))
                .collect::<Vec<_>>()
        );
        // 画面の外にある宣言のうちいちばん内側、が sticky に出るもの。
        assert_eq!(
            found
                .iter()
                .map(|e| e.declaration.line as usize)
                .find(|line| *line < inside),
            Some(declaration),
            "画面の外にある宣言のうちいちばん内側が build_hover_info になっていない"
        );
    }

    /// 索引が説明を答える割合を、実リポジトリの実 Bridge (tree-sitter) 越しに測る。
    ///
    /// 合成した索引では、rust-analyzer が実際に何を書くかを検査できない。ここが
    /// 落ちるのは、宣言の綴りが変わって読めなくなったとき (`Signature` の
    /// フィールド番号がまさにそれ) と、種別の番号が変わったとき。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_describes_what_it_answers() {
        use crate::symbol_index::{code_identifiers_on_line, occurrence_span_in_source};

        let repo_root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
        let repo_root = Path::new(&repo_root);
        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");

        // 索引はこのワークスペースのシンボルしか説明を持たない (rust-analyzer は
        // SCIP の external_symbols を書かないので、std や ratatui の語は符号だけ)。
        // 全体の割合で見ると、その欠落と自前の欠落が混ざって回帰に気づけない。
        let own = |symbol: &str| {
            symbol.starts_with("local ")
                || matches!(
                    symbol.split(' ').nth(2),
                    Some("conductor" | "sheaf-core" | "revidere" | "revidere-fixtures")
                )
        };

        let (mut asked, mut described, mut own_described, mut own_signature) = (0, 0, 0, 0);
        let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
        for rel in [
            "src/repo_path.rs",
            "src/jump_history.rs",
            "src/hover_info.rs",
        ] {
            let abs = repo_root.join(rel);
            let source = std::fs::read_to_string(&abs).unwrap();
            let mask = crate::symbol_index::CodeMask::compute(&source, rel);
            let index = crate::symbol_index::SymbolIndex::new(repo_root.to_path_buf());
            let bridge = Bridge {
                abs_path: &abs,
                source: &source,
                mask: &mask,
                index: &index,
            };
            for (line, text) in source.lines().enumerate() {
                for (k, _, word) in code_identifiers_on_line(text, line + 1, &mask) {
                    let Some((start, end)) = occurrence_span_in_source(text, k) else {
                        continue;
                    };
                    if text.get(start..end) != Some(word.as_str()) {
                        continue;
                    }
                    asked += 1;
                    let answer = sheaf_core::describe_at(
                        &store,
                        &bridge,
                        Path::new(rel),
                        line as u32,
                        start as u32,
                    );
                    let Some(detail) = answer.first() else {
                        continue;
                    };
                    described += 1;
                    if !own(detail.symbol.as_str()) {
                        continue;
                    }
                    own_described += 1;
                    if detail.signature.is_some() {
                        own_signature += 1;
                    }
                    let label = kind_label(detail.kind);
                    if !label.is_empty() {
                        *kinds.entry(label).or_default() += 1;
                    }
                }
            }
        }

        println!(
            "聞いた {asked} / 符号が付いた {described} / うち自前 {own_described} / 宣言 {own_signature}"
        );
        println!("種別の内訳: {kinds:?}");
        assert!(
            own_described > 100,
            "索引がほとんど答えていない: {own_described}"
        );
        // 自前のシンボルには索引が必ず SymbolInformation を書く。ここが落ちるのは
        // 宣言の綴りか種別の番号が変わったとき。
        assert!(
            own_signature * 20 >= own_described * 19,
            "自前のシンボルの宣言が読めていない: {own_signature}/{own_described}"
        );
        let with_kind: usize = kinds.values().sum();
        assert!(
            with_kind * 20 >= own_described * 19,
            "自前のシンボルの種別が読めていない: {with_kind}/{own_described}"
        );
        // 分類が 1 種類に潰れていたら、番号の対応表が壊れている。
        assert!(kinds.len() >= 5, "種別が偏りすぎ: {kinds:?}");
    }

    /// 呼び出し口(`App::pick_line_identifier`)が選ばせうる位置を、リポジトリの
    /// 実ファイルで全部叩く。索引が実際にどれだけ答えるかと、飛び先が
    /// リポジトリ内の実在する位置であることを見る。
    ///
    /// 呼び出し口は viewer が持つタブ展開済みの行から出現インデックスを取るが、
    /// 対象は Rust なのでタブを含む行が無く、ここでは元ソースから取っている。
    #[test]
    #[ignore = ".conductor/ に索引を置いたリポジトリが要る"]
    fn real_index_answers_across_the_repository() {
        use crate::symbol_index::{code_identifiers_on_line, occurrence_span_in_source};

        let repo_root = std::env::var("CONDUCTOR_TEST_REPO").expect("CONDUCTOR_TEST_REPO");
        let repo_root = Path::new(&repo_root);
        let store = load(repo_root, repo_root).expect("索引と出自の申告が揃っている");

        let dirty = std::process::Command::new("git")
            .args([
                "-C",
                repo_root.to_str().unwrap(),
                "diff",
                "--name-only",
                "HEAD",
            ])
            .output()
            .unwrap();
        let dirty: Vec<String> = String::from_utf8_lossy(&dirty.stdout)
            .lines()
            .map(String::from)
            .collect();

        let (mut asked, mut exact, mut examples) = (0usize, 0usize, Vec::new());
        let (mut containers, mut named) = (0usize, Vec::new());
        let mut slowest = std::time::Duration::ZERO;
        for rel in [
            "src/repo_path.rs",
            "src/jump_history.rs",
            "src/background.rs",
        ] {
            let abs = repo_root.join(rel);
            let source = std::fs::read_to_string(&abs).unwrap();
            let mask = crate::symbol_index::CodeMask::compute(&source, rel);
            let index = crate::symbol_index::SymbolIndex::new(repo_root.to_path_buf());
            let bridge = Bridge {
                abs_path: &abs,
                source: &source,
                mask: &mask,
                index: &index,
            };
            for (line, text) in source.lines().enumerate() {
                for (k, _, word) in code_identifiers_on_line(text, line + 1, &mask) {
                    let Some((start, end)) = occurrence_span_in_source(text, k) else {
                        continue;
                    };
                    if text.get(start..end) != Some(word.as_str()) {
                        continue;
                    }
                    asked += 1;
                    let at = std::time::Instant::now();
                    let answer = sheaf_core::definition_at(
                        &store,
                        &bridge,
                        Path::new(rel),
                        line as u32,
                        start as u32,
                    );
                    slowest = slowest.max(at.elapsed());
                    if let sheaf_core::Definition::Exact(locations) = answer {
                        exact += 1;
                        if let Some(path) = sheaf_core::describe_at(
                            &store,
                            &bridge,
                            Path::new(rel),
                            line as u32,
                            start as u32,
                        )
                        .iter()
                        .find_map(|d| d.container.clone())
                        {
                            containers += 1;
                            if named.len() < 5 {
                                named.push(format!("  {word} <- {path}"));
                            }
                        }
                        for loc in &locations {
                            // 飛び先は必ずリポジトリ内の実在する行でなければならない。
                            let target = repo_root.join(&loc.path);
                            let text = std::fs::read_to_string(&target).unwrap_or_else(|_| {
                                panic!("飛び先が存在しない: {}", target.display())
                            });
                            assert!(
                                (loc.line as usize) < text.lines().count(),
                                "飛び先の行がファイルの外: {}:{}",
                                loc.path.display(),
                                loc.line
                            );
                        }
                        if examples.len() < 5 {
                            examples.push(format!("  {rel}:{line} {word} -> {:?}", locations[0]));
                        }
                    }
                }
            }
        }

        println!("汚れているファイル {} 件", dirty.len());
        println!("問い合わせ {asked} 箇所 / Exact {exact} / 最遅 1 クエリ {slowest:?}");
        println!("所属の綴りが出た {containers} 箇所");
        for n in &named {
            println!("{n}");
        }
        for e in &examples {
            println!("{e}");
        }
        assert!(exact > 0, "索引が1件も答えていない");
        assert!(
            slowest < std::time::Duration::from_millis(100),
            "1 クエリが gd の予算(100ms)を超えた: {slowest:?}"
        );
    }

    // 「向き先が違うツリーには答えない」の検査は sheaf 側 (`Slot`) にある。
    // 判定そのものがあちらにあるので、こちらに写しを置くと片方だけが古くなる。

    #[test]
    fn 読めない種別は見出しごと出さない() {
        // 種別を読めなかったときにそれらしい名前を返すと、ホバーが自信を持って嘘を出す。
        assert_eq!(kind_label(sheaf_core::SymbolKind::Unknown), "");
        assert_eq!(kind_label(sheaf_core::SymbolKind::Function), "fn");
        assert_eq!(kind_label(sheaf_core::SymbolKind::Variable), "let");
    }

    #[test]
    fn nested_paths_are_spelled_the_way_scip_spells_them() {
        // 表の鍵は SCIP の relative_path と突き合わせられる。綴りがずれると
        // 一致するファイルが 1 つも無くなり、全部が構文層に落ちる。誤答にはならないので
        // 気づけず、テストも緑のままになる。深い階層で綴りを固定しておく。
        let dir = tempfile::tempdir().unwrap();
        let hashes_path = dir.path().join("index.hashes");
        write_hashes(
            &hashes_path,
            &[
                ("src/deep/nested/lib.rs", "0".repeat(40)),
                ("top.rs", "1".repeat(40)),
            ],
        );

        let hashes = sheaf_core::read_provenance(&hashes_path, &*producer()).unwrap();

        let mut keys: Vec<_> = hashes.keys().map(|p| p.to_string_lossy()).collect();
        keys.sort();
        assert_eq!(keys, ["src/deep/nested/lib.rs", "top.rs"]);
    }

    /// `index.hashes` に書かれたハッシュが、そのまま(取り直さずに)期待ハッシュの表に
    /// 入ることを確かめる。
    #[test]
    fn expected_hashes_uses_the_recorded_hash_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let hashes_path = dir.path().join("index.hashes");
        let hash = sheaf_core::blob_hash(b"fn f() {}\n");
        write_hashes(&hashes_path, &[("src/lib.rs", hash.clone())]);

        let hashes = sheaf_core::read_provenance(&hashes_path, &*producer()).unwrap();
        assert_eq!(hashes.get(Path::new("src/lib.rs")), Some(&hash));
    }

    /// `load` が実際に返す `Store` の鮮度判定を、4 通りのファイルで見る。生きたリポジトリの
    /// Exact 率は汚れ具合で変わるため assert できないので、固定の一時リポジトリに
    /// 1 ファイルずつ用意して個別に検査する。
    #[test]
    fn provenance_table_governs_freshness_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // (a) 生成後に触っていないファイル。
        let untouched = "src/untouched.rs";
        // (b) コミットされていない(未追跡の)ファイル。ここが今日永久に落ちている箇所。
        let untracked = "src/untracked.rs";
        // (c) 生成後に編集されたファイル。
        let edited = "src/edited.rs";
        // (d) 生成の前後でハッシュが食い違うファイル。index.hashes に載らない。
        let racy = "src/racy.rs";

        std::fs::write(dir.path().join(CARGO_TOML.0), CARGO_TOML.1).unwrap();
        for rel in [untouched, untracked, edited, racy] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, SOURCE).unwrap();
        }

        // untouched と edited はコミットしておく。untracked は git に足さないままにして、
        // 出自の申告が git のトラッキング状態と無関係に効くことを確かめる。
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(untouched)).unwrap();
        index.add_path(Path::new(edited)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        write_index_for(
            &artifact(&conductor_dir, "scip"),
            &[untouched, untracked, edited, racy],
        );

        // 生成手順を模す。racy は前後でハッシュが食い違った体にして書かない。
        let hash = sheaf_core::blob_hash(SOURCE.as_bytes());
        write_hashes(
            &artifact(&conductor_dir, "hashes"),
            &[
                (untouched, hash.clone()),
                (untracked, hash.clone()),
                (edited, hash),
            ],
        );

        // edited は生成が終わった後に編集される。呼び出し箇所(1行目)は変えていないので、
        // クエリの単語自体は引き続き greet を指す。
        std::fs::write(
            dir.path().join(edited),
            "pub fn hello() {}\nfn caller() { greet(); }\n",
        )
        .unwrap();

        let store = load(dir.path(), dir.path()).expect("索引と出自の申告が揃っている");

        assert!(matches!(
            definition_of_greet_at(&store, dir.path(), untouched),
            sheaf_core::Definition::Exact(_)
        ));
        assert!(
            matches!(
                definition_of_greet_at(&store, dir.path(), untracked),
                sheaf_core::Definition::Exact(_)
            ),
            "未追跡でも index.hashes に載っていれば Exact になるはず(今日の欠陥の修正対象)"
        );
        assert!(!matches!(
            definition_of_greet_at(&store, dir.path(), edited),
            sheaf_core::Definition::Exact(_)
        ));
        assert!(!matches!(
            definition_of_greet_at(&store, dir.path(), racy),
            sheaf_core::Definition::Exact(_)
        ));
    }
}
