//! [App] における AI ウォークスルー生成のオーケストレーション。
//!
//! バックグラウンドスレッドで [crate::walkthrough::generate] を実行する — 問い合わせ
//! 先は [api] で指定されたモデルであり、Conductor 自身が起動する claude プロセスでは
//! 決してない — そして結果を [crate::review_store::ReviewStore] 経由でレビュー
//! データベースへ反映する。
//!
//! 生成はブランチごとに最大1件まで同時実行できる（[WalkthroughGenerations]）ため、
//! あるワークツリーを見て回っているレビュアーも、別のワークツリーでウォークスルー
//! 生成を開始できる。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use super::*;

/// 実行中の生成: 結果がどこに届くか、どのブランチのためのものか、そして
/// 停止させるためのフラグを持つ。
pub struct WalkthroughGeneration {
    pub branch: String,
    result: Receiver<Result<crate::walkthrough::Generated, String>>,
    cancel: Arc<AtomicBool>,
}

impl WalkthroughGeneration {
    /// ワーカーに停止を通知する。AI 呼び出し側は子プロセスをポーリングする合間に
    /// これを確認するので、外部コマンドは動かしっぱなしにされず確実にkillされる。
    fn abort(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 実行を終えた生成。[WalkthroughGenerations::take_finished] が返す。
/// 対象のブランチを保持しているのは、呼び出し側が残されたデータベースの行を
/// 整合させる時点で、ハンドル自体はすでになくなっているため。
pub struct FinishedGeneration {
    pub branch: String,
    pub outcome: Result<crate::walkthrough::Generated, String>,
}

/// この Conductor インスタンスで実行中の全ウォークスルー生成。ブランチごとに
/// 最大1件まで。
///
/// 「同時に1件まで」ではなくブランチをキーにしているのは、生成が実際に競合する
/// 対象がブランチだけだからである。begin_walkthrough は対象ブランチの
/// walkthroughs 行を削除して作り直し、save_walkthrough がそれを置き換える。
/// つまり同じブランチで2つの生成が走ると1つの行を奪い合い、負けた側の手順が
/// 消えてしまう。*別の*ブランチの生成 — git は同じブランチを二重にチェックアウト
/// できないので、これは別のワークツリーを意味する — は別々の行に触れ、データベースは
/// すでに WAL + busy_timeout（review_store::schema 参照）になっているので
/// 並行して走らせて構わない。すべてを直列化するのはやりすぎで、あるワークツリーを
/// 見て回っているレビュアーが別のワークツリーでウォークスルーを開始できなくなって
/// しまっていた。
#[derive(Default)]
pub struct WalkthroughGenerations {
    by_branch: HashMap<String, WalkthroughGeneration>,
}

impl WalkthroughGenerations {
    /// branch の生成が現在実行中かどうか。
    pub fn is_generating(&self, branch: &str) -> bool {
        self.by_branch.contains_key(branch)
    }

    /// 起動したばかりの生成を登録する。呼び出し側は事前に [Self::is_generating]
    /// を確認している前提。稼働中のハンドルに上書き挿入すると receiver が drop され、
    /// ワーカーの結果が行き場を失い、そのブランチの行が generating のまま
    /// 永久に取り残されてしまう。
    pub fn insert(&mut self, generation: WalkthroughGeneration) {
        debug_assert!(
            !self.is_generating(&generation.branch),
            "would orphan the in-flight generation for {}",
            generation.branch
        );
        self.by_branch
            .insert(generation.branch.clone(), generation);
    }

    /// 実行中の全生成の結果チャンネルを取り出し、すでに終わったものを取り除いて
    /// それぞれが生成した結果を返す。
    ///
    /// この「取り除く」処理が、死んだワーカーを自己修復にしている。パニックした
    /// スレッドや結果を送らずに sender を drop したスレッドは、ここで自分の枠を
    /// 解放するので、同じブランチへの次のリクエストは「すでに実行中」と言われる
    /// のではなく新しい生成を開始できる。
    pub fn take_finished(&mut self) -> Vec<FinishedGeneration> {
        let mut finished = Vec::new();
        self.by_branch.retain(|branch, generation| {
            let outcome = match generation.result.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) => return true,
                // 送信せずにワーカーが死んだ場合: generating のまま永久に放置するのではなく
                // 失敗として扱う。
                Err(TryRecvError::Disconnected) => {
                    Err("walkthrough generation ended without a result".to_string())
                }
            };
            finished.push(FinishedGeneration {
                branch: branch.clone(),
                outcome,
            });
            false
        });
        finished
    }

    /// 何も実行中でないかどうか（呼び出し側がポーリングを丸ごとスキップできるようにする）。
    pub fn is_empty(&self) -> bool {
        self.by_branch.is_empty()
    }

    /// 実行中の全生成を停止する（アプリのシャットダウン時に使用）。
    pub fn abort_all(&mut self) {
        for (_, generation) in self.by_branch.drain() {
            generation.abort();
        }
    }
}

impl App {
    /// 選択中のワークツリーのブランチに対してウォークスルー生成を開始する:
    /// generating 行を挿入してからバックグラウンドワーカーを起動する。
    /// 再実行はゼロから生成し直す — ただし現在のブランチ先端に対してすでに ready
    /// なウォークスルーが存在する場合は何もせず、それを再表示するだけである
    /// （diff が変わっていなければウォークスルーも変わっていないため）。
    /// *このブランチ*の生成がすでに実行中の場合は何もせずステータスヒントを出す。
    /// 他のブランチの生成には影響せず走り続けるので、複数のワークツリーで同時に
    /// 生成できる。
    pub fn cmd_generate_walkthrough(&mut self, force: bool) {
        if self.review_store.is_none() {
            self.set_status(
                "Review database unavailable — cannot generate a walkthrough.".to_string(),
                StatusLevel::Error,
            );
            return;
        }
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status(
                "No worktree selected — open one to generate a walkthrough.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        // 実行中の生成は*ブランチごとに*1件までしか許されない: ハンドルを置き換えると
        // 実行中のワーカーの receiver が drop され、そのブランチの行が generating
        // のまま永久に取り残されてしまう。他のブランチの生成はこのブランチには
        // 関係ない — 別々の行に書き込むので並行して走らせられる
        // （[WalkthroughGenerations] 参照）。
        if self.walkthrough.generations.is_generating(&branch) {
            self.set_status(
                "A walkthrough is already being generated for this branch.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let Some((wt_path, head_oid)) = self
            .worktrees
            .get(self.worktrees.selected_index())
            .map(|w| (w.path.clone(), w.head_oid.clone()))
        else {
            return;
        };

        // この正確なブランチ先端をすでに ready なウォークスルーがカバーしている場合は
        // 再生成をスキップする: diff が動いていなければウォークスルーも動いていない。
        // これは現在の HEAD が実際に分かっている場合のみ成立する — 未知の先端
        // （またはトラッキング開始前の行）は決して一致しないので、常に再生成される。
        // force（Alt+w、またはパレットの force エントリ）はこれを迂回して
        // 強制的に再構築する。
        let up_to_date = !force
            && head_oid.as_deref().is_some_and(|head| {
            self.review_store
                .as_ref()
                .and_then(|s| s.get_walkthrough(&branch).ok().flatten())
                .is_some_and(|(w, _)| {
                    w.status == crate::walkthrough::WalkthroughStatus::Ready
                        && w.head_commit.as_deref() == Some(head)
                })
        });
        if up_to_date {
            let short: String = head_oid
                .as_deref()
                .map(|h| h.chars().take(8).collect())
                .unwrap_or_default();
            self.viewer_state.explorer.explorer_bottom_view =
                crate::viewer::ExplorerBottomView::Walkthrough;
            self.set_status(
                format!(
                    "Walkthrough already up to date for commit {short} — showing it. \
                     Alt+w (or the palette's force entry) to regenerate anyway."
                ),
                StatusLevel::Info,
            );
            return;
        }

        // UI（とタイムアウト処理）が反映する行を常に持てるよう、先に generating
        // 行を挿入してから起動する。ベース ref は、このブランチが PR intake 経由で
        // 取り込まれた場合には PR のメタ情報から得る。次回同じコミットで再生成した
        // ときに上の分岐で短絡できるよう、ブランチ先端を記録しておく。
        let store = self.review_store.as_ref().expect("checked above");
        if let Err(e) = store.begin_walkthrough(&branch, head_oid.as_deref()) {
            let msg = format!("Failed to start walkthrough: {e}");
            self.set_status(msg, StatusLevel::Error);
            return;
        }
        let base_ref = store
            .get_pr_review_meta(&branch)
            .ok()
            .flatten()
            .and_then(|m| m.base_ref);
        let api = self.config.api.clone();
        let language = self.config.review.walkthrough_language.clone();
        // [review] walkthrough_model はここには渡さない: どのモデルが答えるかは、
        // 今や設定されたコマンド側の責務であり Conductor の責務ではない。特定のモデルを
        // 使いたいユーザは [api] command の側で指定する。
        let worktree = wt_path.clone();
        let branch_for_thread = branch.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::walkthrough::generate(
                    &api,
                    &worktree,
                    &branch_for_thread,
                    base_ref.as_deref(),
                    language.as_deref(),
                    &worker_cancel,
                )
            }))
            .unwrap_or_else(|_| Err("walkthrough generation thread panicked".to_string()));
            // receiver が閉じているのはアプリが先に進んだ（またはシャットダウンした）
            // ことを意味する。報告先も後始末すべきものもない。
            let _ = tx.send(outcome);
        });

        self.walkthrough.generations.insert(WalkthroughGeneration {
            branch: branch.clone(),
            result: rx,
            cancel,
        });
        // 表示だけを切り替える — set_focus は呼ばない。そのためパレットから生成を
        // 開始してもアクティブなターミナル入力からフォーカスを奪うことはない。
        // レビュアーが実際に Explorer を見たときに進行中の状態が見えるようにするだけ。
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::Walkthrough;
        self.set_status(
            "Generating walkthrough in the background — this takes a few minutes.".to_string(),
            StatusLevel::Info,
        );
        self.refresh_reviews();
    }

    /// 実行中の生成をすべて停止し、孤児サブプロセスとしてアプリより長生きしないように
    /// する。シャットダウン時に一度だけ呼ばれる（event_loop.rs の should_quit で
    /// 返る直前を参照）— その時点でまだ生成が動いていると、結果を読む者が誰もいない
    /// まま延々とトークンを消費し続けてしまう。
    pub fn shutdown_walkthrough_generation(&mut self) {
        self.walkthrough.generations.abort_all();
    }

    /// 実行中の生成の結果チャンネルを取り出し、それぞれのデータベース行と整合させる。
    /// [App::poll_all_background_ops](Self::poll_all_background_ops) から呼ばれる。
    ///
    /// 旧来の headless セッション経由の方式と異なり、行の ready 状態はパースした
    /// 返信から*ここで*書き込まれる — モデル自身には、素のテキストというインタフェースを
    /// 越えてそれを書き込む手段がない。これが、壊れた返信によって行が generating
    /// のまま止まってしまうことがもはや起きない理由でもある。
    pub fn poll_walkthrough_generation(&mut self) {
        if self.walkthrough.generations.is_empty() {
            return;
        }
        let finished = self.walkthrough.generations.take_finished();
        if finished.is_empty() {
            return;
        }
        for generation in finished {
            let (message, level) = self.reconcile_finished_generation(generation);
            self.set_status(message, level);
        }
        self.refresh_reviews();
    }

    /// 完了した1件の生成をフラッシュ表示用のステータスメッセージへ変換する。
    /// 使えるウォークスルーを生成できなかった場合は failed 行を書き込む。
    ///
    /// メッセージにはブランチ名を含める: 複数のワークツリーで同時に生成していると、
    /// 完了するのはレビュアーが見ているものとは限らないことが多いため。
    fn reconcile_finished_generation(
        &mut self,
        finished: FinishedGeneration,
    ) -> (String, StatusLevel) {
        let FinishedGeneration { branch, outcome } = finished;
        match outcome {
            Ok(generated) => self.save_generated_walkthrough(&branch, generated),
            Err(error) => {
                if let Some(store) = &self.review_store {
                    let _ = store.fail_walkthrough(&branch, &error);
                }
                (
                    format!("Walkthrough failed for '{branch}': {error}"),
                    StatusLevel::Error,
                )
            }
        }
    }

    /// パースした生成結果をレビューデータベースへ書き込み、表示するステータス行を返す。
    ///
    /// インラインコメントの挿入はあえてベストエフォートにしている。あくまでツアーに
    /// 対するおまけの追加要素なので、コメントの挿入に失敗しても、それ自体が
    /// 問題のないウォークスルーを失敗扱いにしてしまってはいけない。
    fn save_generated_walkthrough(
        &mut self,
        branch: &str,
        generated: crate::walkthrough::Generated,
    ) -> (String, StatusLevel) {
        let Some(store) = &self.review_store else {
            return (
                "Walkthrough generated but the review database is unavailable.".to_string(),
                StatusLevel::Error,
            );
        };
        let step_count = generated.steps.len();
        if let Err(e) = store.save_walkthrough(
            branch,
            &generated.title,
            &generated.summary,
            &generated.steps,
        ) {
            let msg = format!("Failed to save walkthrough: {e}");
            let _ = store.fail_walkthrough(branch, &msg);
            return (msg, StatusLevel::Error);
        }

        let mut saved_comments = 0usize;
        for comment in &generated.comments {
            let Some(line_start) = comment.line_start else {
                continue;
            };
            let result = store.add_review(
                branch,
                &comment.file_path,
                line_start,
                comment.line_end,
                crate::review_store::CommentKind::Question,
                &comment.body,
                "HEAD",
                crate::review_store::Author::Claude,
                Some(branch),
            );
            match result {
                Ok(_) => saved_comments += 1,
                Err(e) => log::warn!("failed to save generated inline comment: {e}"),
            }
        }

        let comments = if saved_comments > 0 {
            format!(", {saved_comments} inline comment(s)")
        } else {
            String::new()
        };
        (
            format!("Walkthrough ready for '{branch}' ({step_count} step(s){comments})."),
            StatusLevel::Success,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    type Outcome = Result<crate::walkthrough::Generated, String>;

    /// 登録済みの生成と、そのワーカースレッドが保持するはずの sender のペア。
    /// sender を生かしておくことが [WalkthroughGenerations::take_finished] にとって
    /// 「まだ実行中」を意味し、drop することがここから見た「結果を残さずに死んだ
    /// ワーカー」の見え方になる。
    fn generation(branch: &str) -> (WalkthroughGeneration, Sender<Outcome>, Arc<AtomicBool>) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            WalkthroughGeneration {
                branch: branch.to_string(),
                result: rx,
                cancel: cancel.clone(),
            },
            tx,
            cancel,
        )
    }

    fn sample_generated() -> crate::walkthrough::Generated {
        crate::walkthrough::Generated {
            title: "t".to_string(),
            summary: "s".to_string(),
            steps: Vec::new(),
            comments: Vec::new(),
        }
    }

    #[test]
    fn different_branches_generate_side_by_side() {
        // これが置き換えたバグ: 実行中の生成が1件あると、自分のブランチだけでなく
        // 他のすべてのワークツリーのブランチもブロックされていた。
        let mut generations = WalkthroughGenerations::default();
        let (a, _tx_a, _) = generation("feature/a");
        let (b, _tx_b, _) = generation("feature/b");
        generations.insert(a);
        generations.insert(b);

        assert!(generations.is_generating("feature/a"));
        assert!(generations.is_generating("feature/b"));
        // どちらも相手を置き換えたり終了させたりしていない。
        assert!(generations.take_finished().is_empty());

        generations.abort_all();
        assert!(generations.is_empty());
    }

    #[test]
    fn only_the_same_branch_is_refused() {
        let mut generations = WalkthroughGenerations::default();
        let (a, _tx_a, _) = generation("feature/a");
        generations.insert(a);

        // この述語が cmd_generate_walkthrough が参照するガードである。
        assert!(generations.is_generating("feature/a"));
        assert!(!generations.is_generating("feature/b"));
    }

    #[test]
    fn a_finished_generation_frees_its_branch() {
        let mut generations = WalkthroughGenerations::default();
        let (a, tx, _) = generation("feature/a");
        generations.insert(a);
        tx.send(Ok(sample_generated())).expect("receiver is alive");

        let finished = generations.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].branch, "feature/a");
        assert!(finished[0].outcome.is_ok());
        assert!(!generations.is_generating("feature/a"));
    }

    #[test]
    fn a_dead_worker_frees_its_branch() {
        // 古いロックからの回復: パニックした（あるいは何らかの理由で結果を送らずに
        // sender を drop した）スレッドは自分の枠を解放しなければならず、次のリクエストは
        // 「すでに実行中」と言われるのではなく再生成される。
        let mut generations = WalkthroughGenerations::default();
        let (a, tx, _) = generation("feature/a");
        generations.insert(a);
        drop(tx);

        let finished = generations.take_finished();
        assert_eq!(finished.len(), 1);
        let err = finished[0].outcome.as_ref().unwrap_err();
        assert!(err.contains("without a result"), "got: {err}");
        assert!(!generations.is_generating("feature/a"));

        // そしてそのブランチは即座に新しい生成を受け付ける。
        let (again, _tx, _) = generation("feature/a");
        generations.insert(again);
        assert!(generations.is_generating("feature/a"));
    }

    #[test]
    fn abort_all_signals_every_worker_to_stop() {
        let mut generations = WalkthroughGenerations::default();
        let (a, _tx_a, cancel_a) = generation("feature/a");
        let (b, _tx_b, cancel_b) = generation("feature/b");
        generations.insert(a);
        generations.insert(b);

        generations.abort_all();

        // 各ワーカー（そしてそれを通じて AI 呼び出し側の子プロセス）がポーリングするフラグ。
        assert!(cancel_a.load(Ordering::Relaxed));
        assert!(cancel_b.load(Ordering::Relaxed));
        assert!(generations.is_empty());
    }
}
