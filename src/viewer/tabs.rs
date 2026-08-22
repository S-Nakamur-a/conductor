//! 開いているファイルのタブ — 追加・切り替え・クローズと、根が変わったときの整理。
//!
//! アクティブなタブの状態は [ViewerState] の content/search/diff_view/selection
//! が実体を持ち、非アクティブなタブの分だけ [ViewerTab::stashed] に退避される。
//! 実体を 1 つに保つのは、アクティブなタブの状態が「タブの中」と「ViewerState
//! の直下」の 2 か所に現れると、どちらを書いたかで表示がずれるため。

use super::state::{TabView, ViewerState, ViewerTab};

impl ViewerTab {
    /// 描画テスト用に、中身を持たないタブを作る。
    #[cfg(test)]
    pub fn for_test(path: &str) -> Self {
        Self {
            path: path.to_string(),
            stashed: None,
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
    pub fn focus_tab(&mut self, idx: usize, tab_width: usize) {
        let Some(path) = self.tabs.get(idx).map(|t| t.path.clone()) else {
            return;
        };
        if idx != self.active_tab {
            self.stash_active_view();
            self.active_tab = idx;
            if let Some(view) = self.tabs[idx].stashed.take() {
                self.restore_view(view);
            }
        }
        self.reload_active_file(&path, tab_width);
    }

    /// 次のタブへ（末尾からは先頭へ回り込む）。
    pub fn next_tab(&mut self, tab_width: usize) {
        if self.tabs.len() < 2 {
            return;
        }
        self.focus_tab((self.active_tab + 1) % self.tabs.len(), tab_width);
    }

    /// 前のタブへ（先頭からは末尾へ回り込む）。
    pub fn prev_tab(&mut self, tab_width: usize) {
        if self.tabs.len() < 2 {
            return;
        }
        let last = self.tabs.len() - 1;
        let prev = if self.active_tab == 0 {
            last
        } else {
            self.active_tab - 1
        };
        self.focus_tab(prev, tab_width);
    }

    /// idx のタブを閉じる。アクティブなタブを閉じたときは右隣（無ければ左隣）へ
    /// 移り、最後の 1 枚を閉じたときはファイル未選択の状態へ戻る。
    pub fn close_tab(&mut self, idx: usize, tab_width: usize) {
        if idx >= self.tabs.len() {
            return;
        }
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
        self.reload_active_file(&path, tab_width);
    }

    /// 根が変わったあと、新しい根に無いファイルのタブを閉じる。
    ///
    /// 相対パスは根が変わると別のファイルを指すので、残しておくと切り替え前の
    /// worktree のファイルを開いたままに見える。
    pub fn prune_tabs_to_root(&mut self, tab_width: usize) {
        let root = self.tree.root.clone();
        let before = self.tabs.len();
        let active_path = self.active_tab_path().map(str::to_string);
        self.tabs.retain(|t| root.join(&t.path).is_file());
        if self.tabs.len() == before {
            return;
        }
        match active_path.and_then(|p| self.tabs.iter().position(|t| t.path == p)) {
            Some(idx) => self.active_tab = idx,
            None => {
                self.clear_active_view();
                self.active_tab = 0;
                if let Some(tab) = self.tabs.first_mut() {
                    // 別の根で溜めた状態は捨て、新しい根のファイルとして開き直す。
                    tab.stashed = None;
                    let path = tab.path.clone();
                    self.load_active_file(&path, tab_width);
                }
            }
        }
    }

    /// relative_path のタブを用意してアクティブにする。既に開いていればそれを使う。
    /// 新しく作った場合だけ true を返す。
    pub(in crate::viewer) fn activate_tab_for(&mut self, relative_path: &str) -> bool {
        if let Some(idx) = self.tabs.iter().position(|t| t.path == relative_path) {
            if idx != self.active_tab {
                self.stash_active_view();
                self.active_tab = idx;
                if let Some(view) = self.tabs[idx].stashed.take() {
                    self.restore_view(view);
                }
            }
            return false;
        }
        self.stash_active_view();
        self.tabs.push(ViewerTab {
            path: relative_path.to_string(),
            stashed: None,
        });
        self.active_tab = self.tabs.len() - 1;
        true
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
        let mut vs = ViewerState::default();
        vs.set_root(dir.clone());

        vs.open_file("a.txt", 4);
        vs.open_file("b.txt", 4);
        assert_eq!(vs.tabs.len(), 2);
        assert_eq!(vs.active_tab_path(), Some("b.txt"));

        vs.open_file("a.txt", 4);
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
        let mut vs = ViewerState::default();
        vs.set_root(dir.clone());

        vs.open_file("a.txt", 4);
        vs.content.file_scroll = 30;
        vs.open_file("b.txt", 4);
        assert_eq!(vs.content.file_scroll, 0, "新しいタブは先頭から");
        vs.content.file_scroll = 10;

        vs.prev_tab(4);
        assert_eq!(vs.active_tab_path(), Some("a.txt"));
        assert_eq!(vs.content.file_scroll, 30);

        vs.next_tab(4);
        assert_eq!(vs.active_tab_path(), Some("b.txt"));
        assert_eq!(vs.content.file_scroll, 10);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非アクティブな間にディスク上で書き換えられたタブは、戻った時点の中身を出す。
    /// Claude Code が裏でファイルを直すのが日常なので、古い本文が残ると実害が出る。
    #[test]
    fn returning_to_a_tab_rereads_it_from_disk() {
        let dir = fixture("stale", &[("a.txt", "OLD\n"), ("b.txt", "B\n")]);
        let mut vs = ViewerState::default();
        vs.set_root(dir.clone());

        vs.open_file("a.txt", 4);
        vs.open_file("b.txt", 4);
        std::fs::write(dir.join("a.txt"), "NEW\n").unwrap();
        vs.prev_tab(4);

        assert_eq!(vs.content.file_content, vec!["NEW"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// タブを閉じたら隣へ移る。最後の 1 枚を閉じたらファイル未選択に戻る。
    #[test]
    fn closing_a_tab_falls_back_to_a_neighbour_then_to_nothing() {
        let dir = fixture("close", &[("a.txt", "A\n"), ("b.txt", "B\n")]);
        let mut vs = ViewerState::default();
        vs.set_root(dir.clone());

        vs.open_file("a.txt", 4);
        vs.open_file("b.txt", 4);
        vs.close_tab(vs.active_tab, 4);
        assert_eq!(vs.tabs.len(), 1);
        assert_eq!(vs.active_tab_path(), Some("a.txt"));
        assert_eq!(vs.content.file_content, vec!["A"]);

        vs.close_tab(0, 4);
        assert!(vs.tabs.is_empty());
        assert_eq!(vs.content.current_file, None);
        assert!(vs.content.file_content.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// worktree を切り替えたら、切り替え先に無いファイルのタブは残らない。
    /// 相対パスは根が変わると別のファイルを指すので、残すと別ブランチの中身を
    /// 開いたままに見える。
    #[test]
    fn switching_root_drops_tabs_that_do_not_exist_there() {
        let a = fixture("root_a", &[("both.txt", "A\n"), ("only_a.txt", "A\n")]);
        let b = fixture("root_b", &[("both.txt", "B\n")]);
        let mut vs = ViewerState::default();
        vs.set_root(a.clone());

        vs.open_file("both.txt", 4);
        vs.open_file("only_a.txt", 4);
        assert_eq!(vs.tabs.len(), 2);

        vs.load_file_tree(&b, 4);

        assert_eq!(vs.tabs.len(), 1);
        assert_eq!(vs.active_tab_path(), Some("both.txt"));
        assert_eq!(vs.content.file_content, vec!["B"], "新しい根の中身を読む");

        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }
}
