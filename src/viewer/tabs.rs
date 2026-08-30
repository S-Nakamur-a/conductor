//! 開いているファイルのタブ — 追加・切り替え・クローズと、根が変わったときの整理。
//!
//! アクティブなタブの状態は [ViewerState] の content/search/diff_view/selection
//! が実体を持ち、非アクティブなタブの分だけ [ViewerTab::stashed] に退避される。
//! 実体を 1 つに保つのは、アクティブなタブの状態が「タブの中」と「ViewerState
//! の直下」の 2 か所に現れると、どちらを書いたかで表示がずれるため。

use std::path::Path;

use super::state::{TabView, ViewerState, ViewerTab, ViewerTabStatus};

impl ViewerTabStatus {
    /// フォーカスが外れたら閉じるタブか。
    pub fn is_preview(self) -> bool {
        self == Self::Preview
    }

    /// 既に開いているタブを開き直したときの寿命。永続で開き直すと固定される —
    /// シングルクリックしたファイルをダブルクリックで固定する経路がこれ。
    /// 逆に、固定済みのタブがシングルクリックで preview へ戻ることはない。
    fn reopened(self, requested: Self) -> Self {
        match requested {
            Self::Persistent => Self::Persistent,
            Self::Preview => self,
        }
    }
}

impl ViewerTab {
    /// 描画テスト用に、中身を持たないタブを作る。
    #[cfg(test)]
    pub fn for_test(path: &str) -> Self {
        Self {
            path: path.to_string(),
            stashed: None,
            status: ViewerTabStatus::Persistent,
        }
    }
}

impl ViewerState {
    /// アクティブなタブのパス（タブが 1 つも無ければ None）。
    pub fn active_tab_path(&self) -> Option<&str> {
        self.tabs.get(self.active_tab).map(|t| t.path.as_str())
    }

    /// idx のタブへ切り替える。中身はディスクから読み直すので、裏で編集された
    /// ファイルが古いまま表示されることはない。
    pub fn focus_tab(&mut self, root: &Path, idx: usize, tab_width: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tab_reveal = true;
        if idx != self.active_tab {
            self.stash_active_view();
            let idx = self.close_preview_tab(idx);
            self.active_tab = idx;
            if let Some(view) = self.tabs[idx].stashed.take() {
                self.restore_view(view);
            }
        }
        let path = self.tabs[self.active_tab].path.clone();
        self.reload_active_file(root, &path, tab_width);
    }

    /// 次のタブへ（末尾からは先頭へ回り込む）。
    pub fn next_tab(&mut self, root: &Path, tab_width: usize) {
        if self.tabs.len() < 2 {
            return;
        }
        self.focus_tab(root, (self.active_tab + 1) % self.tabs.len(), tab_width);
    }

    /// 前のタブへ（先頭からは末尾へ回り込む）。
    pub fn prev_tab(&mut self, root: &Path, tab_width: usize) {
        if self.tabs.len() < 2 {
            return;
        }
        let last = self.tabs.len() - 1;
        let prev = if self.active_tab == 0 {
            last
        } else {
            self.active_tab - 1
        };
        self.focus_tab(root, prev, tab_width);
    }

    /// idx のタブを閉じる。アクティブなタブを閉じたときは右隣（無ければ左隣）へ
    /// 移り、最後の 1 枚を閉じたときはファイル未選択の状態へ戻る。
    pub fn close_tab(&mut self, root: &Path, idx: usize, tab_width: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tab_reveal = true;
        if idx != self.active_tab {
            self.tabs.remove(idx);
            if idx < self.active_tab {
                self.active_tab -= 1;
            }
            return;
        }

        self.tabs.remove(idx);
        self.clear_active_view();
        if self.tabs.is_empty() {
            self.active_tab = 0;
            return;
        }
        let next = idx.min(self.tabs.len() - 1);
        self.active_tab = next;
        if let Some(view) = self.tabs[next].stashed.take() {
            self.restore_view(view);
        }
        let path = self.tabs[next].path.clone();
        self.reload_active_file(root, &path, tab_width);
    }

    /// 根が変わったあと、新しい根に無いファイルのタブを閉じる。
    ///
    /// 相対パスは根が変わると別のファイルを指すので、残しておくと切り替え前の
    /// worktree のファイルを開いたままに見える。根は Explorer 側の状態なので
    /// 引数で受け取る (Viewer が Explorer をフィールドとして持つことはしない)。
    pub fn prune_tabs_to_root(&mut self, root: &std::path::Path, tab_width: usize) {
        let before = self.tabs.len();
        let active_path = self.active_tab_path().map(str::to_string);
        self.tabs.retain(|t| root.join(&t.path).is_file());
        if self.tabs.len() == before {
            return;
        }
        self.tab_reveal = true;
        match active_path.and_then(|p| self.tabs.iter().position(|t| t.path == p)) {
            Some(idx) => self.active_tab = idx,
            None => {
                self.clear_active_view();
                self.active_tab = 0;
                if let Some(tab) = self.tabs.first_mut() {
                    // 別の根で溜めた状態は捨て、新しい根のファイルとして開き直す。
                    tab.stashed = None;
                    let path = tab.path.clone();
                    self.load_active_file(root, &path, tab_width);
                }
            }
        }
    }

    /// relative_path のタブを用意してアクティブにする。既に開いていればそれを使う。
    /// 新しく作った場合だけ true を返す。
    ///
    /// status は開き方の要求で、既にあるタブへは
    /// [ViewerTabStatus::reopened] を通して反映する。
    pub(in crate::viewer) fn activate_tab_for(
        &mut self,
        relative_path: &str,
        status: ViewerTabStatus,
    ) -> bool {
        self.tab_reveal = true;
        if let Some(idx) = self.tabs.iter().position(|t| t.path == relative_path) {
            if idx == self.active_tab {
                let tab = &mut self.tabs[idx];
                tab.status = tab.status.reopened(status);
                return false;
            }
            self.stash_active_view();
            let idx = self.close_preview_tab(idx);
            self.active_tab = idx;
            if let Some(view) = self.tabs[idx].stashed.take() {
                self.restore_view(view);
            }
            return false;
        }
        self.stash_active_view();
        let len = self.tabs.len();
        self.close_preview_tab(len);
        self.tabs.push(ViewerTab {
            path: relative_path.to_string(),
            stashed: None,
            status,
        });
        self.active_tab = self.tabs.len() - 1;
        true
    }

    /// preview タブを閉じ、フォーカスの行き先 dest を詰めた添字を返す。
    ///
    /// preview は必ずアクティブなタブなので、呼ぶのはフォーカスが他へ移る
    /// 直前（stash_active_view の後）だけ。閉じたタブより後ろの添字は 1 つ
    /// 前へ動くので、行き先をここで詰めておかないと隣のファイルが開く。
    fn close_preview_tab(&mut self, dest: usize) -> usize {
        let Some(idx) = self.tabs.iter().position(|t| t.status.is_preview()) else {
            return dest;
        };
        self.tabs.remove(idx);
        self.clear_active_view();
        if dest > idx { dest - 1 } else { dest }
    }

    /// アクティブなタブの状態をタブ側へ退避する。
    fn stash_active_view(&mut self) {
        let view = self.take_active_view();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.stashed = Some(view);
        }
    }

    fn take_active_view(&mut self) -> TabView {
        TabView {
            content: std::mem::take(&mut self.content),
            search: std::mem::take(&mut self.search),
            diff_view: std::mem::take(&mut self.diff_view),
            selection: std::mem::take(&mut self.selection),
            md_scroll: std::mem::take(&mut self.md_scroll),
        }
    }

    fn restore_view(&mut self, view: TabView) {
        self.content = view.content;
        self.search = view.search;
        self.diff_view = view.diff_view;
        self.selection = view.selection;
        self.md_scroll = view.md_scroll;
    }

    /// アクティブなタブの状態を捨て、ファイル未選択の表示へ戻す。
    fn clear_active_view(&mut self) {
        self.take_active_view();
        self.show_summary = false;
        self.summary_scroll = 0;
        self.media_state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::Explorer;

    fn fixture(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tabs_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (path, body) in files {
            std::fs::write(dir.join(path), body).unwrap();
        }
        dir
    }

    /// 別のファイルを開いても前のファイルは閉じない — これが複数タブの本題。
    /// 同じファイルをもう一度開いたときはタブを増やさず、既にあるタブへ戻る。
    #[test]
    fn opening_files_accumulates_tabs_and_reopening_reuses_one() {
        let dir = fixture("reuse", &[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file(explorer.root(), "a.txt", 4);
        vs.open_file(explorer.root(), "b.txt", 4);
        assert_eq!(vs.tabs.len(), 2);
        assert_eq!(vs.active_tab_path(), Some("b.txt"));

        vs.open_file(explorer.root(), "a.txt", 4);
        assert_eq!(vs.tabs.len(), 2, "既に開いているファイルはタブを増やさない");
        assert_eq!(vs.active_tab_path(), Some("a.txt"));
        assert_eq!(vs.content.file_content, vec!["A"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// タブごとに読んでいた位置を持つ。戻ったときに先頭へ巻き戻されると、
    /// 差分レビュー中に行き来する用途では複数タブの意味が無くなる。
    #[test]
    fn switching_tabs_restores_where_each_file_was_left() {
        let long: String = (0..50).map(|i| format!("line{i}\n")).collect();
        let dir = fixture("scroll", &[("a.txt", &long), ("b.txt", &long)]);
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file(explorer.root(), "a.txt", 4);
        vs.content.file_scroll = 30;
        vs.open_file(explorer.root(), "b.txt", 4);
        assert_eq!(vs.content.file_scroll, 0, "新しいタブは先頭から");
        vs.content.file_scroll = 10;

        vs.prev_tab(explorer.root(), 4);
        assert_eq!(vs.active_tab_path(), Some("a.txt"));
        assert_eq!(vs.content.file_scroll, 30);

        vs.next_tab(explorer.root(), 4);
        assert_eq!(vs.active_tab_path(), Some("b.txt"));
        assert_eq!(vs.content.file_scroll, 10);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非アクティブな間にディスク上で書き換えられたタブは、戻った時点の中身を出す。
    /// Claude Code が裏でファイルを直すのが日常なので、古い本文が残ると実害が出る。
    #[test]
    fn returning_to_a_tab_rereads_it_from_disk() {
        let dir = fixture("stale", &[("a.txt", "OLD\n"), ("b.txt", "B\n")]);
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file(explorer.root(), "a.txt", 4);
        vs.open_file(explorer.root(), "b.txt", 4);
        std::fs::write(dir.join("a.txt"), "NEW\n").unwrap();
        vs.prev_tab(explorer.root(), 4);

        assert_eq!(vs.content.file_content, vec!["NEW"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// タブを閉じたら隣へ移る。最後の 1 枚を閉じたらファイル未選択に戻る。
    #[test]
    fn closing_a_tab_falls_back_to_a_neighbour_then_to_nothing() {
        let dir = fixture("close", &[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file(explorer.root(), "a.txt", 4);
        vs.open_file(explorer.root(), "b.txt", 4);
        let active = vs.active_tab;
        vs.close_tab(explorer.root(), active, 4);
        assert_eq!(vs.tabs.len(), 1);
        assert_eq!(vs.active_tab_path(), Some("a.txt"));
        assert_eq!(vs.content.file_content, vec!["A"]);

        vs.close_tab(explorer.root(), 0, 4);
        assert!(vs.tabs.is_empty());
        assert_eq!(vs.content.current_file, None);
        assert!(vs.content.file_content.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// シングルクリックで開いた preview タブは、次のファイルを開くと閉じる。
    /// クリックするたびにタブが増えるのを防ぐのがこの機能の本題。
    #[test]
    fn a_preview_tab_is_replaced_by_the_next_one() {
        let dir = fixture(
            "preview",
            &[("a.txt", "A\n"), ("b.txt", "B\n"), ("c.txt", "C\n")],
        );
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file_preview(explorer.root(), "a.txt", 4);
        vs.open_file_preview(explorer.root(), "b.txt", 4);
        assert_eq!(vs.tabs.len(), 1, "preview タブは同時に 1 枚だけ");
        assert_eq!(vs.active_tab_path(), Some("b.txt"));
        assert_eq!(vs.content.file_content, vec!["B"]);

        // 永続で開いた場合も、残っていた preview は閉じる。
        vs.open_file(explorer.root(), "c.txt", 4);
        assert_eq!(vs.tabs.len(), 1);
        assert_eq!(vs.active_tab_path(), Some("c.txt"));
        assert!(!vs.tabs[0].status.is_preview());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ダブルクリック相当（preview のまま永続で開き直す）で preview が外れ、
    /// 次のファイルを開いても残る。
    #[test]
    fn reopening_a_preview_tab_as_persistent_pins_it() {
        let dir = fixture("promote", &[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file_preview(explorer.root(), "a.txt", 4);
        vs.open_file(explorer.root(), "a.txt", 4);
        assert!(!vs.tabs[0].status.is_preview());

        vs.open_file_preview(explorer.root(), "b.txt", 4);
        assert_eq!(vs.tabs.len(), 2, "固定したタブは次を開いても残る");
        assert_eq!(vs.active_tab_path(), Some("b.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 別のタブへ移った時点でも preview は閉じる。閉じたぶん添字が前へ動くので、
    /// 詰め忘れると隣のファイルが開く。
    #[test]
    fn focusing_another_tab_closes_the_preview() {
        let dir = fixture(
            "preview_focus",
            &[("a.txt", "A\n"), ("b.txt", "B\n"), ("c.txt", "C\n")],
        );
        let mut explorer = Explorer::default();
        explorer.set_root(dir.clone());
        let mut vs = ViewerState::default();

        vs.open_file(explorer.root(), "a.txt", 4);
        vs.open_file(explorer.root(), "b.txt", 4);
        vs.open_file_preview(explorer.root(), "c.txt", 4);
        assert_eq!(vs.tabs.len(), 3);

        vs.focus_tab(explorer.root(), 0, 4);
        assert_eq!(vs.tabs.len(), 2);
        assert_eq!(vs.active_tab, 0);
        assert_eq!(vs.active_tab_path(), Some("a.txt"));
        assert_eq!(vs.content.file_content, vec!["A"]);
        assert!(vs.tabs.iter().all(|t| !t.status.is_preview()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// worktree を切り替えたら、切り替え先に無いファイルのタブは残らない。
    /// 相対パスは根が変わると別のファイルを指すので、残すと別ブランチの中身を
    /// 開いたままに見える。
    #[test]
    fn switching_root_drops_tabs_that_do_not_exist_there() {
        let a = fixture("root_a", &[("both.txt", "A\n"), ("only_a.txt", "A\n")]);
        let b = fixture("root_b", &[("both.txt", "B\n")]);
        let mut explorer = Explorer::default();
        explorer.set_root(a.clone());
        let mut vs = ViewerState::default();

        vs.open_file(explorer.root(), "both.txt", 4);
        vs.open_file(explorer.root(), "only_a.txt", 4);
        assert_eq!(vs.tabs.len(), 2);

        let reload = explorer.load_file_tree(&b, vs.content.current_file.as_deref());
        if reload.root_changed {
            vs.prune_tabs_to_root(explorer.root(), 4);
        }

        assert_eq!(vs.tabs.len(), 1);
        assert_eq!(vs.active_tab_path(), Some("both.txt"));
        assert_eq!(vs.content.file_content, vec!["B"], "新しい根の中身を読む");

        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }
}
