//! ファイル内検索と、diff 表示との位置合わせ。

use conductor_core::text_input::TextInput;

use super::diff::Entry;

#[derive(Debug, Default)]
pub struct Search {
    pub query: TextInput,
    /// 当たった行の 0 始まり添字。昇順。
    pub matches: Vec<usize>,
    pub index: usize,
}

impl Search {
    /// 検索し直し、`from` 以降で最初に当たった行を返す。当たらなければ None。
    pub fn run(&mut self, lines: &[String], from: usize) -> Option<usize> {
        self.matches.clear();
        self.index = 0;
        if self.query.is_empty() {
            return None;
        }
        let needle = self.query.to_lowercase();
        self.matches = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect();
        self.index = self.matches.iter().position(|&l| l >= from).unwrap_or(0);
        self.current()
    }

    /// 次の当たりへ。端は巻き戻る。
    pub fn advance(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        self.index = (self.index + 1) % self.matches.len();
        self.current()
    }

    pub fn retreat(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        self.index = self.index.checked_sub(1).unwrap_or(self.matches.len() - 1);
        self.current()
    }

    pub fn current(&self) -> Option<usize> {
        self.matches.get(self.index).copied()
    }

    pub fn is_match(&self, line_0: usize) -> bool {
        self.matches.binary_search(&line_0).is_ok()
    }
}

/// diff のスクロール位置が指しているファイルの行 (0 始まり)。
///
/// その位置以降で最も近い具体的な行番号を使い、無ければ手前へ落とす。削除行の上に
/// カーソルがあると、その行自身は新ファイル側の行を持たない。
pub fn file_line_at(entries: &[Entry], diff_scroll: usize) -> Option<usize> {
    let scroll = diff_scroll.min(entries.len());
    entries[scroll..]
        .iter()
        .find_map(Entry::new_line_no)
        .or_else(|| entries[..scroll].iter().rev().find_map(Entry::new_line_no))
        .map(|n| n.saturating_sub(1))
}

/// ファイルの行 (0 始まり) を含む最初の diff エントリ。
pub fn diff_index_for(entries: &[Entry], file_line: usize) -> Option<usize> {
    let target = file_line + 1;
    entries
        .iter()
        .position(|e| e.new_line_no().is_some_and(|n| n >= target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::diff_state::DiffLineTag;

    fn entry(new_line_no: Option<usize>) -> Entry {
        Entry::Line {
            tag: DiffLineTag::Equal,
            old_line_no: None,
            new_line_no,
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 検索は現在位置以降の最初の当たりに着く() {
        let body = lines(&["alpha", "beta", "ALPHA", "gamma", "alpha"]);
        let mut search = Search::default();
        search.query.set_text("alpha");

        assert_eq!(search.run(&body, 0), Some(0), "大小は無視する");
        assert_eq!(search.matches, [0, 2, 4]);
        assert_eq!(search.run(&body, 3), Some(4));
        assert_eq!(search.run(&body, 99), Some(0), "後ろに無ければ先頭へ");
    }

    #[test]
    fn 次と前は端で巻き戻る() {
        let body = lines(&["a", "b", "a"]);
        let mut search = Search::default();
        search.query.set_text("a");
        search.run(&body, 0);
        assert_eq!(search.advance(), Some(2));
        assert_eq!(search.advance(), Some(0));
        assert_eq!(search.retreat(), Some(2));
    }

    #[test]
    fn 当たりが無ければどこへも動かない() {
        let mut search = Search::default();
        search.query.set_text("zzz");
        assert_eq!(search.run(&lines(&["a"]), 0), None);
        assert_eq!(search.advance(), None);
        assert_eq!(search.retreat(), None);
    }

    #[test]
    fn 削除行は前後どちらかの具体的な行に解決する() {
        let entries = vec![
            Entry::HunkSeparator { func_header: None },
            entry(Some(10)),
            entry(None),
            entry(Some(11)),
        ];
        // 削除行の上では次の具体的な行へ前方解決する。
        assert_eq!(file_line_at(&entries, 2), Some(10));
        assert_eq!(file_line_at(&entries, 1), Some(9));
        // 末尾より後ろでは手前へ落ちる。
        assert_eq!(file_line_at(&entries, 4), Some(10));
        assert_eq!(file_line_at(&[], 0), None);
    }

    #[test]
    fn ファイルの行からdiffの位置を引ける() {
        let entries = vec![entry(Some(1)), entry(Some(2)), entry(Some(3))];
        assert_eq!(diff_index_for(&entries, 2), Some(2));
        assert_eq!(diff_index_for(&entries, 0), Some(0));
        assert_eq!(diff_index_for(&entries, 99), None);
    }
}
