//! 索引を作り直す。
//!
//! producer は起動してからしばらくソースを読み続ける (rust-analyzer は実測で
//! 起動から約 0.6〜1.0 秒。境界は実行ごとに動く)。その窓に入った編集は索引に
//! 焼き込まれるのに、成功として返ってくる。だから出自のハッシュは生成の前後で
//! 2 回取り、両方一致したファイルだけを申告する。

mod job;
mod producer;
mod provenance;
#[cfg(test)]
mod tests;

pub use producer::{Producer, RustAnalyzer, ScipGo, ScipTypescript};
pub use provenance::{read_provenance, write_provenance};

use crate::Store;
use job::{Job, kill_group};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// いま何時か。静穏時間と producer の上限を測るのに使う。
///
/// 検査だけが差し替える。実時計のままだと、静穏時間を待つ検査が実際に 3 秒眠り、
/// 上限の検査は producer が死ぬまで待つことになる。
#[derive(Clone)]
struct Clock(Arc<dyn Fn() -> Instant + Send + Sync>);

impl Clock {
    fn now(&self) -> Instant {
        (self.0)()
    }

    fn since(&self, past: Instant) -> Duration {
        self.now().saturating_duration_since(past)
    }

    #[cfg(test)]
    fn reading(at: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Clock(Arc::new(at))
    }
}

impl Default for Clock {
    fn default() -> Self {
        Clock(Arc::new(Instant::now))
    }
}

/// 変更が止まったとみなすまでの静穏時間。
///
/// 編集の最中に始めても、producer が読み終える前の内容で索引ができる。
/// 手が止まるまで待つほうが、始めて捨てるより安い。
const QUIESCENCE: Duration = Duration::from_secs(3);

/// 索引を作る対象と、成果物の置き場所。
#[derive(Clone)]
pub struct Target {
    /// 索引を作るソースツリーのルート。
    pub root: PathBuf,
    /// 完成した索引の置き場所。
    pub index: PathBuf,
    /// 出自の表の置き場所。
    pub hashes: PathBuf,
    /// producer の出力を捨てる先。組み込む側が端末を持っていることがあるので継承させない。
    pub log: PathBuf,
    /// 生成の本数を数えるロックの置き場所。
    ///
    /// 索引の置き場所から導かない。導くと、索引を言語やツリーごとに分けた瞬間に
    /// ロックも分かれ、上限が黙って消える。producer 1 本のピークが 2.3GiB なので、
    /// 同時に立つ本数はリポジトリ単位で抑える必要がある。
    pub lock: PathBuf,
}

/// [`generate_once`] が返すもの。作った索引そのものが付く。
///
/// Ready だけが大きいが、この enum が動くのは生成 1 回につき 1 度チャネルを
/// 渡るときだけ。箱に入れると確保が 1 つ増え、受け取る側が外す手間も増える。
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    /// 新しい索引を投入できた。
    Ready { store: Store },
    /// 生成に失敗した。古い索引はそのまま使い続ける。
    Failed(String),
    /// ほかが索引を作っていてロックを取れなかった。
    ///
    /// [`Failed`](Outcome::Failed) と分けてある。失敗と同じ扱いにすると待機に戻らず、
    /// 次にそのツリーへ編集が入るまで索引されないままになる。索引ルートが複数あって
    /// 同時に変わる（ブランチ切替など）と、負けたルートがそのまま取り残される。
    Busy,
    /// producer を起動できなかった。以後試みない。
    Unavailable(String),
}

/// [`Regenerator::tick`] が返すもの。[`Outcome`] と違って索引そのものは付かない。
///
/// 1 世代が作るのは索引ルート 1 本ぶんで、読み手が引くのは全ルートを畳んだもの。1 本ぶんの
/// [`Store`] をそのまま投入すると他のルートの索引が黙って落ちるので、型の側で渡さない。
/// 成果物はディスクにあるので、受け取った側は読み直せばよい。
pub enum Regenerated {
    /// 新しい索引を置いた。読み直すと反映される。
    Ready {
        documents: usize,
    },
    Failed(String),
    Busy,
    Unavailable(String),
}

impl From<Outcome> for Regenerated {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Ready { store } => Regenerated::Ready {
                documents: store.len(),
            },
            Outcome::Failed(why) => Regenerated::Failed(why),
            Outcome::Busy => Regenerated::Busy,
            Outcome::Unavailable(why) => Regenerated::Unavailable(why),
        }
    }
}

enum State {
    Idle,
    /// 変更を受けた。`last_change` から [`QUIESCENCE`] 静かなら生成を始める。
    Pending {
        last_change: Instant,
    },
    /// 待つ理由が無い。次の [`Regenerator::tick`] で始める。
    Due,
    Running,
}

/// 索引を作り直す係。
///
/// 組み込む側は「変わった」と「1 周進めて」を伝えるだけでよい。
pub struct Regenerator {
    producer: Arc<dyn Producer>,
    state: State,
    rx: Option<mpsc::Receiver<Outcome>>,
    cancel: Arc<AtomicBool>,
    /// 走っている子プロセスのプロセスグループ。
    ///
    /// producer は自分でも子を持つ (rust-analyzer なら proc-macro の展開) ので、
    /// プロセス 1 つを止めても孫が残る。終了時にワーカースレッドを待てない
    /// 組み込み方があるので、ここから直接グループごと止められるようにしている。
    group: Arc<Mutex<Option<i32>>>,
    disabled: bool,
    /// 生成中に来た変更。走り終えてから 1 世代にまとめて作り直す。
    restart: bool,
    /// 引き金を数えるかどうかの判定に使う。ツリーごとに 1 回だけ作る。
    ignores: Option<(PathBuf, ignore::gitignore::Gitignore)>,
    clock: Clock,
}

impl Default for Regenerator {
    fn default() -> Self {
        Regenerator::new(Arc::new(RustAnalyzer))
    }
}

impl Regenerator {
    pub fn new(producer: Arc<dyn Producer>) -> Self {
        Regenerator {
            producer,
            state: State::Idle,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            group: Arc::new(Mutex::new(None)),
            disabled: false,
            restart: false,
            ignores: None,
            clock: Clock::default(),
        }
    }

    #[cfg(test)]
    fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// 生成が走っているか。
    pub fn is_running(&self) -> bool {
        matches!(self.state, State::Running)
    }

    /// 始まるのを待っているか。静穏時間の計測中か、次の tick で始まる状態。
    pub fn is_pending(&self) -> bool {
        matches!(self.state, State::Pending { .. } | State::Due)
    }

    /// 変更を待たずに 1 本作らせる。索引がまだ 1 本も無いときの口。
    ///
    /// 静穏時間は変更のときと同じに置く。組み込む側が起動直後に呼ぶので、すぐ始めると
    /// 起動と生成 (実測 2.3GiB) が重なる。走っている最中と、producer を起動できないと
    /// 分かったあとは何もしない。
    pub fn request(&mut self) {
        if self.disabled || matches!(self.state, State::Running) {
            return;
        }
        self.state = State::Pending {
            last_change: self.clock.now(),
        };
    }

    /// 静穏時間を待たずに始める。手で頼まれたぶんの口。[`request`](Self::request) が待つのは
    /// 起動と生成が重なるのを避けるためで、押した本人が結果を待つ場面には当たらない。
    pub fn request_now(&mut self) {
        if self.disabled || matches!(self.state, State::Running) {
            return;
        }
        self.state = State::Due;
    }

    /// `root` のツリーの中でファイルが変わった。
    ///
    /// 索引に載らないファイルは数えない。ビルド成果物は数秒おきに書き換わるので、数えると
    /// 静穏時間が永久に来ない。生成中に来たぶんは、走り終えてから 1 世代にまとめて作り直す。
    pub fn note_change(&mut self, changed: &Path, root: &Path) {
        if self.disabled || root.as_os_str().is_empty() || !changed.starts_with(root) {
            return;
        }
        if self.is_ignored(changed, root) {
            return;
        }
        match self.state {
            State::Running => self.restart = true,
            // 手で頼まれたぶんを追い越さない。待たせると押した本人が待つことになる。
            State::Due => {}
            _ => {
                self.state = State::Pending {
                    last_change: self.clock.now(),
                }
            }
        }
    }

    /// 出自の対象にならないファイルか。読むのは root 直下の `.gitignore` 1 枚だけで、
    /// [`snapshot`] はサブディレクトリの `.gitignore` も読む。ずれる向きは
    /// 「無駄に作り直す」側だけなので、答えは変わらない。
    fn is_ignored(&mut self, changed: &Path, root: &Path) -> bool {
        let Ok(rel) = changed.strip_prefix(root) else {
            return true;
        };
        if rel
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            return true;
        }
        if self.ignores.as_ref().is_none_or(|(at, _)| at != root) {
            let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
            builder.add(root.join(".gitignore"));
            let Ok(built) = builder.build() else {
                return false;
            };
            self.ignores = Some((root.to_path_buf(), built));
        }
        let (_, matcher) = self.ignores.as_ref().expect("直前に入れた");
        matcher.matched_path_or_any_parents(rel, false).is_ignore()
    }

    /// 走っている生成を止める。対象のツリーが変わったときと、終了するとき。
    pub fn abort(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        kill_group(&self.group);
        self.rx = None;
        self.restart = false;
        self.state = State::Idle;
    }

    /// 毎周呼ぶ。始めどきなら始め、終わっていれば結果を返す。
    ///
    /// ここでやるのはチャネルを覗くのと時刻の比較だけ。索引の読み込みと
    /// パース (実測 67ms) はワーカースレッドの側に置いてある。
    pub fn tick(&mut self, target: &Target) -> Option<Regenerated> {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(outcome) => {
                    self.rx = None;
                    // 走っている間に来た変更は、この世代の索引には入っていない。
                    let changed_while_running = std::mem::take(&mut self.restart);
                    // ロックを取れなかっただけなら、対象は変わっていないのに索引だけが
                    // 古いままになる。待機に戻して静穏時間のあとにやり直さないと、
                    // 次にそのツリーへ編集が入るまで索引されない。
                    let busy = matches!(outcome, Outcome::Busy);
                    self.state = if changed_while_running || busy {
                        State::Pending {
                            last_change: self.clock.now(),
                        }
                    } else {
                        State::Idle
                    };
                    if let Outcome::Unavailable(_) = outcome {
                        self.disabled = true;
                    }
                    return Some(outcome.into());
                }
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.state = State::Idle;
                }
            }
        }
        let due = match self.state {
            State::Pending { last_change } => self.clock.since(last_change) >= QUIESCENCE,
            State::Due => true,
            _ => false,
        };
        if due {
            self.start(target);
        }
        None
    }

    fn start(&mut self, target: &Target) {
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.group = Arc::new(Mutex::new(None));
        let job = Job {
            target: target.clone(),
            producer: Arc::clone(&self.producer),
            cancel: Arc::clone(&self.cancel),
            group: Arc::clone(&self.group),
            clock: self.clock.clone(),
        };
        std::thread::spawn(move || {
            let _ = tx.send(job.run());
        });
        self.rx = Some(rx);
        self.state = State::Running;
    }
}

impl Drop for Regenerator {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        kill_group(&self.group);
    }
}

/// 索引を 1 本、その場で作って置く。
///
/// 背景の作り直しと同じ経路を通す。出自の採取をここ以外に書くと、
/// 手で作った索引と背景で作った索引で申告の規則が食い違う。
pub fn generate_once(target: Target, producer: Arc<dyn Producer>) -> Outcome {
    Job {
        target,
        producer,
        cancel: Arc::new(AtomicBool::new(false)),
        group: Arc::new(Mutex::new(None)),
        clock: Clock::default(),
    }
    .run()
}
