//! [App] の grep 検索オーバーレイのためのインクリメンタル grep 検索。
//!
//! 短いクエリ(3文字以下)は高速な2段階検索を行う — まず最近変更された
//! ファイルを検索し、それと並行してフル検索を走らせてフェーズ1の結果を
//! 置き換える — 一方、長いクエリは最初から単一のフル検索に進む。
//! 入力は200msでデバウンスされ、素早いタイピングでキー入力ごとに検索が
//! 走ることはない。

use super::*;
use crate::grep_search::GrepProgress;

impl App {
    /// デバウンス(200ms)付きでインクリメンタル grep 検索をスケジュールする。
    ///
    /// クエリを変更するキー入力のたびに呼ばれる。締切をセットし、
    /// check_grep_debounce() が締切を過ぎたら実際の検索を発火させる。
    pub fn schedule_grep_search(&mut self) {
        let query = self.overlays.grep_search.query.text().to_string();
        if query.is_empty() {
            // 即座に全てクリアする。
            self.overlays.grep_search.result_tree = Default::default();
            self.overlays.grep_search.pending_matches.clear();
            self.overlays.grep_search.selected = 0;
            self.overlays.grep_search.scroll = 0;
            self.overlays.grep_search.running = false;
            self.overlays.grep_search.bg_op.clear();
            self.overlays.grep_search.bg_op_phase2.clear();
            self.overlays.grep_search.debounce_deadline = None;
            self.overlays.grep_search.phase1_active = false;
            return;
        }
        self.overlays.grep_search.debounce_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
    }

    /// デバウンスの締切を過ぎたか確認する。過ぎていれば検索を開始する。
    /// 検索を開始した場合 true を返す(呼び出し側は再描画をトリガーすべき)。
    pub fn check_grep_debounce(&mut self) -> bool {
        if let Some(deadline) = self.overlays.grep_search.debounce_deadline
            && std::time::Instant::now() >= deadline
        {
            self.overlays.grep_search.debounce_deadline = None;
            self.start_incremental_grep_search();
            return true;
        }
        false
    }

    /// インクリメンタル grep 検索を開始する。
    ///
    /// 短いクエリ(3文字以下)では2段階検索を使う:
    ///   phase1 — 最近変更されたファイルだけを検索する(高速)
    ///   phase2 — フル検索(並行して実行し、phase1 の結果を置き換える)
    /// 長いクエリではフル検索だけを実行する。
    fn start_incremental_grep_search(&mut self) {
        let query = self.overlays.grep_search.query.text().to_string();
        if query.is_empty() {
            return;
        }

        // 検索範囲は Viewer が表示しているツリーの根。結果を選ぶと同じ相対パスで
        // reveal と open をするので、別の根を歩くと「ヒットしたのに開けない」
        // 結果が並ぶ。
        let wt_path = self.viewer_state.root().to_path_buf();

        // 以前の検索があればキャンセルする。
        self.overlays.grep_search.bg_op.clear();
        self.overlays.grep_search.bg_op_phase2.clear();

        // 結果をリセットする。
        self.overlays.grep_search.result_tree = Default::default();
        self.overlays.grep_search.pending_matches.clear();
        self.overlays.grep_search.selected = 0;
        self.overlays.grep_search.scroll = 0;
        self.overlays.grep_search.running = true;

        let regex_mode = self.overlays.grep_search.regex_mode;
        let case_sensitive = self.overlays.grep_search.case_sensitive;

        if query.chars().count() <= 3 {
            // 短いクエリには2段階検索を使う。
            self.overlays.grep_search.phase1_active = true;

            // 最近変更されたファイルを取得する(同期・高速)。
            let recent_files =
                crate::git_engine::recently_modified_files(&wt_path, 200).unwrap_or_default();

            // フェーズ1: 最近のファイルだけを検索する。
            if !recent_files.is_empty() {
                let wt1 = wt_path.clone();
                let q1 = query.clone();
                let files1 = recent_files;
                self.overlays.grep_search.bg_op.start(move |tx| {
                    crate::grep_search::run_search_files(
                        &wt1,
                        &q1,
                        regex_mode,
                        case_sensitive,
                        files1,
                        tx,
                    );
                });
            }

            // フェーズ2: フル検索(並行して実行する)。
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.overlays.grep_search.bg_op_phase2.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        } else {
            // 長いクエリには単一段階のフル検索を使う。
            self.overlays.grep_search.phase1_active = false;
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.overlays.grep_search.bg_op.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        }
    }

    /// バックグラウンドの grep 検索結果をポーリングする。
    pub fn poll_grep_search(&mut self) {
        let mut tree_dirty = false;

        // phase1 / 単一段階の bg_op をポーリングする。
        let messages = self.overlays.grep_search.bg_op.poll_all();
        for msg in messages {
            match msg {
                GrepProgress::Results(batch) => {
                    self.overlays.grep_search.pending_matches.extend(batch);
                    tree_dirty = true;
                }
                GrepProgress::Done(total) => {
                    // phase1 は完了したが phase2 がまだ実行中なら running = true のままにする。
                    if !self.overlays.grep_search.phase1_active
                        || !self.overlays.grep_search.bg_op_phase2.is_running()
                    {
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    } else {
                        self.overlays.grep_search.bg_op.clear();
                    }
                }
                GrepProgress::Error(msg) => {
                    self.overlays.grep_search.running = false;
                    self.overlays.grep_search.bg_op.clear();
                    self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                    return;
                }
            }
        }

        // phase2 の bg_op をポーリングする。
        if self.overlays.grep_search.phase1_active {
            let messages2 = self.overlays.grep_search.bg_op_phase2.poll_all();
            let mut got_phase2_results = false;
            for msg in messages2 {
                match msg {
                    GrepProgress::Results(batch) => {
                        if !got_phase2_results {
                            // phase1 の結果を phase2 の結果で置き換える。
                            self.overlays.grep_search.pending_matches.clear();
                            self.overlays.grep_search.selected = 0;
                            self.overlays.grep_search.scroll = 0;
                            self.overlays.grep_search.phase1_active = false;
                            got_phase2_results = true;
                        }
                        self.overlays.grep_search.pending_matches.extend(batch);
                        tree_dirty = true;
                    }
                    GrepProgress::Done(total) => {
                        if !got_phase2_results {
                            self.overlays.grep_search.phase1_active = false;
                        }
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op_phase2.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    }
                    GrepProgress::Error(msg) => {
                        self.overlays.grep_search.phase1_active = false;
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op_phase2.clear();
                        self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                        return;
                    }
                }
            }
        }

        // 新しい結果が届いたらツリーを再構築する。
        if tree_dirty {
            self.overlays.grep_search.result_tree =
                crate::search_result_tree::SearchResultTree::build(
                    &self.overlays.grep_search.pending_matches,
                );
        }
    }
}
