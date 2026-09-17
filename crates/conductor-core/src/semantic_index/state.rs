//! 索引の有無と、その作り直しの進み具合。
//!
//! ここにあるのは判断だけで、スレッドは持たない。ツリーを歩く調査は呼び出し側が背景で
//! 走らせ、結果を [`SemanticIndex::install`] に渡す。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sheaf_core::{Regenerated, Regenerator, Slot, Store};

use super::history::{self, Trigger};
use super::roots::{self, IndexRoot};
use super::{Reading, Survey, main_conductor_dir};

/// 終わった生成 1 世代。
///
/// `manual` を添えるのは、編集ごとの作り直しで status を埋めずに、手で頼んだものだけ
/// 結果を出すため。
pub struct Finished {
    pub outcome: Regenerated,
    pub manual: bool,
}

/// 索引ルート 1 本と、その作り直し係。
pub(super) struct Root {
    pub(super) at: IndexRoot,
    /// 調査した時点のツリーの内容から決まる鍵。成果物の名前に入る。
    ///
    /// `None` は「編集が入って、まだ引き直せていない」。鍵が古いまま生成すると、出来た索引が
    /// 読む側の探す名前と食い違うので、鍵の無いルートは生成を始めない。
    pub(super) key: Option<String>,
    pub(super) regenerator: Regenerator,
    /// 走っている 1 世代の顛末。記録に残すためだけに持つ。
    pub(super) run: Run,
}

impl Root {
    pub(super) fn is_working(&self) -> bool {
        self.regenerator.is_pending() || self.regenerator.is_running()
    }

    /// 既に待っている/走っている世代のきっかけは上書きしない (最初に頼んだものがその理由)。
    /// 手で頼まれたぶんだけは上書きする — 待っている世代を編集由来のまま置くと、
    /// tick_regeneration の「もうある」門が手動でないものとして弾いて何も起きない。
    pub(super) fn request(&mut self, trigger: Trigger, cause: Option<PathBuf>) {
        if !self.is_working() {
            self.run = Run::asked(trigger, cause);
        } else if trigger == Trigger::Manual {
            if self.regenerator.is_running() {
                self.run.next_trigger = Some(Trigger::Manual);
                // 押した人の文脈を優先する。走行中の編集の cause で上書きすると、
                // 手動の行が「押したとき読んでいたファイル」を指さなくなる。
                self.run.next_cause = cause.or_else(|| self.run.next_cause.take());
            } else {
                // 待っているだけなら、この要求のものとして数え直す。待ち時間を最初の
                // 編集から数えると、押した本人が待っていない時間が手動の行に乗る。
                self.run.trigger = Some(Trigger::Manual);
                self.run.cause = cause;
                self.run.asked_at = Some(Instant::now());
            }
        }
        match trigger {
            Trigger::Manual => self.regenerator.request_now(),
            _ => self.regenerator.request(),
        }
    }

    fn record(&mut self, dir: &Path, key: &str, outcome: &Regenerated) {
        // 生成に至らなかったものは、比べる相手がそもそも入れ替わっていない。
        let sources = match outcome {
            Regenerated::Ready { .. } => {
                history::source_delta(self.run.before.as_ref(), &self.at, dir, key)
            }
            _ => history::Sources::Unknown,
        };
        self.log_with(dir, sources, outcome.into());
        // 生成中に変更が来た場合とロックを取れなかった場合、sheaf は待機に戻してもう一度
        // 走らせる。その世代のきっかけを引き継がないと、記録が「きっかけ不明・待ち時間 0 秒」
        // になる。
        self.run = if self.regenerator.is_pending() {
            // 走っている間に来た要求はここで初めて自分の世代を持つ。済んだきっかけは
            // 引き継がない — 手動を残すと、以後どの世代も「もうある」門を素通りして、
            // 誰も押していないのに producer が走り続ける。
            let unserved = matches!(outcome, Regenerated::Busy);
            let trigger = self
                .run
                .next_trigger
                .take()
                .or_else(|| unserved.then_some(self.run.trigger).flatten())
                .unwrap_or(Trigger::Change);
            let cause = self
                .run
                .next_cause
                .take()
                .or_else(|| self.run.cause.clone());
            Run::asked(trigger, cause)
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
pub(super) struct Run {
    pub(super) trigger: Option<Trigger>,
    cause: Option<PathBuf>,
    asked_at: Option<Instant>,
    started_at: Option<Instant>,
    /// producer が立つ直前の出自の表。この生成に意味があったかを言うために取っておく。
    before: Option<HashMap<PathBuf, String>>,
    /// 走っている間に変わったファイル。空でなければ、その索引は置いた時点で既に古い。
    /// 件数ではなくファイルで持つ — 監視は 1 回の保存で複数のイベントを上げるので、生の
    /// 件数を出すと編集 2 回が 6 に見える。
    changed_during: HashSet<PathBuf>,
    next_cause: Option<PathBuf>,
    /// 走っている間に来た要求のきっかけ。走っている世代はそれで始まったものではないので、
    /// 今の記録には混ぜず次へ回す。
    next_trigger: Option<Trigger>,
    /// producer を起こしたときの鍵。走っている間に編集が入ると `Root::key` は落ちるが、
    /// 置かれる成果物の名前はこちらのまま。
    pub(super) key: Option<String>,
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

    fn waited(&self) -> Duration {
        match (self.asked_at, self.started_at) {
            (Some(a), Some(s)) => s.duration_since(a),
            (Some(a), None) => a.elapsed(),
            _ => Duration::ZERO,
        }
    }

    fn took(&self) -> Duration {
        self.started_at.map(|s| s.elapsed()).unwrap_or_default()
    }
}

/// 索引 ([`Store`]) の有無と、その作り直しを保持する。
///
/// `Store` は 1 つで、ツリーの中の索引ルートすべてを束ねたもの。作り直しはルートごとに独立
/// して走る (道具も成果物も別なので、片方の失敗をもう片方に伝播させない)。
#[derive(Default)]
pub struct SemanticIndex {
    slot: Slot,
    /// いま列挙してある索引ルートと、その列挙元のツリー。ツリーが変われば引き直す。
    pub(super) roots: Vec<Root>,
    pub(super) tree: PathBuf,
    /// いま読んでいるファイル。同じものを繰り返し告げられたときに何もしないため。
    reading: Option<PathBuf>,
    /// 読んでいるファイルの索引ルートが、まだ調査に載っていない。
    unsurveyed_reading: bool,
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
    /// 1 本作らせる。索引ルートは実在するリポジトリで 109 本になることがあり、まとめて作ると
    /// 数十分かかるので、読むところから順に作る (全部なら `conductor index`)。
    ///
    /// 毎フレーム呼ばれる前提で、前回と同じファイルなら即座に返る。開く経路が 12 箇所あるので、
    /// そのどこかを通し忘れるより毎周見るほうが落ちない。
    pub fn note_open(&mut self, rel: &Path, repo_root: &Path, tree_root: &Path) -> Reading {
        // ツリーが動いていれば、いまの索引ルートは前のツリーのもの。調査が届くまで答えを
        // 確定させない。確定させると、同じ相対パスのファイルを開いたまま worktree を
        // 切り替えたときに前のツリーの答えが残る。
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
            // 調査が鍵を出すのは成果物の置いてあるルートだけなので、一度も索引されていない
            // ルートはここに来る。頼まないと鍵を持てないまま生成も始まらず、そのルートは
            // 永久に構文層のままになる。
            self.unsurveyed_reading = roots::Language::of_file(rel).is_some();
            return settle(self, Reading::NotIndexed);
        };
        let Some(dir) = self.conductor_dir(repo_root).map(Path::to_path_buf) else {
            return settle(self, Reading::NotIndexed);
        };
        let Some(key) = self.roots[index].key.clone() else {
            return Reading::Loading;
        };
        let indexed = self.roots[index].at.has_generation(&dir, &key);
        // 読み込みは別スレッドなので、worktree を切り替えた直後は間に合っていない。ここで
        // 確定させると、古いと言うべき唯一の場面で必ず黙る。読むものが置いてすら無いときは
        // 待つ相手がいないので、そのまま作りに行く。
        let covered = match self.slot.get(tree_root) {
            Some(store) => store.is_current(rel),
            None if indexed => return Reading::Loading,
            None => false,
        };
        // 出自はファイル単位なので、ほかのファイルが動いて鍵がずれても、読んでいるファイルは
        // 前の世代のまま Exact に答える。鍵だけを見て走らせると、git がツリーを動かすたびに
        // producer を丸ごと起こすことになる。
        if covered {
            return settle(self, Reading::Indexed);
        }
        if self.roots[index].is_working() {
            return settle(self, Reading::Building);
        }
        if indexed {
            return settle(self, Reading::Stale);
        }
        self.roots[index].request(Trigger::Open, Some(rel.to_path_buf()));
        settle(self, Reading::Building)
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
        self.roots[index].request(Trigger::Manual, Some(rel));
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
        // gitignore は歩く対象を変えるので、載る言語が無くても鍵が動く。動くのは、それを
        // 含むルートと、その下にあるルートだけ。歩かれない場所のもの (venv/.gitignore など)
        // では動かない。生成は頼まない — 次の調査が鍵を出し直せば済む。
        if rel.file_name() == Some(std::ffi::OsStr::new(".gitignore")) {
            let at = rel.parent().unwrap_or(Path::new("")).to_path_buf();
            let dir = tree_root.join(&at);
            for i in 0..self.roots.len() {
                let subroot = self.roots[i].at.subroot.clone();
                // ルートより上にあるものはルート全体の歩き方を決めるので、そのまま効く。
                // 中にあるものは、その場所自体が歩かれているときだけ効く。
                let hits = if subroot.starts_with(&at) {
                    true
                } else {
                    let root_at = tree_root.join(&subroot);
                    at.starts_with(&subroot) && self.roots[i].regenerator.walks(&dir, &root_at)
                };
                if hits {
                    self.roots[i].key = None;
                }
            }
            return;
        }
        let Some(index) = self.owning_root(rel) else {
            return;
        };
        let at = tree_root.join(&self.roots[index].at.subroot);
        let root = &mut self.roots[index];
        let was_working = root.is_working();
        // 索引に載らないファイルでは鍵も動かない。落とすと target/ の書き換えのたびに
        // 調査が要る状態になり、ビルド中はツリー走査が回り続ける。
        if !root.regenerator.note_change(changed, &at) {
            return;
        }
        // 内容が動いたので鍵も動く。生成を始めない理由は Root::key。
        root.key = None;
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
        roots::owning_index(self.roots.iter().map(|r| &r.at), rel)
    }

    /// 作り直しを 1 周進める。毎フレーム呼ばれる。待つものが無いうちに置き場所を組み立て
    /// ないのは、そこに git2 のリポジトリオープンが要るため。
    pub fn tick_regeneration(&mut self, repo_root: &Path, tree_root: &Path) -> Option<Finished> {
        if !self.roots.iter().any(|r| r.is_working()) {
            return None;
        }
        let dir = self.conductor_dir(repo_root)?.to_path_buf();
        // 調査が届くまでは前のツリーの鍵しか持っていない。その鍵で起こすと、動いた先の
        // 内容が古い名前で置かれ、戻ったときに「もうある」で止まって自動では治らない。
        let settled = self.tree == tree_root;
        // 1 周で返すのは 1 本ぶん。生成はロックで直列化されているので、同じ周に 2 本が
        // 終わることはほとんど無い。
        self.roots.iter_mut().find_map(|root| {
            // 鍵が要るのは producer を起こすときだけ。走っているぶんまで止めると、生成中の
            // 編集で鍵が落ちたルートが顛末を拾えず、記録も掃除も済まないまま二度と終われない。
            let key = root.key.clone().or_else(|| root.run.key.clone())?;
            if !settled && !root.regenerator.is_running() {
                return None;
            }
            // この内容の索引はもう置いてある。producer を起こしても同じものが出るし、編集で
            // 行ったり来たりするたびに丸ごと 1 本ぶん払うことになる。手で頼まれたときは
            // 通さない — 押した人は「もうある」ではなく作り直しを待っている。
            if root.regenerator.is_pending()
                && root.run.trigger != Some(Trigger::Manual)
                && root.at.has_generation(&dir, &key)
            {
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
                root.run.key = Some(key.clone());
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

    /// producer が実際に走っているか。静穏時間の待ちは含めない。
    pub fn is_generating(&self) -> bool {
        self.roots.iter().any(|r| r.regenerator.is_running())
    }

    /// 索引ルートの調査が要るか。要るなら、鍵を出しておくべきルート。
    ///
    /// ツリーを歩く重さは [`super::survey`]。名指しで返すのは、鍵を出す相手を調査が自分で
    /// 選ぶためで、選から漏れたルートはいつまでも「調査が要る」と言い続けることになる。
    pub fn needs_survey(&self, tree_root: &Path) -> Option<Vec<IndexRoot>> {
        if self.tree != tree_root || self.unsurveyed_reading {
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
            // 走っている生成は前のツリーを索引している。Regenerator を捨てれば Drop が
            // プロセスグループごと止める。
            self.roots.clear();
        }
        // 載らなかったなら、そのファイルはどの索引ルートにも属さない。立てたままだと調査が
        // 毎フレーム走る。
        self.unsurveyed_reading = false;
        // 目印が消えたルートは調べ直しても鍵が出ない。残すと needs_survey が毎フレーム
        // そのルートを名指しし続ける。落としすぎても、読み直せば対象外として調べ直しの
        // 口が開く (下の None 分岐と同じ)。
        let before = self.roots.len();
        // 走っているものは残す。落とすと Drop が producer を殺し、その世代は記録に
        // 1 行も残らない。本当に消えたなら走り終えた次の調査で落ちる。
        self.roots
            .retain(|root| survey.found.contains(&root.at) || root.regenerator.is_running());
        if self.roots.len() != before {
            self.reading = None;
        }
        for (at, key) in survey.roots {
            match self.roots.iter_mut().find(|r| r.at == at) {
                // 走っている生成はそのまま続ける。鍵だけ差し替えると、置かれる索引の名前と
                // 中身が食い違うので、走っていない間にだけ入れる。
                Some(root) if !root.regenerator.is_running() => root.key = Some(key),
                Some(_) => {}
                None => {
                    // 「対象外」と答えたファイルがこのルートのものかもしれない。読み直させ
                    // ないと、答えが確定したまま生成が始まらない。
                    self.reading = None;
                    self.roots.push(Root {
                        regenerator: Regenerator::new(at.lang.producer()),
                        at,
                        key: Some(key),
                        run: Run::default(),
                    });
                }
            }
        }
    }

    pub(super) fn conductor_dir(&mut self, repo_root: &Path) -> Option<&Path> {
        if self
            .conductor_dir
            .as_ref()
            .is_none_or(|(at, _)| at != repo_root)
        {
            self.conductor_dir = Some((repo_root.to_path_buf(), main_conductor_dir(repo_root)));
        }
        self.conductor_dir.as_ref()?.1.as_deref()
    }

    /// 走っている生成を止める。止めた時点までの producer の時間は捨てるので、記録に残す。
    /// 待っているだけの世代も捨てる — 静穏が明けるのは切り替えた後で、そのとき組む
    /// 置き場所は新しいツリーのものになる。
    ///
    /// 呼ぶのはツリーが動くときだけ。動かないなら結果はそのまま使えるので、止めると
    /// 誰も頼み直さないまま生成が消える。
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

    /// 背景ロードの結果を取り込む。取り込まなかったときは `false` を返すので、呼び出し側は
    /// それを見て読み直しを起こす。
    pub fn accept(&mut self, requested: &Path, current: &Path, store: Option<Store>) -> bool {
        self.slot.accept(requested, current, store)
    }

    /// 静穏を待っている生成があるか。producer はまだ立っていない。
    pub fn is_pending(&self) -> bool {
        self.roots.iter().any(|r| r.regenerator.is_pending())
    }

    /// 検査が生成中の状態を組み立てるための入口。
    #[cfg(test)]
    pub(super) fn with_root(
        tree: &Path,
        at: IndexRoot,
        regenerator: Regenerator,
        key: &str,
    ) -> Self {
        SemanticIndex {
            tree: tree.to_path_buf(),
            roots: vec![Root {
                at,
                regenerator,
                key: Some(key.to_string()),
                run: Run::default(),
            }],
            ..Default::default()
        }
    }
}
