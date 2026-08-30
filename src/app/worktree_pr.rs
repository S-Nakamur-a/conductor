//! [App] における PR intake とブラウザへの受け渡し。
//!
//! 「Review Pull Request」オーバーレイを駆動する: PR のメタデータと git の
//! ref を取得し、バックグラウンドでレビュー用の worktree を準備した後、
//! 結果を反映する(メタデータの永続化、worktree への切り替え、review
//! フォーカスへの遷移)。選択中の worktree の PR ページを直接ブラウザで
//! 開く機能もここにある。

use super::*;

impl App {
    // PR intake(Review Pull Request オーバーレイ)

    /// 「Review Pull Request」オーバーレイのために、バックグラウンドの PR
    /// intake(gh メタデータ + git fetch + worktree 作成)を開始する。
    /// 失敗後の再試行として再度呼んでも安全 — 以前の bg_op は単に
    /// 置き換わるだけである。
    pub fn start_pr_intake(&mut self, input: &str) {
        self.overlays.pr_input.loading = true;
        self.overlays.pr_input.error = None;

        let repo_path = self.repo.path.clone();
        let worktree_dir = self.config.general.worktree_dir.clone();
        let input = input.to_string();

        self.overlays.pr_input.bg_op.start(move |tx| {
            let outcome = crate::pr_intake::intake_pr(&repo_path, worktree_dir.as_deref(), &input);
            let _ = tx.send(outcome);
        });
    }

    /// バックグラウンドの PR intake の結果をポーリングし、あれば反映する:
    /// 新しく取得した PR メタデータを永続化し、worktree に切り替え、
    /// review モードを自動起動する。
    ///
    /// intake がまだ実行中の間にオーバーレイが(Esc で)閉じられていても
    /// 結果は反映される — その時点で fetch/worktree 作成はすでに成功して
    /// いるので、破棄すべきではない。
    pub fn poll_pr_intake(&mut self) {
        let Some(outcome) = self.overlays.pr_input.bg_op.poll() else {
            return;
        };
        self.overlays.pr_input.loading = false;

        match outcome {
            crate::pr_intake::PrIntakeOutcome::Ready {
                pr_number,
                worktree_path,
                meta,
            } => {
                if let Some(meta) = &meta
                    && let Some(store) = &self.review_store
                {
                    let _ = store.save_worktree_base_branch(&meta.branch, &meta.base_ref);
                    let _ = store.save_pr_review_meta(
                        &meta.branch,
                        Some(pr_number as i64),
                        Some(&meta.url),
                        Some(&meta.title),
                        Some(&meta.base_ref),
                        Some(&meta.head_ref),
                        meta.head_owner_login.as_deref(),
                    );
                }
                self.refresh_worktrees();
                self.select_worktree_by_path(&worktree_path);
                self.overlays.active = crate::overlay::ActiveOverlay::None;
                self.overlays.pr_input.buffer.clear();
                self.explorer.focus_on_diff_list = true;
                self.set_focus(Focus::Explorer);
                self.set_status(
                    format!("PR #{pr_number} ready for review."),
                    StatusLevel::Success,
                );
                // 取り込んだだけでは読む順が無い。「Review Pull Request」は
                // ブランチのレビューと同じ解析へ続く 1 本の道なので、そのまま
                // 確認へ渡す。
                self.cmd_confirm_analyze_revidere();
            }
            crate::pr_intake::PrIntakeOutcome::Failed { error } => {
                self.overlays.pr_input.error = Some(error.to_string());
            }
        }
    }

    // PR をブラウザで開く

    /// 選択中の worktree のブランチに対応するプルリクエストページを
    /// デフォルトの Web ブラウザで開く。
    pub fn open_pr_in_browser(&mut self) {
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status("No worktree selected.".to_string(), StatusLevel::Warning);
            return;
        }

        match crate::git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.pr_url_for_branch(&branch) {
                Some(url) => {
                    log::info!("Opening PR URL: {url}");
                    if let Err(e) = open::that(&url) {
                        self.set_status(format!("Failed to open browser: {e}"), StatusLevel::Error);
                    } else {
                        self.set_status(format!("Opened PR for '{branch}'"), StatusLevel::Success);
                    }
                }
                None => {
                    self.set_status(
                        "Could not determine remote URL.".to_string(),
                        StatusLevel::Error,
                    );
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }
}
