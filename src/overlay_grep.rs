//! grep 検索オーバーレイのインクリメンタル検索ドライバ。
//!
//! 短いクエリ(3文字以下)は高速な2段階検索を行う — まず最近変更された
//! ファイルを検索し、それと並行してフル検索を走らせてフェーズ1の結果を
//! 置き換える — 一方、長いクエリは最初から単一のフル検索に進む。
//! 入力は200msでデバウンスされ、素早いタイピングでキー入力ごとに検索が
//! 走ることはない。

use std::path::Path;

use crate::grep_search::GrepProgress;
use crate::overlay::GrepSearchOverlay;
use crate::types::{Notice, StatusLevel};

impl GrepSearchOverlay {
    /// オーバーレイを開くときの初期状態に戻す。
    pub fn reset(&mut self) {
        self.query.clear();
        self.clear_results();
        self.cancel();
        self.input_focused = true;
    }

    /// 実行中および予約済みの検索をすべて取り下げる。結果は残す。
    pub fn cancel(&mut self) {
        self.running = false;
        self.bg_op.clear();
        self.bg_op_phase2.clear();
        self.debounce_deadline = None;
        self.phase1_active = false;
    }

    fn clear_results(&mut self) {
        self.result_tree = Default::default();
        self.pending_matches.clear();
        self.selected = 0;
        self.scroll = 0;
    }

    /// デバウンス(200ms)付きでインクリメンタル grep 検索をスケジュールする。
    ///
    /// クエリを変更するキー入力のたびに呼ばれる。締切をセットし、
    /// check_debounce() が締切を過ぎたら実際の検索を発火させる。
    pub fn schedule(&mut self) {
        if self.query.is_empty() {
            self.clear_results();
            self.cancel();
            return;
        }
        self.debounce_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
    }

    /// デバウンスの締切を過ぎたか確認する。過ぎていれば検索を開始する。
    /// 検索を開始した場合 true を返す(呼び出し側は再描画をトリガーすべき)。
    ///
    /// root は検索範囲。結果を選ぶと同じ相対パスで reveal と open をするので、
    /// Viewer が表示しているツリーの根と一致していないと「ヒットしたのに
    /// 開けない」結果が並ぶ。
    pub fn check_debounce(&mut self, root: &Path) -> bool {
        if let Some(deadline) = self.debounce_deadline
            && std::time::Instant::now() >= deadline
        {
            self.debounce_deadline = None;
            self.start(root);
            return true;
        }
        false
    }

    /// 3 文字以下では 2 段階にする。phase1 が最近変更されたファイルだけを速く返し、
    /// 並行して走る phase2 のフル検索が結果を置き換える。
    fn start(&mut self, root: &Path) {
        let query = self.query.text().to_string();
        if query.is_empty() {
            return;
        }

        self.cancel();
        self.clear_results();
        self.running = true;

        let regex_mode = self.regex_mode;
        let case_sensitive = self.case_sensitive;

        if query.chars().count() <= 3 {
            self.phase1_active = true;

            // 最近変更されたファイルを取得する(同期・高速)。
            let recent_files =
                crate::git_engine::recently_modified_files(root, 200).unwrap_or_default();

            if !recent_files.is_empty() {
                let wt1 = root.to_path_buf();
                let q1 = query.clone();
                self.bg_op.start(move |tx| {
                    crate::grep_search::run_search_files(
                        &wt1,
                        &q1,
                        regex_mode,
                        case_sensitive,
                        recent_files,
                        tx,
                    );
                });
            }

            let wt2 = root.to_path_buf();
            let q2 = query;
            self.bg_op_phase2.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        } else {
            self.phase1_active = false;
            let wt = root.to_path_buf();
            self.bg_op.start(move |tx| {
                crate::grep_search::run_search(&wt, &query, regex_mode, case_sensitive, tx);
            });
        }
    }

    /// バックグラウンドの grep 検索結果をポーリングする。
    /// ステータスバーへ出すべき通知があれば返す。
    pub fn poll(&mut self) -> Option<Notice> {
        let mut tree_dirty = false;
        let mut notice = None;

        // phase1 / 単一段階の bg_op をポーリングする。
        for msg in self.bg_op.poll_all() {
            match msg {
                GrepProgress::Results(batch) => {
                    self.pending_matches.extend(batch);
                    tree_dirty = true;
                }
                GrepProgress::Done(total) => {
                    self.bg_op.clear();
                    // phase1 は完了したが phase2 がまだ実行中なら running = true のままにする。
                    if !self.phase1_active || !self.bg_op_phase2.is_running() {
                        self.running = false;
                        notice = truncation_notice(total);
                    }
                }
                GrepProgress::Error(msg) => {
                    self.running = false;
                    self.bg_op.clear();
                    self.rebuild_tree_if(tree_dirty);
                    return Some((format!("Search error: {msg}"), StatusLevel::Error));
                }
            }
        }

        if self.phase1_active {
            let mut got_phase2_results = false;
            for msg in self.bg_op_phase2.poll_all() {
                match msg {
                    GrepProgress::Results(batch) => {
                        if !got_phase2_results {
                            // phase1 の結果を phase2 の結果で置き換える。
                            self.pending_matches.clear();
                            self.selected = 0;
                            self.scroll = 0;
                            self.phase1_active = false;
                            got_phase2_results = true;
                        }
                        self.pending_matches.extend(batch);
                        tree_dirty = true;
                    }
                    GrepProgress::Done(total) => {
                        self.phase1_active = false;
                        self.running = false;
                        self.bg_op_phase2.clear();
                        notice = truncation_notice(total).or(notice);
                    }
                    GrepProgress::Error(msg) => {
                        self.phase1_active = false;
                        self.running = false;
                        self.bg_op_phase2.clear();
                        self.rebuild_tree_if(tree_dirty);
                        return Some((format!("Search error: {msg}"), StatusLevel::Error));
                    }
                }
            }
        }

        self.rebuild_tree_if(tree_dirty);
        notice
    }

    fn rebuild_tree_if(&mut self, dirty: bool) {
        if dirty {
            self.result_tree =
                crate::search_result_tree::SearchResultTree::build(&self.pending_matches);
        }
    }
}

fn truncation_notice(total: usize) -> Option<Notice> {
    (total >= 5000).then(|| {
        (
            format!("Search truncated at {total} results."),
            StatusLevel::Warning,
        )
    })
}
