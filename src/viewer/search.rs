//! ファイル内テキスト検索とファジーなファイル名検索。

use super::file_view::UnifiedDiffEntry;
use super::state::ViewerState;

/// 削除行は新ファイル側の行を持たないので None。ExpandableContext は最初の隠れた行、
/// HunkSeparator は行番号そのものを持たない。
fn diff_entry_new_line_no(entry: &UnifiedDiffEntry) -> Option<usize> {
    match entry {
        UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
        UnifiedDiffEntry::ExpandableContext { new_line_start, .. } => Some(*new_line_start),
        UnifiedDiffEntry::HunkSeparator { .. } => None,
    }
}

impl ViewerState {
    /// ファイル内容に対して検索を実行し、search_matches を埋める。
    pub fn execute_search(&mut self) {
        self.search.search_matches.clear();
        self.search.search_match_idx = 0;

        if self.search.search_query.is_empty() {
            return;
        }

        let query_lower = self.search.search_query.to_lowercase();
        for (i, line) in self.content.file_content.iter().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                self.search.search_matches.push(i);
            }
        }

        // 現在のスクロール位置以降にある最初のマッチへジャンプする。
        if !self.search.search_matches.is_empty() {
            self.search.search_match_idx = self
                .search
                .search_matches
                .iter()
                .position(|&line| line >= self.content.file_scroll)
                .unwrap_or(0);
            self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        }
        self.sync_diff_scroll_to_file_scroll();
    }

    /// 次の検索マッチへジャンプする。
    pub fn next_search_match(&mut self) {
        if self.search.search_matches.is_empty() {
            return;
        }
        self.search.search_match_idx =
            (self.search.search_match_idx + 1) % self.search.search_matches.len();
        self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        self.sync_diff_scroll_to_file_scroll();
    }

    /// 前の検索マッチへジャンプする。
    pub fn prev_search_match(&mut self) {
        if self.search.search_matches.is_empty() {
            return;
        }
        self.search.search_match_idx = if self.search.search_match_idx == 0 {
            self.search.search_matches.len() - 1
        } else {
            self.search.search_match_idx - 1
        };
        self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        self.sync_diff_scroll_to_file_scroll();
    }

    /// diff_view_scroll 以降で最も近い具体的な行番号を使い、無ければそれより前で最も
    /// 近いものに落とす (カーソルが削除行の上にある場合など)。
    fn diff_scroll_file_line(&self) -> Option<usize> {
        let lines = &self.diff_view.diff_view_lines;
        let scroll = self.diff_view.diff_view_scroll.min(lines.len());
        lines[scroll..]
            .iter()
            .find_map(diff_entry_new_line_no)
            .or_else(|| {
                lines[..scroll]
                    .iter()
                    .rev()
                    .find_map(diff_entry_new_line_no)
            })
            .map(|n| n.saturating_sub(1))
    }

    /// content.file_scroll を diff 表示のスクロール位置と同期させておく。
    /// symbol 検索やテキスト検索は無条件に content.file_scroll を使う
    /// （diff モードより前から存在する機能なので）ので、diff を閲覧中に
    /// それらへ到達するにはまずここで同期しておかないと、通常表示での閲覧が
    /// 最後に file_scroll を残した行に対して動作してしまう。diff モード以外
    /// では何もしない。
    pub fn sync_file_scroll_to_diff_scroll(&mut self) {
        if !self.diff_view.diff_mode {
            return;
        }
        if let Some(line) = self.diff_scroll_file_line() {
            self.content.file_scroll = line;
        }
    }

    /// 背後のカーソルだけが動いて diff ペインがその場に留まるのを防ぐ。
    /// diff モード以外では何もしない。
    fn sync_diff_scroll_to_file_scroll(&mut self) {
        if !self.diff_view.diff_mode {
            return;
        }
        let target_line = self.content.file_scroll + 1; // new_line_no は1始まり
        if let Some(idx) = self
            .diff_view
            .diff_view_lines
            .iter()
            .position(|entry| diff_entry_new_line_no(entry).is_some_and(|n| n >= target_line))
        {
            self.diff_view.diff_view_scroll = idx;
        }
    }

    // ファイル名のファジー検索

    /// キャッシュ済みのファイル一覧に対してファジーなファイル名検索を実行し、結果を埋める。
    pub fn execute_filename_search(&mut self) {
        self.filename_search.filename_search_results.clear();

        let query = self.filename_search.filename_search_query.to_lowercase();

        for path in &self.filename_search.filename_search_all_files {
            let path_lower = path.to_lowercase();
            let name_lower = path.rsplit('/').next().unwrap_or(path).to_lowercase();

            // クエリが空なら、全ファイルをスコア0で含める。
            if query.is_empty() {
                self.filename_search
                    .filename_search_results
                    .push(super::file_tree::ScoredFile {
                        path: path.clone(),
                        score: 0,
                    });
                continue;
            }

            // まずファジーな部分列マッチを確認する — マッチしないファイルはスキップする。
            if !Self::is_fuzzy_match(&query, &path_lower) {
                continue;
            }

            let mut score: i32 = 10; // ファジーマッチのベーススコア。

            // ボーナス: 連続する文字のマッチ。
            score += Self::consecutive_bonus(&query, &path_lower);

            // ボーナス: ファイル名の完全な前方一致。
            if name_lower.starts_with(&query) {
                score += 100;
            }

            // ボーナス: パスの部分一致。
            if path_lower.contains(&query) {
                score += 50;
            }

            // ボーナス: ファイル名の部分一致。
            if name_lower.contains(&query) {
                score += 30;
            }

            // ボーナス: 単語境界でのマッチ（'/'、'_'、'-'、'.' の直後の文字）。
            if Self::has_word_boundary_match(&query, &path_lower) {
                score += 20;
            }

            self.filename_search
                .filename_search_results
                .push(super::file_tree::ScoredFile {
                    path: path.clone(),
                    score,
                });
        }

        // スコアの降順でソートし、安定させるためパスの昇順を副キーにする。
        self.filename_search
            .filename_search_results
            .sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    }

    /// query の全文字が haystack の中に順番通りに出現するかを確認する。
    fn is_fuzzy_match(query: &str, haystack: &str) -> bool {
        let mut haystack_chars = haystack.chars();
        for qc in query.chars() {
            if !haystack_chars.any(|hc| hc == qc) {
                return false;
            }
        }
        true
    }

    /// 連続してマッチする文字に対してボーナス点を与える。
    fn consecutive_bonus(query: &str, haystack: &str) -> i32 {
        let mut bonus = 0i32;
        let mut consecutive = 0;
        let mut hay_iter = haystack.chars().peekable();

        for qc in query.chars() {
            let mut found = false;
            for hc in hay_iter.by_ref() {
                if hc == qc {
                    consecutive += 1;
                    if consecutive > 1 {
                        bonus += consecutive;
                    }
                    found = true;
                    break;
                }
                consecutive = 0;
            }
            if !found {
                break;
            }
        }
        bonus
    }

    /// query の文字が haystack の単語境界（'/'、'_'、'-'、'.' の直後、または
    /// 先頭位置）でマッチするかを確認する。
    fn has_word_boundary_match(query: &str, haystack: &str) -> bool {
        let boundary_chars: Vec<char> = haystack
            .char_indices()
            .filter(|&(i, _)| {
                if i == 0 {
                    return true;
                }
                let prev = haystack.as_bytes().get(i - 1).copied().unwrap_or(0);
                matches!(prev, b'/' | b'_' | b'-' | b'.')
            })
            .map(|(_, c)| c)
            .collect();

        if boundary_chars.len() < query.len() {
            return false;
        }

        let mut bi = 0;
        for qc in query.chars() {
            let mut found = false;
            while bi < boundary_chars.len() {
                if boundary_chars[bi] == qc {
                    bi += 1;
                    found = true;
                    break;
                }
                bi += 1;
            }
            if !found {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::DiffLineTag;

    /// 指定した新ファイル側の行番号を持つ Line エントリを作る（削除行なら
    /// 新ファイル側の行が無いので None）。
    fn diff_line(new_line_no: Option<usize>) -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag: DiffLineTag::Equal,
            new_line_no,
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }

    #[test]
    fn 削除行は後ろ向きに解決して同期する() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![
            UnifiedDiffEntry::HunkSeparator { func_header: None },
            diff_line(Some(10)), // インデックス1
            diff_line(None),     // インデックス2 — 削除行なので新ファイル側の行が無い
            diff_line(Some(11)), // インデックス3
        ];

        // 削除行の上にスクロールした: このインデックスちょうどには新ファイル側の
        // 行が無いので、カーソルは次の具体的な行（11）へ前方解決される。
        vs.diff_view.diff_view_scroll = 2;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 10); // 11行目、0始まり

        // 具体的な行に直接スクロールした場合: その行に解決される。
        vs.diff_view.diff_view_scroll = 1;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 9); // 10行目、0始まり
    }

    #[test]
    fn diff表示の外では同期は何もしない() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = false;
        vs.diff_view.diff_view_lines = vec![diff_line(Some(5))];
        vs.diff_view.diff_view_scroll = 0;
        vs.content.file_scroll = 42;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 42);
    }

    #[test]
    fn 検索のジャンプにdiff側も追従する() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![
            diff_line(Some(1)), // インデックス0
            diff_line(Some(2)), // インデックス1
            diff_line(Some(3)), // インデックス2
        ];
        vs.diff_view.diff_view_scroll = 0;

        // 検索マッチが file_scroll = 2（3行目、0始まり）に着地した。
        vs.content.file_scroll = 2;
        vs.sync_diff_scroll_to_file_scroll();
        assert_eq!(vs.diff_view.diff_view_scroll, 2);
    }
}
