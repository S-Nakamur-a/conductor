//! [App] のレビューコメントの中核処理と diff ナビゲーション。
//!
//! DB からのコメント再読み込み、diff リストからの diff ファイルオープン(その最初の
//! コメントまたは変更箇所へ着地)、変更ファイル間のジャンプ、未解決スレッドの自動展開、
//! 新規コメントの追加を担う。削除は [super::review_delete]、編集/ステータス/返信は
//! [super::review_edit]、テンプレート/履歴のヘルパーは [super::review_history]、
//! revidere の成果物の読み込みと解析の起動は [super::revidere] にある。

use super::*;
use crate::review_store::{Author, CommentKind};

impl App {
    /// 現在選択中の worktree について、DB からレビューコメントを再読み込みし、
    /// revidere の成果物も読み直す。
    pub fn refresh_reviews(&mut self) {
        self.reload_revidere();
        if let Some(store) = &self.review_store {
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // 現在表示中のファイルについて、ファイル単位のキャッシュを再構築する。
            if let Some(file_path) = self.viewer.content.current_file.clone() {
                self.review_state.build_file_comment_cache(&file_path);
            }
            // diff リストの SUMMARY 疑似ファイルを、このブランチに change summary が
            // あるかどうかと同期させる。実際に切り替わったときだけ再構築し、
            // 毎回のリロードで表示リストを乱さないようにする。
            let has_summary = self.review_state.change_summary.is_some();
            if self.diff_state.has_summary != has_summary {
                self.diff_state.has_summary = has_summary;
                self.diff_state.rebuild_display_list();
            }
            // ここには意図的に「summary が消えたのでビューを閉じる」分岐を置かない。
            // データの再読み込みがユーザの開いたビューを勝手に閉じてはならない。ここでの
            // None は「今回の読み込みでは summary が見つからなかった」ことを意味するに
            // 過ぎず、ブランチの解決に失敗した再読み込みも同様に None になる。それで
            // 閉じてしまうと、ユーザは無関係なファイルへ放り出される。summary ペインは
            // 自身の空状態を描画するので、迷子になったビューはそれ自体で状況を説明でき、
            // Esc で閉じられる。
        }
    }

    /// diff リストで現在選択中のファイル(diff_list_selected のエントリ)を Viewer で
    /// 開く。ファイルジャンプ系のキーから共有される。選択エントリがファイルでなければ
    /// 何もしない。
    pub fn open_diff_file_at_selected(&mut self, how: crate::app::OpenAs) {
        let idx = self.explorer.changes_cursor.selected();
        let (file_path, file_diff_clone) = match self.diff_state.resolve_file(idx) {
            Some(f) => (f.path.clone(), f.clone()),
            None => return,
        };
        self.show_file(&file_path, how);
        self.expand_threads_for_file(&file_path);
        self.viewer.build_unified_diff_view(&file_diff_clone);
        // ファイルにレビューコメントがあれば最初のコメントへ着地させ(レビュアーが
        // すぐ気付けるようにする)、なければ最初の変更箇所へ着地させる。
        let first_comment_line = self
            .review_state
            .comments
            .iter()
            .filter(|c| c.file_path == file_path)
            .map(|c| c.line_start as usize)
            .min();
        let target = first_comment_line
            .and_then(|line| {
                self.viewer
                    .diff_view
                    .diff_view_lines
                    .iter()
                    .position(|e| matches!(e, crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(n), .. } if *n == line))
            })
            .or_else(|| {
                self.viewer
                    .diff_view
                    .diff_view_lines
                    .iter()
                    .position(|e| {
                        matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. }
                            if *tag != crate::diff_state::DiffLineTag::Equal)
                    })
            });
        if let Some(pos) = target {
            self.viewer.diff_view.diff_view_scroll = pos.saturating_sub(3);
        }
    }

    /// diff リストで次(または前)の変更ファイルへジャンプして開く。
    /// ファイル以外の行(セクション見出し、ディレクトリ、SUMMARY エントリ)はスキップする。
    /// GitHub 風のファイル横断スクロールの簡易代替。
    pub fn jump_to_changed_file(&mut self, forward: bool) {
        use crate::diff_state::DiffListEntry;
        let len = self.diff_state.display_list.len();
        // カーソルをクランプする: 古い diff_list_selected(リフレッシュでリストが
        // 縮んだ場合など)が下の後方スキャンでリスト範囲を超えてはならない。
        // 超えると display_list[i] がパニックする。
        let cur = self.explorer.changes_cursor.selected().min(len);
        let target = if forward {
            (cur + 1..len)
                .find(|&i| matches!(self.diff_state.display_list[i], DiffListEntry::File { .. }))
        } else {
            (0..cur)
                .rev()
                .find(|&i| matches!(self.diff_state.display_list[i], DiffListEntry::File { .. }))
        };
        if let Some(idx) = target {
            let len = self.diff_state.display_list.len();
            self.explorer.changes_cursor.place(idx, len);
            self.open_diff_file_at_selected(crate::app::OpenAs::Persistent);
        }
    }

    /// 新しく開いたファイルのインラインコメントスレッドをデフォルトで展開し、レビュー
    /// コメントが折りたたまれた状態で始まらず一目で見えるようにする。展開するのは
    /// 開いたファイルのスレッドだけ(全ファイルではない)で、「選択中ファイルのコメントは
    /// デフォルトで開く」という仕様に合わせている。個々のスレッドは後からユーザが
    /// 折りたためる。
    pub fn expand_threads_for_file(&mut self, file_path: &str) {
        // 未解決のコメントが1件以上ある行だけを自動展開する。
        // 解決済みのコメントはデフォルトで折りたたまれる(ガター上のバッジは表示され続け、
        // クリックするとスレッドがオンデマンドで開く)。
        let lines: Vec<usize> = self
            .review_state
            .comments
            .iter()
            .filter(|c| {
                c.file_path == file_path && c.status != crate::review_store::CommentStatus::Resolved
            })
            .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
            .collect();
        for line in lines {
            self.viewer.inline.expanded.insert(line);
        }
    }

    /// 現在の worktree に新しいレビューコメントを追加し、コメント一覧を更新する。
    pub fn add_review_comment(
        &mut self,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        author: Author,
    ) {
        let branch = self
            .worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.branch.clone());

        if let Some(store) = &self.review_store {
            // 不変条件: コメントの worktree カラムはブランチ名を保存し、commit_ref は
            // シンボリックな "HEAD"、branch も同じブランチ。MCP の create_comment
            // ツール(plugins/.../mcp)はこれと全く同じ形で書き込む姉妹実装であり、
            // 両者を同期させ続けること。
            let wt = self.selected_worktree_branch();
            match store.add_review(
                &wt,
                file_path,
                line_start,
                line_end,
                kind,
                body,
                "HEAD",
                author,
                branch.as_deref(),
            ) {
                Ok(_) => {
                    self.review_state.status_message = Some("Comment added.".to_string());
                    self.record_stat("reviews_created");
                }
                Err(e) => {
                    log::warn!("failed to add review comment: {e}");
                    self.review_state.status_message = Some(format!("Error: {e}"));
                }
            }
            self.review_state.load_comments(store, &wt);
            // コメントを追加したファイルについて、ファイル単位のキャッシュを再構築する。
            self.review_state.build_file_comment_cache(file_path);
            // 作成直後のスレッドは展開したままにし、ガターバッジに折りたたまれず
            // コメントがすぐに見えるようにする。
            let line = line_end.unwrap_or(line_start) as usize;
            self.viewer.inline.expanded.insert(line);
        }
    }

    /// ファイルパスの「viewed」マークをトグルする — diff リストの v キーと
    /// Viewer の diff モードの v キーから使われる。
    pub fn toggle_path_viewed(&mut self, path: &str) {
        let viewed = &mut self.explorer.viewed;
        if !viewed.remove(path) {
            viewed.insert(path.to_string());
        }
    }
}
