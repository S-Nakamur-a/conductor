//! [App] の Explorer walkthrough ビュー用メソッド: ステップ選択、diff ペイン内の
//! ステップ位置へのジャンプ、「viewed」ファイルのトグル。これらは Explorer の
//! Walkthrough ボトムペインビュー(viewer::ExplorerBottomView を参照)と、
//! diff リストのファイルごとの viewed マークを支えている。

use super::*;

impl App {
    /// walkthrough ステップのカーソルを delta 行分動かす(j/k)。ステップ一覧の
    /// 範囲でクランプする。選択のみでジャンプはしない(n/N とは異なる)。
    pub fn walkthrough_move(&mut self, delta: isize) {
        let Some(len) = self
            .walkthrough.current
            .as_ref()
            .map(|wt| wt.steps.len())
        else {
            return;
        };
        if len == 0 {
            return;
        }
        let cur = self.viewer_state.explorer.walkthrough_selected as isize;
        self.viewer_state.explorer.walkthrough_selected =
            (cur + delta).clamp(0, len as isize - 1) as usize;
    }

    /// 現在選択中の walkthrough ステップへジャンプする(Enter): そのファイルを
    /// diff ペインで開き、フォーカスを Viewer へ移す。diff リスト自体の Enter
    /// ハンドリングと同じ挙動にして、両ビューの一貫性を保つ。
    pub fn walkthrough_jump_selected(&mut self) {
        let idx = self.viewer_state.explorer.walkthrough_selected;
        if !self.jump_to_walkthrough_step(idx) {
            return;
        }
        self.set_focus(Focus::Viewer);
    }

    /// walkthrough の選択を delta 分動かし、即座にジャンプする(Walkthrough
    /// ビューがフォーカスされているときの n/N)。walkthrough_jump_selected
    /// とは異なり Walkthrough ビューに留まるため、繰り返し押すことで diff
    /// ペインにキーボードフォーカスを奪われずにステップをページ送りできる。
    pub fn walkthrough_step(&mut self, delta: isize) {
        let Some(len) = self
            .walkthrough.current
            .as_ref()
            .map(|wt| wt.steps.len())
        else {
            return;
        };
        if len == 0 {
            return;
        }
        let cur = self.viewer_state.explorer.walkthrough_selected as isize;
        let next = (cur + delta).clamp(0, len as isize - 1) as usize;
        // ジャンプが成功してもしなくてもカーソルは動かす。そうしないと、ファイルを
        // 解決できないステップで n/N が固定されてしまう — 押すたびに同じ解決
        // できないステップを再試行し、レビュアーはツアーの残りに辿り着けなくなる。
        self.viewer_state.explorer.walkthrough_selected = next;
        self.jump_to_walkthrough_step(next);
    }

    /// walkthrough ステップ idx へジャンプする共通実装: そのファイルを diff に
    /// 対して解決し、diff ペインで開き、開始行までスクロールし、viewed として
    /// マークする。walkthrough が存在しない、インデックスが範囲外、あるいは
    /// ステップのファイルが本当に現在の diff に含まれない場合は false を返す
    /// (それ以外は何もしない)。
    ///
    /// ステップのパスは*緩やかに*マッチさせる([crate::diff_state::DiffState::resolve_changed_path]
    /// を参照)。これは言語モデルが書いたものであり、diff リストは文字列の完全一致で
    /// 照合するためで、./src/a.rs と書かれたステップはリストに実在するファイルを
    /// 指しているにも関わらず、以前は見つからないと報告されていた。
    fn jump_to_walkthrough_step(&mut self, idx: usize) -> bool {
        let Some(steps) = self.walkthrough.current.as_ref().map(|wt| &wt.steps) else {
            return false;
        };
        let Some(step) = steps.get(idx) else {
            return false;
        };
        let step_path = step.file_path.clone();
        let line_start = step.line_start;
        let step_id = step.id.clone();
        let Some(wt) = self.worktrees.selected() else {
            return false;
        };
        let wt_path = wt.path.clone();

        // ステップ側のパス表記と diff 側のそれは異なることがある(パス正規化前に
        // 保存された古い行、git diff の a//b/ プレフィックスなど)。
        // resolve_changed_path は diff 側自身の表記を返す。
        let Some(file_path) = self.diff_state.resolve_changed_path(&step_path) else {
            // ステータスバーの省略で一番役に立たない部分が最後に削られるよう
            // 順序を決めている: 検索対象を先に、(場合によっては非常に長い)
            // ベースディレクトリを最後に置く。ログには常に全文が届く。
            let normalized = crate::repo_path::normalize(&step_path);
            let searched = if normalized == step_path {
                String::new()
            } else {
                format!(" (searched as {normalized})")
            };
            let msg = format!(
                "Walkthrough step's file isn't in this diff: {step_path}{searched} — \
                 {} changed file(s) vs {}, under {}",
                self.diff_state.changed_paths().len(),
                self.diff_state.base_branch,
                wt_path.display(),
            );
            log::warn!("{msg}");
            self.set_status(msg, StatusLevel::Warning);
            return false;
        };
        // 以降のセッションではその表記を採用する: Viewer のステップバナーと
        // 行範囲の下線表示はどちらも current_file == step.file_path を
        // 判定するので、ステップ側の表記のままにしておくと、ジャンプ自体は
        // 正しくても表示は何も出ないことになる。
        if file_path != step_path
            && let Some(steps) = self.walkthrough.current.as_mut().map(|wt| &mut wt.steps)
            && let Some(step) = steps.get_mut(idx)
        {
            log::debug!("walkthrough step {idx}: resolved '{step_path}' to '{file_path}'");
            step.file_path = file_path.clone();
        }

        // 折りたたまれたディレクトリの中にあるファイルは、展開されるまで表示行が
        // 存在しないため、行を探す前に reveal する。
        let Some(file_diff) = self
            .diff_state
            .reveal_path(&file_path)
            .and_then(|i| self.diff_state.resolve_file(i))
        else {
            self.set_status(
                format!("Walkthrough step's file is in the diff but has no row: {file_path}"),
                StatusLevel::Warning,
            );
            return false;
        };
        let file_diff_clone = file_diff.clone();
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&file_path, tab_width);
        self.viewer_state.reveal_file_in_tree(&file_path);
        self.rehighlight_viewer();
        self.review_state.build_file_comment_cache(&file_path);
        self.expand_threads_for_file(&file_path);
        self.viewer_state.build_unified_diff_view(&file_diff_clone);
        if let Some(list_idx) = self.diff_state.display_index_for_path(&file_path) {
            self.viewer_state.explorer.diff_list_selected = list_idx;
        }

        let target = line_start
            .and_then(|line| {
                self.viewer_state.diff_view.diff_view_lines.iter().position(|e| {
                    matches!(e, crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(n), .. } if *n as i64 == line)
                })
            })
            .or_else(|| {
                self.viewer_state.diff_view.diff_view_lines.iter().position(|e| {
                    matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. } if *tag != crate::diff_state::DiffLineTag::Equal)
                })
            });
        if let Some(pos) = target {
            self.viewer_state.diff_view.diff_view_scroll = pos.saturating_sub(3);
        }

        self.viewer_state.explorer.walkthrough_selected = idx;
        // Viewer は今このステップを反映している: バナーと行範囲の下線表示は
        // リストのカーソルではなく walkthrough_viewing に追従するため、この後
        // カーソルだけを動かす j/k では Viewer は乱れない。
        self.viewer_state.explorer.walkthrough_viewing = Some(idx);
        self.viewer_state.explorer.viewed_steps.insert(step_id);
        true
    }

    /// ファイルパスの「viewed」マークをトグルする — diff リストの v キーと
    /// Viewer の diff モードの v キー(セクションC)から使われる。
    pub fn toggle_path_viewed(&mut self, path: &str) {
        let viewed = &mut self.viewer_state.explorer.viewed;
        if !viewed.remove(path) {
            viewed.insert(path.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    /// 手動で保存した walkthrough(headless generator が書き込む内容を模したもの)が、
    /// walkthrough の UI とジャンプロジックが依存する形のまま store を往復すること
    /// を確認する: WalkthroughStep::kind は同じバリアントにパースし直され、行範囲は
    /// Some のまま残り、ファイルパスは DiffState::display_index_for_path
    /// (jump_to_walkthrough_step が diff 中のステップファイルを探すのに使う参照)
    /// を通じて解決される。
    #[test]
    fn saved_walkthrough_round_trips_for_ui_consumption() {
        use crate::diff_state::{DiffListEntry, DiffState, DiffViewMode, FileDiff};
        use crate::review_store::ReviewStore;
        use crate::walkthrough::{NewWalkthroughStep, WalkthroughStatus, WalkthroughStepKind};

        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(&dir.path().join("conductor.db")).unwrap();
        store.begin_walkthrough("feature-x", None).unwrap();
        store
            .save_walkthrough(
                "feature-x",
                "Add feature X",
                "Wires up feature X end to end.",
                &[
                    NewWalkthroughStep {
                        file_path: "src/a.rs".to_string(),
                        line_start: None,
                        line_end: None,
                        kind: WalkthroughStepKind::Intent,
                        title: "Why".to_string(),
                        body: "Motivation.".to_string(),
                    },
                    NewWalkthroughStep {
                        file_path: "src/a.rs".to_string(),
                        line_start: Some(10),
                        line_end: Some(12),
                        kind: WalkthroughStepKind::Core,
                        title: "Core change".to_string(),
                        body: "What changed.".to_string(),
                    },
                ],
            )
            .unwrap();

        let (walkthrough, steps) = store.get_walkthrough("feature-x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Ready);
        assert_eq!(steps.len(), 2);
        let core = &steps[1];
        assert_eq!(core.kind, WalkthroughStepKind::Core);
        assert_eq!(core.line_start, Some(10));
        assert_eq!(core.line_end, Some(12));
        assert!(!core.id.is_empty());

        // ジャンプ経路でのファイル検索: ステップのファイルは
        // jump_to_walkthrough_step が要求するのと全く同じ形で diff リストを
        // 通じて解決できなければならない。
        let mut ds = DiffState::new("main", DiffViewMode::Unified);
        ds.files = vec![FileDiff {
            path: core.file_path.clone(),
            added_lines: 3,
            deleted_lines: 0,
            hunks: Vec::new(),
        }];
        ds.display_list = vec![DiffListEntry::File {
            file_index: 0,
            depth: 0,
        }];
        assert_eq!(ds.display_index_for_path(&core.file_path), Some(0));
    }
}
