//! 索引を 1 本作る仕事。子プロセスの起動・寿命・強制終了と、成果物の置き換え。

use super::provenance::{snapshot, unchanged, write_provenance};
use super::{Outcome, Producer, Target};
use crate::Store;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 子プロセスの終了・キャンセル・タイムアウトを確認する間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 生成そのものの失敗。
///
/// [`Outcome`] をそのまま使うと、成功を表す `Ready` を失敗側に置ける形になる。
enum Failure {
    Failed(String),
    Unavailable(String),
}

impl From<Failure> for Outcome {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Failed(why) => Outcome::Failed(why),
            Failure::Unavailable(why) => Outcome::Unavailable(why),
        }
    }
}

pub(super) struct Job {
    pub(super) target: Target,
    pub(super) producer: Arc<dyn Producer>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) group: Arc<Mutex<Option<i32>>>,
}

impl Job {
    pub(super) fn run(self) -> Outcome {
        let _lock = match Lock::acquire(&self.target.lock) {
            Ok(Some(lock)) => lock,
            Ok(None) => return Outcome::Busy,
            Err(e) => {
                return Outcome::Failed(format!(
                    "ロックを置けない ({}): {e}",
                    self.target.lock.display()
                ));
            }
        };

        // 索引の置き場所は、同じリポジトリを開いた別のプロセスと共有していることがある。
        // 一時ファイルまで共有すると、片方が rename したあとも もう片方の fd が
        // 同じ実体を指し続けるので、生きた索引を上書きしてしまう。
        let tmp = TempIndex::new(
            self.target
                .index
                .with_extension(format!("scip.{}.tmp", std::process::id())),
        );

        let before = snapshot(&self.target.root);

        if let Err(failure) = self.spawn_and_wait(tmp.path()) {
            return failure.into();
        }

        let after = snapshot(&self.target.root);
        let expected = unchanged(&before, &after);

        // 1 回の生成が作るのは索引 1 本で、その索引ルートは対象そのもの。
        // 複数の索引ルートをまとめるのは、どれを投入するかを決める呼び出し側の仕事。
        let source = crate::IndexSource {
            index: tmp.path().to_path_buf(),
            subroot: PathBuf::new(),
            expected,
        };
        let store = match Store::load(std::slice::from_ref(&source), &self.target.root) {
            Ok(store) => store,
            Err(e) => return Outcome::Failed(format!("索引を投入できない: {e}")),
        };

        // go.mod が見つからないなど、対象を認識できない producer は終了コード 0 で
        // Document 0 件の索引を書くことがある。exit status しか見ないここまでの
        // 検査ではこれを正常な生成と区別できず、放置すると空の索引が古い索引を
        // 上書きして、以後のクエリが無言で構文層に落ち続ける。
        if store.is_empty() {
            return Outcome::Failed("producer が空の索引を書いた（Document 0 件）".into());
        }

        // 子プロセスが終わったあとに対象のツリーを切り替えられる。ここで見ないと、
        // 見ていないツリーの索引を置き場所に書き込んでしまう。
        // 窓は投入にかかる 100ms ほどしかなく、そこへ中止を差し込むテストは
        // 決定的に書けないので、この 4 行はテストに支えられていない。
        if self.cancel.load(Ordering::Relaxed) {
            return Outcome::Failed("中止した".into());
        }

        // 索引を先に置き、出自を後に書く。この 2 つを原子的に一緒には置けないので、
        // 間で落ちたときに安全側へ倒れる順を選ぶ。逆順だと、古い索引に新しい出自が
        // 付いた状態が残り、変わっていないファイルについて古い行番号を Exact として返す。
        if let Err(e) = tmp.commit(&self.target.index) {
            return Outcome::Failed(format!("索引を置けない: {e}"));
        }
        if let Err(e) = write_provenance(&self.target.hashes, &*self.producer, &source.expected) {
            return Outcome::Failed(format!("出自を書けない: {e}"));
        }

        Outcome::Ready { store }
    }

    fn spawn_and_wait(&self, out: &Path) -> Result<(), Failure> {
        let argv = self.producer.command(out);
        let Some((program, args)) = argv.split_first() else {
            return Err(Failure::Unavailable("producer が空".into()));
        };
        let log = std::fs::File::create(&self.target.log)
            .map_err(|e| Failure::Failed(format!("ログを開けない: {e}")))?;
        let errlog = log
            .try_clone()
            .map_err(|e| Failure::Failed(format!("ログを開けない: {e}")))?;

        use std::os::unix::process::CommandExt;

        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&self.target.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errlog));
        // 孫まで一度に止められるよう、子を新しいプロセスグループの長にする。
        command.process_group(0);

        let mut child = command
            .spawn()
            .map_err(|e| Failure::Unavailable(format!("{program} を起動できない: {e}")))?;
        if let Ok(mut slot) = self.group.lock() {
            *slot = Some(child.id() as i32);
        }

        let start = Instant::now();
        let status = loop {
            if self.cancel.load(Ordering::Relaxed) {
                kill_group(&self.group);
                let _ = child.wait();
                return Err(Failure::Failed("中止した".into()));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() >= self.producer.timeout() {
                        kill_group(&self.group);
                        let _ = child.wait();
                        return Err(Failure::Failed(format!(
                            "{program} が {:?} で終わらない",
                            self.producer.timeout()
                        )));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => {
                    // 待てないまま返すと、次の生成で group が差し替わって
                    // このプロセスを誰も止められなくなる。
                    kill_group(&self.group);
                    return Err(Failure::Failed(format!("待てない: {e}")));
                }
            }
        };
        if let Ok(mut slot) = self.group.lock() {
            *slot = None;
        }

        if !status.success() {
            return Err(Failure::Failed(format!(
                "{program} が異常終了した ({status})。詳細は {}",
                self.target.log.display()
            )));
        }
        Ok(())
    }
}

/// 生成中の一時ファイル。
///
/// 投入までに失敗しうる経路が 5 つあり、そのどれかで消し忘れると、置き場所に
/// PID つきの残骸が積もる。経路が増えても消し忘れないよう、片づけを Drop に持たせる。
struct TempIndex(Option<PathBuf>);

impl TempIndex {
    /// 前回の残骸を投入してしまわないよう、書き始める前に消す。producer が
    /// 成功を返しながら出力を書かなかった場合、古い索引を新しい出自と一緒に
    /// 載せることになる。
    fn new(path: PathBuf) -> Self {
        let _ = std::fs::remove_file(&path);
        TempIndex(Some(path))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("commit したあとは触らない")
    }

    /// 置き場所へ移す。移せたら消さない（それが成果物になる）。移せなければ消す。
    fn commit(mut self, to: &Path) -> std::io::Result<()> {
        let from = self.0.take().expect("commit は 1 度だけ");
        std::fs::rename(&from, to).inspect_err(|_| {
            let _ = std::fs::remove_file(&from);
        })
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 索引の生成をプロセスをまたいで直列化する助言ロック。
///
/// 同じリポジトリを別のプロセスから開くと、生成の置き場所が同じになる。
/// 上限を 1 本に保つ理由はメモリで、生成 1 本がピーク 2.3GiB を使うため、
/// プロセスの数だけ並ぶと本数に比例して積み上がる。
pub(super) struct Lock(std::fs::File);

impl Lock {
    /// 取れなければ諦める。待たないのは、待っている間に対象のツリーが
    /// 変わってしまい、待った先で作るものが古くなるため。次の変更で作り直す。
    ///
    /// **「ほかが持っている」と「置き場所が使えない」を分ける。** 混ぜると、
    /// 置き場所のディレクトリがまだ無いだけのときに「ほかのプロセスが索引を
    /// 作っている」と報告することになる (索引を一度も作っていないリポジトリが
    /// まさにその状態で、`conductor index` が必ずそう答えていた)。
    pub(super) fn acquire(path: &Path) -> std::io::Result<Option<Self>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::create(path)?;
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
        Ok((unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock(file)))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&self.0);
        unsafe { libc::flock(fd, libc::LOCK_UN) };
    }
}

pub(super) fn kill_group(group: &Arc<Mutex<Option<i32>>>) {
    let Ok(mut slot) = group.lock() else { return };
    if let Some(pgid) = slot.take() {
        unsafe { libc::killpg(pgid, libc::SIGKILL) };
    }
}
