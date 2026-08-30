//! [App] における revidere の駆動: 成果物の読み直しと、解析の起動。
//!
//! 解析そのものは revidere が持ち、AI をどう呼ぶかだけをここから差し込む
//! ([AiSeam])。呼び先は他の AI 機能と同じ `[api]` 設定なので、レビューのために
//! 別の設定ファイルを用意する必要は無い。
//!
//! ただし `provider = "gemini"` では使えない。プロンプトが渡すのは変更箇所の
//! 一覧までで、中身はモデルが自分でリポジトリを読む前提のため、素の HTTP 補完
//! ではなくエージェント型の CLI を指した `provider = "command"` が要る。
//!
//! 見るのはベースから作業ツリーまで
//!
//! 起点はベースとの共通祖先で、終点は今の作業ツリー。ブランチでやったこと
//! 全部が対象になり、まだコミットしていない手元の変更もそこに入る。作り直す
//! ときも同じで、前回の成果物は何も引き継がない。
//! 出力先は `<worktree>/.conductor/review.json` — conductor の worktree は
//! それぞれ別ディレクトリなので、ブランチごとに自然に分かれる。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crate::app::{App, Focus, StatusLevel};
use crate::overlay::{ActiveOverlay, RevidereArtifact, RevidereConfirmOverlay};

/// 解析 1 回の AI 呼び出しに与える実時間の上限 (秒)。
///
/// `[api] command_timeout_secs` の既定は数秒のブランチ命名を想定した値なので、
/// 差分を読んで語る呼び出しはそのままでは打ち切られる。予算を知っているのは
/// タスクの側という [crate::ai_caller::TaskEnv] の考え方どおり、ここで上書きする。
const AI_TIMEOUT_SECS: u64 = 15 * 60;

/// 解析が終わったときの結果。
pub enum RunOutcome {
    /// 成果物ができた。coverage_complete が false なら、成果物はあるが
    /// 説明もれ検査は通っていない。
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
    /// ワーカーに停止を通知する。AI を待っている間もこの旗は見られていて
    /// ([crate::ai_caller::CommandCaller])、走っている AI コマンドは kill される。
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
                    RunOutcome::Failed("the analysis ended without a result".to_string())
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
        let stamp = artifact_stamp(&worktree, self.revidere.scope);
        if stamp.is_some() && stamp == self.revidere.loaded_from && self.revidere.has_review() {
            return;
        }
        self.reload_revidere_now();
    }

    /// 門を通さずに読み直す。ビューを開く直前など、いま画面に出すものが
    /// 最新でなければならない場面で使う。
    pub fn reload_revidere_now(&mut self) {
        let worktree = self.selected_worktree_path();
        let stamp = artifact_stamp(&worktree, self.revidere.scope);
        match crate::revidere::load(&worktree, self.revidere.scope) {
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

    /// 2 列のレビュービューを開く (w)。成果物が無ければ、その場で作るかを聞く。
    pub fn cmd_show_revidere(&mut self) {
        // 門を通さずに読み直す。成果物が同じでも作業ツリーが動いていれば
        // 読む順は変わっている。
        self.reload_revidere_now();
        if let Some(why) = self.revidere.load_error.clone() {
            self.set_status(
                format!("Review artifact unreadable: {why}"),
                StatusLevel::Error,
            );
            return;
        }
        if self.revidere.current.is_none() {
            // 「無い」と言って終わるより、作る口をその場で出したほうが早い。
            self.cmd_confirm_analyze_revidere();
            return;
        }
        self.set_focus(Focus::Revidere);
    }

    /// 選択中の worktree の解析がいまどの状態か。
    ///
    /// 常設表示もクリックもここだけを見る。画面が「最新」と言っているのに
    /// 押すと解析が始まる、のような食い違いが起きようがないようにするため。
    pub fn revidere_artifact_state(&self) -> crate::revidere::ArtifactState {
        let head_time = self
            .worktrees
            .get(self.worktrees.selected_index())
            .and_then(|w| w.head_time);
        crate::revidere::artifact_state(
            &self.selected_worktree_path(),
            head_time,
            self.revidere
                .runs
                .is_running(&self.selected_worktree_branch()),
        )
    }

    /// Changed files パネルの状態チップを押したとき。
    ///
    /// 解析中は止めない。数分かかる仕事を、枠の中の 10 セルを 1 回押しただけで
    /// 確認も無く捨てられるようにはしない。始めるほうも同じ理由で確認を通す。
    pub fn cmd_revidere_badge_click(&mut self) {
        use crate::revidere::ArtifactState;
        match self.revidere_artifact_state() {
            ArtifactState::Running => self.set_status(
                "revidere is analysing this branch — this takes a few minutes.".to_string(),
                StatusLevel::Info,
            ),
            ArtifactState::Fresh => self.cmd_show_revidere(),
            ArtifactState::None | ArtifactState::Stale => self.cmd_confirm_analyze_revidere(),
        }
    }

    /// 解析の前に確認を出す (W、メニューの 2 つの入口、PR の取り込みの後)。
    ///
    /// AI の呼び出しは数分と費用がかかるので、走り出す前に一度止める。
    /// worktree が無いときや既に走っているときの断り方は解析側が持っている
    /// ので、ここでは確認を挟まずそのまま渡す。
    pub fn cmd_confirm_analyze_revidere(&mut self) {
        let branch = self.selected_worktree_branch();
        if branch.is_empty() || self.revidere.runs.is_running(&branch) {
            self.cmd_analyze_revidere(false);
            return;
        }
        let scope = self.revidere.scope;
        let head = self.worktrees.selected().and_then(|w| w.head_oid.clone());
        let artifact = match crate::revidere::artifact_head(&self.selected_worktree_path(), scope) {
            None => RevidereArtifact::None,
            Some(analysed) if Some(&analysed) == head.as_ref() => RevidereArtifact::Current,
            Some(_) => RevidereArtifact::Stale,
        };
        self.overlays.revidere_confirm = RevidereConfirmOverlay {
            branch,
            scope: crate::revidere::scope_label(scope),
            artifact,
        };
        self.overlays.active = ActiveOverlay::RevidereConfirm;
    }

    /// 確認を通ったので解析を始める。
    ///
    /// 同じコミットの成果物があるなら貯めた応答を捨てる。捨てないと、作り直しを
    /// 選んだのに前と同じ答えがそのまま返ってくる。
    pub fn confirm_analyze_revidere(&mut self) {
        let force = self.overlays.revidere_confirm.artifact == RevidereArtifact::Current;
        self.overlays.active = ActiveOverlay::None;
        self.cmd_analyze_revidere(force);
    }

    pub fn cancel_analyze_revidere(&mut self) {
        self.overlays.active = ActiveOverlay::None;
    }

    /// 選択中の worktree の解析を起こす。
    ///
    /// `force` は貯めた応答を捨てる。既定では効くので、diff が動いていなければ
    /// AI は起動せず即座に返る — 旧 walkthrough が「同じコミットならスキップ」で
    /// 自前に持っていた判断は、こちらでは revidere のキャッシュが引き受ける。
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
        let api = self.config.api.clone();
        let scope = self.revidere.scope;
        // 起点はブランチ全体の成果物にしか書かれていない。無いのは 1 度目の
        // レビューを作った直後で、比べる前回がまだ存在しない。
        let base = match scope {
            ::revidere::Scope::Base => None,
            ::revidere::Scope::SincePrevious => match crate::revidere::previous_head(&worktree) {
                Some(b) => Some(b),
                None => {
                    self.set_status(
                        "No previous review to compare against yet — analyse the branch \
                             (W), commit, then analyse again."
                            .to_string(),
                        StatusLevel::Warning,
                    );
                    return;
                }
            },
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_analyze(&worktree, force, scope, base, &api, &worker_cancel)
            }))
            .unwrap_or_else(|_| RunOutcome::Failed("the analysis thread panicked".to_string()));
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
        // どちらの区間かを言う。区間はビューを閉じても残るので、外から W を
        // 押すと、数分待った先で思っていない方が出てくることがある。
        self.set_status(
            format!(
                "Analysing [{}] with revidere — this takes a few minutes.",
                crate::revidere::scope_label(scope)
            ),
            StatusLevel::Info,
        );
    }

    /// 見る区間を切り替えて読み直す (p)。区間の意味は [revidere::Scope]。
    ///
    /// 切り替えた結果は画面が名乗るので、ここではステータスに出さない。
    pub fn cmd_toggle_revidere_scope(&mut self) {
        self.revidere.scope = match self.revidere.scope {
            ::revidere::Scope::Base => ::revidere::Scope::SincePrevious,
            ::revidere::Scope::SincePrevious => ::revidere::Scope::Base,
        };
        // 区間ごとに読みかけの位置は別物。持ち越すと、行数の違う diff の
        // 途中にいきなり着地する。
        self.revidere.selected = 0;
        self.revidere.diff_scroll = 0;
        self.revidere.overview_scroll = 0;
        self.reload_revidere_now();
    }

    /// 2 列ビューで選択中の項目が指す位置を、通常の Viewer で開く (Enter)。
    ///
    /// レビューコメントを書けるのは Viewer なので、ここが 2 列ビューと既存の
    /// コメント作成をつなぐ口になる。着地先はその項目が最初に持っている変更行で、
    /// 借りた文脈行は飛ばす — 項目の話の中心は、借りた行ではなく持ち物の行にある。
    pub fn jump_to_selected_section(&mut self) {
        let Some(review) = self.revidere.current.as_ref() else {
            return;
        };
        let Some(placed) = review.order.sections.get(self.revidere.selected) else {
            return;
        };
        // 最初の「持ち物の行」を持つ束を探す。行を持たない変更 (バイナリなど) しか
        // 無い項目は開く先が無いので、そう言って止まる。
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

        // 項目のパスの綴りと diff 側のそれは違いうるので、diff 側の表記に寄せる。
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
    /// メインループが止まったあとも AI コマンドが孤児として走り続ける。
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
fn artifact_stamp(
    worktree: &std::path::Path,
    scope: ::revidere::Scope,
) -> Option<(PathBuf, std::time::SystemTime)> {
    let path = ::revidere::review::artifact_path(worktree, scope);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some((path, modified))
}

/// revidere の解析を 1 回最後まで走らせる。ブロッキング。
///
/// 説明もれ検査に落ちても失敗にはしない。読める成果物ができている以上、
/// 画面に出さずに捨てる理由が無い。
fn run_analyze(
    worktree: &std::path::Path,
    force: bool,
    scope: ::revidere::Scope,
    base: Option<String>,
    api: &crate::config::ApiConfig,
    cancel: &Arc<AtomicBool>,
) -> RunOutcome {
    let env = crate::ai_caller::TaskEnv {
        timeout_secs: Some(AI_TIMEOUT_SECS),
        working_dir: Some(worktree.to_path_buf()),
    };
    let caller = match crate::ai_caller::build_caller(api, &env) {
        Ok(c) => c,
        Err(e) => return RunOutcome::Failed(e),
    };
    let ai = AiSeam {
        caller,
        identity: identity(api),
        cancel: cancel.clone(),
    };
    let options = ::revidere::Options {
        repo: worktree.to_path_buf(),
        base,
        cache: !force,
        scope,
    };
    match ::revidere::analyze(&options, &ai) {
        Ok(review) => RunOutcome::Done {
            coverage_complete: review.coverage.is_complete(),
        },
        Err(e) => RunOutcome::Failed(tail_chars(&e.to_string(), 300).to_string()),
    }
}

/// revidere の AI の継ぎ目を、conductor の [crate::ai_caller] に繋ぐ。
struct AiSeam {
    caller: Box<dyn crate::ai_caller::AiCaller>,
    identity: String,
    cancel: Arc<AtomicBool>,
}

impl ::revidere::Ai for AiSeam {
    fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        self.caller.complete(system, user, &self.cancel)
    }

    fn identity(&self) -> String {
        self.identity.clone()
    }
}

/// 貯めた応答の鍵に混ぜる、呼び先の見分け。
///
/// 答えを出しているものが変わったら別物として扱えればよいので、provider と
/// その provider が実際に叩く先だけを並べる。
fn identity(api: &crate::config::ApiConfig) -> String {
    let provider = api.provider.trim().to_lowercase();
    let target = if provider == "command" {
        api.command.join(" ")
    } else {
        api.model.clone()
    };
    format!("{provider}:{target}")
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
