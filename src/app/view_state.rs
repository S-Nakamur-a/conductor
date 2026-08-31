//! Viewer/diffのリフレッシュと、worktreeブランチごとに永続化される
//! 「ユーザーがどこを見ていたか」のビュー状態（開いていたファイル + スクロール位置）。

use super::{App, PendingViewRestore, StatusLevel};
use crate::types::Focus;

impl App {
    /// 現在選択中の worktree の Viewer ファイルツリーを再読み込みする。開いているファイルと
    /// スクロール位置は保持する。表示エントリが変わった場合は true。
    ///
    /// [Self::selected_worktree_path] は worktree が無いとき repo_path に落ちるので、
    /// 非 git ディレクトリでも Explorer はカレントフォルダを表示し続ける。
    pub fn refresh_viewer(&mut self) -> bool {
        let path = self.selected_worktree_path();
        let tab_width = self.config.viewer.tab_width;
        let reload = self
            .explorer
            .load_file_tree(&path, self.viewer.content.current_file.as_deref());
        if reload.root_changed {
            self.viewer
                .prune_tabs_to_root(self.explorer.root(), tab_width);
        }
        if let Some(rel) = &reload.reopen {
            self.viewer
                .reload_active_file(self.explorer.root(), rel, tab_width);
        }
        // 同期のツリー読み込み経路なので、保留中のファイルはここで再度開く。非同期の
        // worktree 切り替え経路では poll_worktree_switch_ops が行う。
        self.consume_pending_view_restore();
        self.rehighlight_viewer();
        reload.entries_changed
    }

    /// 以前選択していた worktree と、保存済みビュー (開いていたファイル + スクロール) を
    /// 復元する。何も永続化されていないときに呼んでも安全。
    ///
    /// worktree リストは [App::refresh_worktrees] が同期的に埋めてあるので、選択の復元に
    /// フレームのちらつきは生じない。ファイル自体はツリー読み込み後に遅延復元される。
    pub fn restore_selected_worktree_and_view(&mut self) {
        let saved_branch = self
            .review_store
            .as_ref()
            .and_then(|s| s.get_selected_worktree().ok().flatten());
        if let Some(branch) = saved_branch
            && let Some(idx) = self.worktrees.iter().position(|w| w.branch == branch)
        {
            self.worktrees.select(idx);
        }

        self.rebuild_worktree_list_rows();
        let sel = self.worktrees.selected_index();
        if let Some(pos) = self
            .worktrees
            .rows
            .iter()
            .position(|r| matches!(r, super::WorktreeListRow::Worktree(i) if *i == sel))
        {
            self.worktrees.row_selected = pos;
        }

        let branch = self.selected_worktree_branch();
        self.view_restore.pending = None;
        if branch.is_empty() {
            self.view_restore.current_branch = None;
            return;
        }
        self.view_restore.current_branch = Some(branch.clone());
        if let Some(store) = &self.review_store
            && let Ok(Some((Some(file), line))) = store.get_view_state(&branch)
        {
            self.view_restore.pending = Some(PendingViewRestore {
                file,
                scroll: line.max(0) as usize,
            });
        }
    }

    /// branch のビュー (開いていたファイル + スクロール) を永続化する。
    ///
    /// まだ復元待ちなら未消費の保留値をそのまま書き戻す。保存済みの状態を空のビューで
    /// 上書きしないため。
    pub(crate) fn save_view_for(&self, branch: &str) {
        let Some(store) = &self.review_store else {
            return;
        };
        let (file, line) = match &self.view_restore.pending {
            Some(r) => (Some(r.file.clone()), r.scroll as i64),
            None => (
                self.viewer.content.current_file.clone(),
                self.viewer.content.file_scroll as i64,
            ),
        };
        let _ = store.save_view_state(branch, file.as_deref(), line);
    }

    /// 現在のworktreeのビューと選択を保存する。終了/再起動前、および
    /// リポジトリ切り替え前に呼ばれる。
    pub fn persist_view_state(&self) {
        if let Some(branch) = &self.view_restore.current_branch {
            self.save_view_for(branch);
            if let Some(store) = &self.review_store {
                let _ = store.set_selected_worktree(branch);
            }
        }
    }

    /// 一度きりの [PendingViewRestore] を消費し、保存済みのファイルを保存済みの行まで
    /// スクロールして開く。スクロール先はファイル長でクランプするので、縮小された
    /// ファイルで Viewer が空白のままにならない。
    pub fn consume_pending_view_restore(&mut self) {
        let Some(restore) = self.view_restore.pending.take() else {
            return;
        };
        match restore_disposition(
            self.viewer.content.current_file.is_some(),
            self.viewer.is_summary(),
        ) {
            RestoreDisposition::Apply => {}
            RestoreDisposition::Drop => return,
            RestoreDisposition::Keep => {
                self.view_restore.pending = Some(restore);
                return;
            }
        }
        // 復元先の存在確認は Viewer の根で行う。同期・非同期どちらの経路からも呼ばれるので、
        // そのツリーと同じ根で見ないと確認と実際に開く先がずれる。
        if !self.explorer.root().join(&restore.file).is_file() {
            return;
        }
        let tab_width = self.config.viewer.tab_width;
        self.viewer
            .open_file(self.explorer.root(), &restore.file, tab_width);
        let max = self.viewer.content.file_content.len().saturating_sub(1);
        self.viewer.content.file_scroll = restore.scroll.min(max);
    }

    pub fn rehighlight_viewer(&mut self) {
        let syntax_set = &self.appearance.highlight.syntax_set;
        let theme = &self.appearance.highlight.theme;
        let generation = self.appearance.highlight.generation;
        self.viewer.highlight_content(syntax_set, theme, generation);
    }

    /// branch の diff を計算すべき対象 ref。
    ///
    /// diff を計算するすべての経路 (refresh_diff と worktree 切り替え時のバックグラウンド
    /// 計算) はここを通る。別々に決めると、同じ worktree が切り替え直後と次のリフレッシュ後で
    /// 違うファイル一覧を表示する。
    pub(crate) fn diff_base_for(&self, branch: &str) -> String {
        // PR レビュー用の worktree は main 以外を対象にすることがある。intake 時に記録された
        // base ref を優先し、保存されていない場合だけ main_branch へ落ちる。
        let saved_base = self
            .review_store
            .as_ref()
            .and_then(|store| store.get_worktree_base_branch(branch).ok().flatten());
        resolve_diff_base_branch(saved_base, &self.config.general.main_branch)
    }

    /// 現在選択中のworktreeについて、解決済みのbase refに対するdiffを
    /// 読み込む（または再読み込みする）。
    pub fn refresh_diff(&mut self) {
        let word_diff = self.config.diff.word_diff;
        if let Some(wt) = self.worktrees.selected() {
            let path = wt.path.clone();
            let base_branch = self.diff_base_for(&wt.branch);
            let tab_width = self.config.viewer.tab_width;
            self.diff_state
                .load_diff(&path, &base_branch, word_diff, tab_width);
            self.viewer.invalidate_diff_annotations();
        }
    }

    /// HEAD oid とステータス件数を前回の既知値と比較し、変化が検出されたときだけコストの高い
    /// refresh_diff() / refresh_viewer() を呼ぶ。refresh_worktrees() の後に呼ばれ、その副作用で
    /// 両方の値は取得済み。
    pub fn check_diff_viewer_staleness(&mut self) {
        let wt = match self.worktrees.selected() {
            Some(wt) => wt,
            None => return,
        };

        let current_head = self.change_watch.heads.get(&wt.branch).cloned();
        // staged を含めているのは git add / git reset を可視化するため。他の 3 つはインデックスを
        // 先に見て 1 ファイル 1 バケットで数えるのでステージしても値が変わらず、ファイル
        // ウォッチャーも .git/ を無視する。無ければステージ色は無関係な編集でしか更新されない。
        let current_status = (wt.added, wt.modified, wt.deleted, wt.staged);

        if self.change_watch.record(current_head, current_status) {
            log::debug!("Change detected for worktree '{}'", wt.branch);
            self.refresh_diff();
            self.refresh_viewer();
        }
    }

    /// Viewer を生の Markdown ソースとレンダリング済み表示の間で切り替える。プレーン表示中の
    /// Markdown でのみ意味を持ち、それ以外ではヒントを出す — ヘッダーのトグルがまさにその
    /// 場面で隠れているので、見えないモードを黙ってラッチしない。
    pub fn cmd_fold_one_level(&mut self) {
        let depth = self.viewer.fold_collapse_deepest();
        self.report_fold_depth(depth);
    }

    /// 深さ単位の畳み込みを1段開き戻す（zr）。
    pub fn cmd_unfold_one_level(&mut self) {
        let depth = self.viewer.fold_expand_shallowest();
        self.report_fold_depth(depth);
    }

    /// すべて畳む（zM）。
    pub fn cmd_fold_all(&mut self) {
        self.viewer.fold_close_all();
    }

    /// すべて開く（zR）。
    pub fn cmd_unfold_all(&mut self) {
        self.viewer.fold_open_all();
    }

    /// 畳んだ段数は畳んだ跡からは読み取れないので、操作のたびに知らせる。
    fn report_fold_depth(&mut self, depth: Option<crate::viewer::FoldDepth>) {
        if let Some(depth) = depth {
            self.set_status_info(format!("Fold level {}/{}", depth.level, depth.max));
        }
    }

    pub fn cmd_toggle_markdown_render(&mut self) {
        if !self.viewer.markdown_toggle_available() {
            self.set_status(
                "Raw/Rendered applies to a markdown file in the Viewer".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        self.viewer.toggle_markdown_rendered();
        let msg = if self.viewer.md_rendered {
            "Markdown: Rendered"
        } else {
            "Markdown: Raw"
        };
        self.set_status(msg.to_string(), StatusLevel::Info);
    }

    pub fn next_viewer_tab(&mut self) {
        let tab_width = self.config.viewer.tab_width;
        self.viewer.next_tab(self.explorer.root(), tab_width);
        self.after_viewer_file_change();
    }

    pub fn prev_viewer_tab(&mut self) {
        let tab_width = self.config.viewer.tab_width;
        self.viewer.prev_tab(self.explorer.root(), tab_width);
        self.after_viewer_file_change();
    }

    pub fn focus_viewer_tab(&mut self, idx: usize) {
        let tab_width = self.config.viewer.tab_width;
        self.viewer.focus_tab(self.explorer.root(), idx, tab_width);
        self.after_viewer_file_change();
    }

    /// idx のタブを閉じる。省略時はアクティブなタブ。
    pub fn close_viewer_tab(&mut self, idx: Option<usize>) {
        let tab_width = self.config.viewer.tab_width;
        let idx = idx.unwrap_or(self.viewer.active_tab);
        let Some(closed) = self.viewer.tabs.get(idx).map(|t| t.path.clone()) else {
            return;
        };
        self.viewer.close_tab(self.explorer.root(), idx, tab_width);
        self.after_viewer_file_change();
        self.set_status(format!("Closed {closed}"), StatusLevel::Info);
    }

    /// Viewer にファイルを出す唯一の入口。
    ///
    /// 開いたあとに貼り直すものを呼ぶ側に覚えさせない。8 箇所が別々の手順を持つと、
    /// コードジャンプと grep の飛び先がコメントのキャッシュを貼り直さず、行番号だけが
    /// 一致した前のファイルの印を出す。
    pub fn show_file(&mut self, relative_path: &str, how: OpenAs) {
        let tab_width = self.config.viewer.tab_width;
        let root = self.explorer.root().to_path_buf();
        match how {
            OpenAs::Preview => self
                .viewer
                .open_file_preview(&root, relative_path, tab_width),
            OpenAs::Persistent => self.viewer.open_file(&root, relative_path, tab_width),
        }
        self.after_viewer_file_change();
    }

    /// 開いているファイルに紐づくキャッシュを貼り直す。タブの切り替えも同じ経路。
    fn after_viewer_file_change(&mut self) {
        self.rehighlight_viewer();
        if let Some(path) = self.viewer.content.current_file.clone() {
            self.review_state.build_file_comment_cache(&path);
            self.explorer.reveal_file_in_tree(&path);
        }
    }

    /// Viewer で開いたうえで、行へ寄せ、フォーカスを移し、開いたことを知らせる。
    pub fn open_file_in_viewer(&mut self, relative_path: &str, line: Option<usize>) {
        self.show_file(relative_path, OpenAs::Persistent);

        if let Some(ln) = line {
            let max = self.viewer.content.file_content.len().saturating_sub(1);
            self.viewer.content.file_scroll = (ln.saturating_sub(1)).min(max);
            self.viewer.show_raw_for_line_target();
        }

        self.set_focus(Focus::Viewer);

        let msg = if let Some(ln) = line {
            format!("Opened {relative_path}:{ln} in Viewer")
        } else {
            format!("Opened {relative_path} in Viewer")
        };
        self.set_status(msg, StatusLevel::Success);
    }
}

/// [App::show_file] が開いたタブを残すかどうか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAs {
    /// 明示的に閉じるまで残る。
    Persistent,
    /// ちょっと見るだけ。フォーカスが外れると閉じる。
    Preview,
}

/// 期限が来た保留中の [PendingViewRestore] をどう扱うか。
#[derive(Debug, PartialEq, Eq)]
enum RestoreDisposition {
    /// 何も表示されていない — 意図どおり保存済みのファイルを開く。
    Apply,
    /// 走査完了までの隙間でユーザーが実際のファイルを開いた場合。保持すると
    /// [App::save_view_for] が古びた保留パスを永続化してしまう。
    Drop,
    /// SUMMARY 疑似ファイルだけが表示されている場合。上書きはしないが保留状態は保つ —
    /// ビュー状態のスキーマに「SUMMARY を見ていた」が無いので、破棄すると空のビューが
    /// 永続化されて保存済みファイルを失う。Viewer が再び空になれば復元は成立し得る。
    Keep,
}

/// どちらの誤答も静かに起きる。Keep すべきを Drop すると保存済みファイルを黙って
/// 消し、Drop すべきを Keep すると古い値で固定する。真理値表はテストで固定してある。
fn restore_disposition(has_open_file: bool, showing_summary: bool) -> RestoreDisposition {
    if has_open_file {
        RestoreDisposition::Drop
    } else if showing_summary {
        RestoreDisposition::Keep
    } else {
        RestoreDisposition::Apply
    }
}

/// PR intake が記録した base ref を優先する。PR は main 以外 (release/develop など) を
/// 対象にすることがあるため。main_branch は保存済み base が無いときだけ使う。
fn resolve_diff_base_branch(saved_base: Option<String>, main_branch: &str) -> String {
    saved_base.unwrap_or_else(|| main_branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_diff_base_branch_prefers_saved_base_over_main() {
        assert_eq!(
            resolve_diff_base_branch(Some("release/1.0".to_string()), "main"),
            "release/1.0"
        );
    }

    #[test]
    fn resolve_diff_base_branch_falls_back_to_main_when_unsaved() {
        assert_eq!(resolve_diff_base_branch(None, "main"), "main");
    }

    /// 完全な真理値表。起動時と worktree 切り替えはどちらも復元前に Viewer をリセットするので
    /// Apply が通常経路。残り 2 行はツリー走査中にユーザーが先に動いた場合にのみ発生する。
    #[test]
    fn restore_disposition_truth_table() {
        use RestoreDisposition::*;
        assert_eq!(restore_disposition(false, false), Apply);
        // SUMMARY だけが開いている: 上書きはしないが、後の save で保存済みファイルが消されない
        // よう保留状態を保つ。
        assert_eq!(restore_disposition(false, true), Keep);
        // 実ファイルが開いている: 保存済みビューはいずれにせよ古い。破棄することで、永続化は
        // ユーザーが実際に開いたものを追跡し続ける。
        assert_eq!(restore_disposition(true, false), Drop);
        assert_eq!(restore_disposition(true, true), Drop);
    }
}
