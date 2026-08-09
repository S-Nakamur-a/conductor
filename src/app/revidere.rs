//! [App] における revidere の駆動: 成果物の読み直しと、解析の起動。
//!
//! 解析は `conductor revidere analyze` を子プロセスとして起こす。実装は
//! crates/revidere-cli にあり、同じバイナリに入っている。どの AI を使うかは
//! revidere の `[ai] command` に委ねたままなので、「conductor はどのモデルが
//! 答えるかを決めない」という規則はそのまま。委譲先が 1 段増えているだけ。
//!
//! 見るのは常に作業ツリー (`--head worktree`)
//!
//! レビューしたいものは大抵まだコミットされていない。merge-base 固定だと、
//! いま手元で書いているものが画面に出るまで一度コミットしなければならない。
//! 出力先も既定のまま (`<worktree>/.revidere/review.json`) にしてある —
//! conductor の worktree はそれぞれ別ディレクトリなので、ブランチごとに
//! 自然に分かれる。

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use super::*;

/// 子プロセスの終了を待つ間のポーリング間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// 解析 1 回の実時間の上限。revidere 自身にも `[ai] timeout_secs` があるが、
/// そちらが効かない壊れ方 (子プロセスが応答しない) でも conductor 側が
/// 諦められるようにしておく。
const RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// 解析が終わったときの結果。
pub enum RunOutcome {
    /// 成果物ができた。coverage_complete が false なら、成果物はあるが
    /// 充足検査は通っていない (revidere の終了コード 2)。
    Done { coverage_complete: bool },
    /// 走らせられなかった / 途中で落ちた。
    Failed(String),
}

/// 実行中の解析 1 本。
pub struct RevidereRun {
    pub branch: String,
    result: Receiver<RunOutcome>,
    cancel: Arc<AtomicBool>,
}

impl RevidereRun {
    /// ワーカーに停止を通知する。ワーカーは子プロセスをポーリングする合間に
    /// これを見るので、revidere とその先の AI コマンドは確実に kill される。
    fn abort(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 実行を終えた解析。対象ブランチを保持しているのは、呼び出し側がメッセージを
/// 組み立てる時点でハンドル自体はもう無いため。
pub struct FinishedRun {
    pub branch: String,
    pub outcome: RunOutcome,
}

/// この conductor インスタンスで実行中の全解析。ブランチごとに高々 1 本。
///
/// 「同時に 1 本まで」ではなくブランチをキーにしているのは、解析が実際に
/// 競合する対象がブランチだけだからである。成果物の置き場は worktree ごとに
/// 分かれているので、別のブランチの解析は別のファイルに書く。すべてを直列化
/// すると、ある worktree を見ているレビュアーが別の worktree の解析を
/// 始められなくなる。
#[derive(Default)]
pub struct RevidereRuns {
    by_branch: HashMap<String, RevidereRun>,
}

impl RevidereRuns {
    /// branch の解析が現在実行中かどうか。
    pub fn is_running(&self, branch: &str) -> bool {
        self.by_branch.contains_key(branch)
    }

    /// 起動したばかりの解析を登録する。呼び出し側は事前に [Self::is_running]
    /// を確認している前提。稼働中のハンドルに上書き挿入すると receiver が
    /// drop され、ワーカーの結果が行き場を失う。
    pub fn insert(&mut self, run: RevidereRun) {
        debug_assert!(
            !self.is_running(&run.branch),
            "would orphan the in-flight run for {}",
            run.branch
        );
        self.by_branch.insert(run.branch.clone(), run);
    }

    /// 何も実行中でないかどうか (呼び出し側がポーリングを丸ごと飛ばせるように)。
    pub fn is_empty(&self) -> bool {
        self.by_branch.is_empty()
    }

    /// 終わったものを取り除いて返す。
    ///
    /// この「取り除く」処理が死んだワーカーの自己修復にもなっている。結果を
    /// 送らずに sender を drop したスレッドはここで枠を解放するので、同じ
    /// ブランチへの次の要求は「すでに実行中」と言われずに始められる。
    pub fn take_finished(&mut self) -> Vec<FinishedRun> {
        let mut finished = Vec::new();
        self.by_branch.retain(|branch, run| {
            let outcome = match run.result.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    RunOutcome::Failed("revidere analyze ended without a result".to_string())
                }
            };
            finished.push(FinishedRun {
                branch: branch.clone(),
                outcome,
            });
            false
        });
        finished
    }

    /// 実行中の全解析を停止する (アプリ終了時)。
    pub fn abort_all(&mut self) {
        for (_, run) in self.by_branch.drain() {
            run.abort();
        }
    }
}

impl App {
    /// 選択中の worktree の成果物を、前回から変わっていれば読み直す。
    /// [App::refresh_reviews] から呼ばれる。
    ///
    /// 成果物のパスと更新時刻が前回と同じなら何もしない。読み直しは git diff を
    /// 取り直すので、MCP がコメントを 1 件書くたびに走らせるには重い。作業ツリーが
    /// 動いて読む順がずれる可能性はあるが、それが問題になるのはビューを開いて
    /// いるときだけなので、そちらは [App::cmd_show_revidere] が強制読み直しで
    /// 面倒を見る。
    pub fn reload_revidere(&mut self) {
        let worktree = self.selected_worktree_path();
        let stamp = artifact_stamp(&worktree);
        if stamp.is_some() && stamp == self.revidere.loaded_from && self.revidere.has_review() {
            return;
        }
        self.reload_revidere_now();
    }

    /// 門を通さずに読み直す。ビューを開く直前など、いま画面に出すものが
    /// 最新でなければならない場面で使う。
    pub fn reload_revidere_now(&mut self) {
        let worktree = self.selected_worktree_path();
        let stamp = artifact_stamp(&worktree);
        match crate::revidere::load(&worktree) {
            crate::revidere::LoadOutcome::Missing => {
                self.revidere.replace(None);
                self.revidere.load_error = None;
            }
            crate::revidere::LoadOutcome::Loaded(review) => {
                self.revidere.replace(Some(review));
                self.revidere.load_error = None;
                self.revidere.loaded_from = stamp;
            }
            crate::revidere::LoadOutcome::Broken(why) => {
                log::warn!("revidere artifact unreadable: {why}");
                self.revidere.replace(None);
                self.revidere.load_error = Some(why);
            }
        }
    }

    /// 2 列のレビュービューを開く (w)。成果物が無ければ作り方を案内して開かない。
    pub fn cmd_show_revidere(&mut self) {
        // 門を通さずに読み直す。ユーザが端末で revidere analyze を打った直後
        // かもしれないし、成果物が同じでも作業ツリーが動いていれば読む順は
        // 変わっている。
        self.reload_revidere_now();
        if let Some(why) = self.revidere.load_error.clone() {
            self.set_status(
                format!("Review artifact unreadable: {why}"),
                StatusLevel::Error,
            );
            return;
        }
        if self.revidere.current.is_none() {
            let branch = self.selected_worktree_branch();
            let hint = if self.revidere.runs.is_running(&branch) {
                "analysing now — this takes a few minutes"
            } else {
                "press W (or the palette's Analyze entry) to build one"
            };
            self.set_status(
                format!("No review artifact for this worktree — {hint}."),
                StatusLevel::Warning,
            );
            return;
        }
        self.set_focus(Focus::Revidere);
    }

    /// 選択中の worktree に対して `revidere analyze` を起こす。
    ///
    /// `force` は revidere に `--no-cache` を渡す。既定ではキャッシュが
    /// 効くので、diff が動いていなければ AI は起動せず即座に返る — 旧
    /// walkthrough が「同じコミットならスキップ」で自前に持っていた判断は、
    /// こちらでは revidere のキャッシュがそのまま引き受ける。
    pub fn cmd_analyze_revidere(&mut self, force: bool) {
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status(
                "No worktree selected — open one to analyse.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        if self.revidere.runs.is_running(&branch) {
            self.set_status(
                "revidere is already analysing this branch.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let worktree = self.selected_worktree_path();

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_analyze(&worktree, force, &worker_cancel)
            }))
            .unwrap_or_else(|_| RunOutcome::Failed("revidere analyze thread panicked".to_string()));
            // receiver が閉じているのはアプリが先に進んだということ。
            // 報告先も後始末すべきものも無い。
            let _ = tx.send(outcome);
        });

        self.revidere.runs.insert(RevidereRun {
            branch,
            result: rx,
            cancel,
        });
        // フォーカスは動かさない。パレットから始めても、動いている端末から
        // 入力を奪わないようにするため。
        self.set_status(
            "Analysing this worktree with revidere — this takes a few minutes.".to_string(),
            StatusLevel::Info,
        );
    }

    /// 2 列ビューで選択中の節が指す位置を、通常の Viewer で開く (Enter)。
    ///
    /// レビューコメントを書けるのは Viewer なので、ここが 2 列ビューと既存の
    /// コメント作成をつなぐ口になる。着地先はその節が最初に持っている変更行で、
    /// 借りた文脈行は飛ばす — 節の話の中心は、借りた行ではなく持ち物の行にある。
    pub fn jump_to_selected_section(&mut self) {
        let Some(review) = self.revidere.current.as_ref() else {
            return;
        };
        let Some(placed) = review.order.sections.get(self.revidere.selected) else {
            return;
        };
        // 最初の「持ち物の行」を持つ束を探す。行を持たない変更 (バイナリなど) しか
        // 無い節は開く先が無いので、そう言って止まる。
        let target = placed.blocks.iter().find_map(|b| {
            let line = b.lines.iter().find(|l| l.owned)?;
            Some((b.path.clone(), line.line.new_line.or(line.line.old_line)))
        });
        let Some((path, line)) = target else {
            self.set_status(
                "This section has no line to open (file-level change only).".to_string(),
                StatusLevel::Warning,
            );
            return;
        };

        // 節のパスの綴りと diff 側のそれは違いうるので、diff 側の表記に寄せる。
        let Some(file_path) = self.diff_state.resolve_changed_path(&path) else {
            self.set_status(
                format!("Section's file isn't in this diff: {path}"),
                StatusLevel::Warning,
            );
            return;
        };
        // 折りたたまれたディレクトリの中のファイルは、展開するまで表示行が無い。
        let Some(file_diff) = self
            .diff_state
            .reveal_path(&file_path)
            .and_then(|i| self.diff_state.resolve_file(i))
        else {
            self.set_status(
                format!("Section's file is in the diff but has no row: {file_path}"),
                StatusLevel::Warning,
            );
            return;
        };

        let file_diff = file_diff.clone();
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&file_path, tab_width);
        self.viewer_state.reveal_file_in_tree(&file_path);
        self.rehighlight_viewer();
        self.review_state.build_file_comment_cache(&file_path);
        self.expand_threads_for_file(&file_path);
        self.viewer_state.build_unified_diff_view(&file_diff);
        if let Some(idx) = self.diff_state.display_index_for_path(&file_path) {
            self.viewer_state.explorer.diff_list_selected = idx;
        }
        if let Some(pos) = line.and_then(|n| {
            self.viewer_state
                .diff_view
                .diff_view_lines
                .iter()
                .position(|e| {
                    matches!(e, crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(m), .. } if *m == n as usize)
                })
        }) {
            self.viewer_state.diff_view.diff_view_scroll = pos.saturating_sub(3);
        }
        self.set_focus(Focus::Viewer);
    }

    /// 実行中の解析をすべて止める。終了時に一度だけ呼ばれる。これが無いと、
    /// メインループが止まったあとも子プロセスが孤児として走り続ける。
    pub fn shutdown_revidere(&mut self) {
        self.revidere.runs.abort_all();
    }

    /// 終わった解析を回収し、成果物を読み直す。
    /// [App::poll_all_background_ops](Self::poll_all_background_ops) から呼ばれる。
    pub fn poll_revidere(&mut self) {
        if self.revidere.runs.is_empty() {
            return;
        }
        let finished = self.revidere.runs.take_finished();
        if finished.is_empty() {
            return;
        }
        // メッセージにブランチ名を入れる。複数の worktree で同時に走らせて
        // いると、終わったのがいま見ているものとは限らない。
        for FinishedRun { branch, outcome } in finished {
            let (message, level) = match outcome {
                RunOutcome::Done {
                    coverage_complete: true,
                } => (
                    format!("Review ready for '{branch}'."),
                    StatusLevel::Success,
                ),
                RunOutcome::Done {
                    coverage_complete: false,
                } => (
                    format!("Review ready for '{branch}', but some changed lines are unexplained."),
                    StatusLevel::Warning,
                ),
                RunOutcome::Failed(why) => (
                    format!("revidere failed for '{branch}': {why}"),
                    StatusLevel::Error,
                ),
            };
            self.set_status(message, level);
        }
        self.reload_revidere_now();
    }
}

/// 成果物の (パス, 更新時刻)。無ければ None。
fn artifact_stamp(worktree: &std::path::Path) -> Option<(PathBuf, std::time::SystemTime)> {
    let path = ::revidere::review::artifact_path(worktree);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some((path, modified))
}

/// `conductor revidere analyze` を 1 回最後まで走らせる。ブロッキング。
///
/// 解析の実装は同じバイナリの中にあるが、それでも子プロセスとして起こす。
/// 中断がプロセスを kill するだけで済み、その先の AI コマンドまで確実に
/// 道連れにできるため。スレッドで直接呼ぶと、AI の待ちを割り込めない。
///
/// 終了コードの読み方は revidere の約束どおり: 0 が成功、2 は「成果物は
/// できたが充足検査が通らなかった」。2 を失敗として扱うと、読める成果物が
/// 画面に出ないまま捨てられる。
fn run_analyze(worktree: &PathBuf, force: bool, cancel: &Arc<AtomicBool>) -> RunOutcome {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return RunOutcome::Failed(format!("could not find conductor itself: {e}")),
    };
    let mut command = Command::new(exe);
    command
        .args(["revidere", "analyze", "--repo"])
        .arg(worktree)
        .args(["--head", ::revidere::git::WORKTREE]);
    if force {
        command.arg("--no-cache");
    }
    command.current_dir(worktree);

    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return RunOutcome::Failed(format!("could not start revidere: {e}")),
    };

    // stdout と stderr はそれぞれ専用スレッドで吸い出す。AI コマンドが
    // 進捗を吐き続けてパイプのバッファを埋め、終了する前にデッドロックする
    // のを防ぐため (ai_caller と同じ理由・同じ形)。
    let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
    let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

    let start = std::time::Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return RunOutcome::Failed("cancelled".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= RUN_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return RunOutcome::Failed(format!(
                        "timed out after {} minutes",
                        RUN_TIMEOUT.as_secs() / 60
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return RunOutcome::Failed(format!("could not wait on revidere: {e}")),
        }
    };

    let stdout = join_pipe_reader(stdout_reader);
    let stderr = join_pipe_reader(stderr_reader);
    log::info!("revidere analyze finished ({status}):\n{stdout}");

    match status.code() {
        Some(0) => RunOutcome::Done {
            coverage_complete: true,
        },
        Some(2) => RunOutcome::Done {
            coverage_complete: false,
        },
        _ => RunOutcome::Failed(tail_chars(stderr.trim(), 300).to_string()),
    }
}

/// 子プロセスのパイプをワーカースレッドで最後まで読み、UTF-8 として寛容にデコードする。
fn spawn_pipe_reader<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn join_pipe_reader(handle: Option<std::thread::JoinHandle<String>>) -> String {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// s の末尾 n 文字 (文字境界を壊さず)。
fn tail_chars(s: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match s.char_indices().nth_back(n - 1) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    fn run(branch: &str) -> (RevidereRun, Sender<RunOutcome>, Arc<AtomicBool>) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            RevidereRun {
                branch: branch.to_string(),
                result: rx,
                cancel: cancel.clone(),
            },
            tx,
            cancel,
        )
    }

    #[test]
    fn different_branches_analyse_side_by_side() {
        let mut runs = RevidereRuns::default();
        let (a, _tx_a, _) = run("feature/a");
        let (b, _tx_b, _) = run("feature/b");
        runs.insert(a);
        runs.insert(b);

        assert!(runs.is_running("feature/a"));
        assert!(runs.is_running("feature/b"));
        // どちらも相手を置き換えたり終了させたりしていない。
        assert!(runs.take_finished().is_empty());

        runs.abort_all();
        assert!(runs.is_empty());
    }

    #[test]
    fn a_finished_run_frees_its_branch() {
        let mut runs = RevidereRuns::default();
        let (a, tx, _) = run("feature/a");
        runs.insert(a);
        tx.send(RunOutcome::Done {
            coverage_complete: true,
        })
        .expect("receiver is alive");

        let finished = runs.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].branch, "feature/a");
        assert!(!runs.is_running("feature/a"));
    }

    /// 古いロックからの回復: 結果を送らずに死んだワーカーは枠を解放しなければ
    /// ならず、次の要求は「すでに実行中」ではなく新しい解析を始められる。
    #[test]
    fn a_dead_worker_frees_its_branch() {
        let mut runs = RevidereRuns::default();
        let (a, tx, _) = run("feature/a");
        runs.insert(a);
        drop(tx);

        let finished = runs.take_finished();
        assert_eq!(finished.len(), 1);
        match &finished[0].outcome {
            RunOutcome::Failed(why) => assert!(why.contains("without a result"), "got: {why}"),
            _ => panic!("死んだワーカーは失敗として届くはず"),
        }
        assert!(!runs.is_running("feature/a"));
    }

    #[test]
    fn abort_all_signals_every_worker_to_stop() {
        let mut runs = RevidereRuns::default();
        let (a, _tx_a, cancel_a) = run("feature/a");
        let (b, _tx_b, cancel_b) = run("feature/b");
        runs.insert(a);
        runs.insert(b);

        runs.abort_all();

        assert!(cancel_a.load(Ordering::Relaxed));
        assert!(cancel_b.load(Ordering::Relaxed));
        assert!(runs.is_empty());
    }
}
