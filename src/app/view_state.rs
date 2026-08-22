//! Viewer/diffのリフレッシュと、worktreeブランチごとに永続化される
//! 「ユーザーがどこを見ていたか」のビュー状態（開いていたファイル + スクロール位置）。

use super::focus::Focus;
use super::{App, PendingViewRestore, StatusLevel};

impl App {
    /// 現在選択中のworktreeのViewerファイルツリーを再読み込みする。
    ///
    /// 現在開いているファイルとスクロール位置を保持するので、ファイル
    /// ウォッチャーによるリフレッシュがユーザーの表示を乱すことはない。
    ///
    /// ファイルツリーの表示エントリが変わった場合は true を返す。
    /// [Self::selected_worktree_path] を使う。これはworktreeが無いとき
    /// repo_path にフォールバックするので、非gitディレクトリでもExplorerは
    /// カレントフォルダの内容を表示し続ける。
    pub fn refresh_viewer(&mut self) -> bool {
        let path = self.selected_worktree_path();
        let tab_width = self.config.viewer.tab_width;
        let changed = self.viewer_state.load_file_tree(&path, tab_width);
        // 起動時の復元: これは遅延（同期的な）ツリー読み込み経路
        // （例えばViewerが初めてフォーカスされたときなど）なので、保留中の
        // ファイルがあればここで再度開く。非同期のworktree切り替え経路では
        // poll_worktree_switch_ops がこれを行う。
        self.consume_pending_view_restore();
        self.rehighlight_viewer();
        changed
    }

    /// 以前選択していたworktreeを復元し、現在のリポジトリの保存済みビュー
    /// （開いていたファイル + スクロール）を仕込む。何も永続化されていない
    /// ときに呼んでも安全 — デフォルトのままになるだけ。
    ///
    /// 起動時とリポジトリ切り替え時に使われる。worktreeリストは
    /// [App::refresh_worktrees] によってすでに同期的に埋まっているので、
    /// 選択の復元にフレームのちらつきは生じない。ファイル自体は、そのツリーが
    /// 読み込まれた後に遅延復元される（[App::consume_pending_view_restore] 参照）。
    pub fn restore_selected_worktree_and_view(&mut self) {
        // 選択されていたworktreeを復元する（見つからなければ現在のまま）。
        let saved_branch = self
            .review_store
            .as_ref()
            .and_then(|s| s.get_selected_worktree().ok().flatten());
        if let Some(branch) = saved_branch
            && let Some(idx) = self.worktrees.iter().position(|w| w.branch == branch)
        {
            self.worktrees.select(idx);
        }

        // worktreeリストのカーソルを復元したworktreeへ合わせる。
        self.rebuild_worktree_list_rows();
        let sel = self.worktrees.selected_index();
        if let Some(pos) = self
            .worktrees.rows
            .iter()
            .position(|r| matches!(r, super::WorktreeListRow::Worktree(i) if *i == sel))
        {
            self.worktrees.row_selected = pos;
        }

        // 読み込んだworktreeを記録し、保存済みのファイル/スクロールを仕込む。
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

    /// branch のメモリ上のビュー（開いていたファイル + スクロール）を永続化する。
    ///
    /// まだ復元待ちの場合（このセッションでこのworktreeのViewerを一度も
    /// 開いていない場合）、未消費の保留値をそのまま書き戻すことで、保存済みの
    /// 状態を空のビューで上書きしないようにする。
    pub(super) fn save_view_for(&self, branch: &str) {
        let Some(store) = &self.review_store else {
            return;
        };
        let (file, line) = match &self.view_restore.pending {
            Some(r) => (Some(r.file.clone()), r.scroll as i64),
            None => (
                self.viewer_state.content.current_file.clone(),
                self.viewer_state.content.file_scroll as i64,
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

    /// 一度きりの [PendingViewRestore] を消費する: 保存済みのファイルを開き、
    /// 保存済みの行までスクロールする。保留中のものが無い、またはファイルが
    /// もう存在しない場合は no-op。スクロール先はファイル長でクランプされるので、
    /// 縮小されたファイルでViewerが空白のままにならない。
    pub fn consume_pending_view_restore(&mut self) {
        let Some(restore) = self.view_restore.pending.take() else {
            return;
        };
        match restore_disposition(
            self.viewer_state.content.current_file.is_some(),
            self.viewer_state.is_summary(),
        ) {
            RestoreDisposition::Apply => {}
            RestoreDisposition::Drop => return,
            RestoreDisposition::Keep => {
                self.view_restore.pending = Some(restore);
                return;
            }
        }
        // 復元先の存在確認は Viewer の根で行う。ここは「ツリーが揃った直後」に
        // 呼ばれる (同期の refresh_viewer と、非同期の worktree 切り替えの両方)
        // ので、そのツリーと同じ根で見ないと確認と実際に開く先がずれる。
        if !self.viewer_state.root().join(&restore.file).is_file() {
            return;
        }
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&restore.file, tab_width);
        let max = self.viewer_state.content.file_content.len().saturating_sub(1);
        self.viewer_state.content.file_scroll = restore.scroll.min(max);
    }

    /// 現在読み込まれているファイル内容にsyntectハイライトを実行する。
    pub fn rehighlight_viewer(&mut self) {
        // borrow checkerを満たすため、フィールドを分離して借用する。
        let syntax_set = &self.highlight.syntax_set;
        let theme = &self.highlight.theme;
        let generation = self.highlight.generation;
        self.viewer_state
            .highlight_content(syntax_set, theme, generation);
    }

    /// branch のdiffを計算すべき対象ref。
    ///
    /// diffを計算するすべての経路はここを通らなければならない。経路は2つある
    /// — refresh_diff から呼ばれるこれと、worktree切り替え時のバックグラウンド
    /// 計算 — かつては両者が異なる基準でbaseを決めていたため、同じworktreeが
    /// 切り替え直後は片方のファイル一覧を、次のリフレッシュ後は別のファイル
    /// 一覧を表示するということが起きていた。決定ロジックを単一のメソッドに
    /// 保つことで、この不具合が静かにぶり返すのを防いでいる。
    pub(super) fn diff_base_for(&self, branch: &str) -> String {
        // PRレビュー用のworktreeは、設定されたmainブランチ以外を対象にすることが
        // ある（例: release/developブランチ）。intake時に記録されたbase refを
        // 優先し、保存されていない場合（通常のworktreeやDBが使えない場合）のみ
        // main_branchへフォールバックする。
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
            self.viewer_state.invalidate_diff_annotations();
        }
    }

    /// Viewerを、生のMarkdownソースとレンダリング済みの文章表示の間で切り替える。
    ///
    /// プレーンファイル表示中のMarkdownファイルでのみ意味を持つ。それ以外の
    /// 場面ではヒントをフラッシュ表示する。ユーザーから見えないモードを黙って
    /// ラッチするのではなく、というのもヘッダーのトグルはまさにそうした場面で
    /// 隠れているからである。
    pub fn cmd_toggle_markdown_render(&mut self) {
        if !self.viewer_state.markdown_toggle_available() {
            self.set_status(
                "Raw/Rendered applies to a markdown file in the Viewer".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        self.viewer_state.toggle_markdown_rendered();
        let msg = if self.viewer_state.md_rendered {
            "Markdown: Rendered"
        } else {
            "Markdown: Raw"
        };
        self.set_status(msg.to_string(), StatusLevel::Info);
    }

    /// Viewer の次のタブへ切り替える。
    pub fn next_viewer_tab(&mut self) {
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.next_tab(tab_width);
        self.after_viewer_tab_change();
    }

    /// Viewer の前のタブへ切り替える。
    pub fn prev_viewer_tab(&mut self) {
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.prev_tab(tab_width);
        self.after_viewer_tab_change();
    }

    /// idx のタブをアクティブにする（タブ行のクリック）。
    pub fn focus_viewer_tab(&mut self, idx: usize) {
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.focus_tab(idx, tab_width);
        self.after_viewer_tab_change();
    }

    /// idx のタブを閉じる。省略時はアクティブなタブ。
    pub fn close_viewer_tab(&mut self, idx: Option<usize>) {
        let tab_width = self.config.viewer.tab_width;
        let idx = idx.unwrap_or(self.viewer_state.active_tab);
        let Some(closed) = self.viewer_state.tabs.get(idx).map(|t| t.path.clone()) else {
            return;
        };
        self.viewer_state.close_tab(idx, tab_width);
        self.after_viewer_tab_change();
        self.set_status(format!("Closed {closed}"), StatusLevel::Info);
    }

    /// タブが切り替わったあと、ファイルに紐づくキャッシュを新しいファイルへ貼り直す。
    /// ハイライトもコメントも「今開いているファイル」が前提なので、貼り直しを
    /// 忘れると前のタブの色とコメントがそのまま残る。
    fn after_viewer_tab_change(&mut self) {
        self.rehighlight_viewer();
        if let Some(path) = self.viewer_state.content.current_file.clone() {
            self.review_state.build_file_comment_cache(&path);
            self.viewer_state.reveal_file_in_tree(&path);
        }
    }

    /// ファイルパス（現在のworktreeからの相対パス）をViewerパネルで開く。
    ///
    /// 指定があれば line（1始まり）へジャンプする。Explorerツリー内で
    /// ファイルを表示し、フォーカスをViewerへ切り替え、ステータスメッセージを
    /// 表示する。
    pub fn open_file_in_viewer(&mut self, relative_path: &str, line: Option<usize>) {
        let tab_width = self.config.viewer.tab_width;

        self.viewer_state.open_file(relative_path, tab_width);
        self.viewer_state.reveal_file_in_tree(relative_path);

        if let Some(ln) = line {
            let max = self
                .viewer_state
                .content
                .file_content
                .len()
                .saturating_sub(1);
            self.viewer_state.content.file_scroll = (ln.saturating_sub(1)).min(max);
            self.viewer_state.show_raw_for_line_target();
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

/// 期限が来た保留中の [PendingViewRestore] をどう扱うか。
#[derive(Debug, PartialEq, Eq)]
enum RestoreDisposition {
    /// 何も表示されていない — 意図どおり保存済みのファイルを開く。
    Apply,
    /// worktree切り替えとツリーの走査完了の間の隙間で、ユーザーが実際の
    /// ファイルを開いた場合。保存済みのビューはもう古いので破棄する。
    /// 保持したままにすると、[App::save_view_for] がユーザーが最終的に
    /// 開いたファイルではなく、古びた保留パスを永続化してしまう。
    Drop,
    /// SUMMARY疑似ファイルだけが表示されていて、背後に実ファイルが無い場合。
    /// それを上書きして開くことはしないが、保留状態は保つ: ビュー状態の
    /// スキーマには「SUMMARYを見ていた」を表す方法が無いので、ここで破棄すると
    /// 空のビューが永続化されて保存済みファイルを完全に失ってしまう。呼び出し側は
    /// この後もconsumeのたびに再実行するので、Viewerが再び空になれば復元は
    /// 成立し得る。
    Keep,
}

/// Viewerがいま何を表示しているかから、期限が来たビュー復元の運命を決める。
///
/// [App::consume_pending_view_restore] から切り出しているのは、どちらの
/// 誤答も静かに起きるため: Keep であるべきところで Drop すると
/// ブランチの保存済みファイルを黙って消してしまい、Drop であるべき
/// ところで Keep すると古い値のまま黙って固定してしまう。どちらも
/// クラッシュとして表面化しないので、この真理値表はテストで固定している。
fn restore_disposition(has_open_file: bool, showing_summary: bool) -> RestoreDisposition {
    if has_open_file {
        RestoreDisposition::Drop
    } else if showing_summary {
        RestoreDisposition::Keep
    } else {
        RestoreDisposition::Apply
    }
}

/// diffを計算すべき対象のbaseブランチを解決する: worktreeの保存済み
/// base ref（PR intake時に記録される — save_worktree_base_branch 参照）が
/// 優先される。PRは設定されたmainブランチ以外（例: release/develop）を
/// 対象にすることがあるため。main_branch は、保存済みbaseが無いworktree
/// （通常のworktree、またはレビューDBが使えない場合）のフォールバックとして
/// のみ使われる。
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

    /// 完全な真理値表。起動時とworktree切り替えはどちらも、復元をセットする前に
    /// Viewerをリセットするので Apply が通常経路である。残り2行は、ツリー
    /// 走査中にユーザーが先に動いた場合にのみ発生する。
    #[test]
    fn restore_disposition_truth_table() {
        use RestoreDisposition::*;
        // Viewerが空: 復元が仕事をする。
        assert_eq!(restore_disposition(false, false), Apply);
        // SUMMARYだけが開いている: 上書きはしないが、後のsaveでブランチの
        // 保存済みファイルが消されないよう保留状態を保つ。
        assert_eq!(restore_disposition(false, true), Keep);
        // 実ファイルが開いている: 保存済みビューはいずれにせよ古い。SUMMARYが
        // そのファイルの上に重なっている場合も含む — 破棄することで、永続化は
        // ユーザーが実際に開いたものを追跡し続ける。
        assert_eq!(restore_disposition(true, false), Drop);
        assert_eq!(restore_disposition(true, true), Drop);
    }
}
