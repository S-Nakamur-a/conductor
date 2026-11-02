//! ファイル内検索。

use conductor_core::text_input::TextInput;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
