//! 統合 diff ビュー向けの純粋なナビゲーションヘルパー: 次/前の変更ブロック、
//! 次/前のコメント付き行を探す。App の状態に依存しないため単体テストが容易。

use std::collections::HashMap;

use crate::diff_state::DiffLineTag;
use crate::viewer::UnifiedDiffEntry;

/// 統合 diff 内の変更行（追加または削除）かどうか。
fn is_change_line(entry: &UnifiedDiffEntry) -> bool {
    matches!(
        entry,
        UnifiedDiffEntry::Line {
            tag: DiffLineTag::Insert | DiffLineTag::Delete,
            ..
        }
    )
}

/// インデックス i が連続した変更ブロックの先頭行なら true。
fn is_change_block_start(lines: &[UnifiedDiffEntry], i: usize) -> bool {
    is_change_line(&lines[i]) && (i == 0 || !is_change_line(&lines[i - 1]))
}

/// from より後にある次の変更ブロックのインデックス。
pub(super) fn next_change_block(lines: &[UnifiedDiffEntry], from: usize) -> Option<usize> {
    (from + 1..lines.len()).find(|&i| is_change_block_start(lines, i))
}

/// from より前にある直前の変更ブロックのインデックス。
pub(super) fn prev_change_block(lines: &[UnifiedDiffEntry], from: usize) -> Option<usize> {
    (0..from).rev().find(|&i| is_change_block_start(lines, i))
}

/// diff のエントリがレビューコメントを持つなら true。Line は新ファイル側の
/// 行番号にコメントがあれば一致し、折りたたまれた ExpandableContext はその
/// 範囲内の隠れた行のいずれかにコメントがあれば一致する。これにより、
/// 現在折りたたみの中に入っているコメントにもジャンプできる（着地先は
/// 展開できるよう折りたたみ自体になる）。
fn entry_has_comment<V>(entry: &UnifiedDiffEntry, comments: &HashMap<usize, V>) -> bool {
    match entry {
        UnifiedDiffEntry::Line {
            new_line_no: Some(n),
            ..
        } => comments.contains_key(n),
        UnifiedDiffEntry::ExpandableContext {
            new_line_start,
            new_line_end,
            ..
        } => (*new_line_start..=*new_line_end).any(|l| comments.contains_key(&l)),
        _ => false,
    }
}

/// from より後にある、次にコメントが付いた diff エントリのインデックス。
pub(super) fn next_comment_line<V>(
    lines: &[UnifiedDiffEntry],
    comments: &HashMap<usize, V>,
    from: usize,
) -> Option<usize> {
    (from + 1..lines.len()).find(|&i| entry_has_comment(&lines[i], comments))
}

/// from より前にある、直前にコメントが付いた diff エントリのインデックス。
pub(super) fn prev_comment_line<V>(
    lines: &[UnifiedDiffEntry],
    comments: &HashMap<usize, V>,
    from: usize,
) -> Option<usize> {
    (0..from)
        .rev()
        .find(|&i| entry_has_comment(&lines[i], comments))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ln(tag: DiffLineTag) -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag,
            new_line_no: Some(1),
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }
    fn eq() -> UnifiedDiffEntry {
        ln(DiffLineTag::Equal)
    }
    fn ins() -> UnifiedDiffEntry {
        ln(DiffLineTag::Insert)
    }
    fn del() -> UnifiedDiffEntry {
        ln(DiffLineTag::Delete)
    }
    fn sep() -> UnifiedDiffEntry {
        UnifiedDiffEntry::HunkSeparator { func_header: None }
    }
    /// 新ファイル側の行番号を明示的に持つ Line。
    fn line_no(tag: DiffLineTag, n: usize) -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag,
            new_line_no: Some(n),
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }
    /// 削除行の Line（新ファイル側の行番号を持たない）。
    fn del_no_line() -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag: DiffLineTag::Delete,
            new_line_no: None,
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }
    fn fold(start: usize, end: usize) -> UnifiedDiffEntry {
        UnifiedDiffEntry::ExpandableContext {
            hidden_count: end - start + 1,
            new_line_start: start,
            new_line_end: end,
            func_header: None,
        }
    }

    #[test]
    fn change_block_start_detection() {
        assert!(is_change_block_start(&[ins()], 0));
        assert!(is_change_block_start(&[eq(), ins()], 1));
        assert!(!is_change_block_start(&[ins(), ins()], 1)); // ブロックの途中
        assert!(!is_change_block_start(&[eq()], 0)); // 変更行ではない
        assert!(is_change_block_start(&[sep(), ins()], 1)); // セパレータで区切られる
        assert!(!is_change_block_start(&[del(), ins()], 1)); // del+ins は1つの変更ブロック
    }

    #[test]
    fn next_change_block_cases() {
        assert_eq!(next_change_block(&[], 0), None);
        assert_eq!(next_change_block(&[eq(), eq()], 0), None);
        let v = vec![eq(), ins(), eq(), ins(), eq()];
        assert_eq!(next_change_block(&v, 0), Some(1));
        assert_eq!(next_change_block(&v, 1), Some(3));
        assert_eq!(next_change_block(&v, 3), None);
        // ブロックの途中からは、現在のブロックの残りをスキップする。
        let v = vec![ins(), ins(), ins(), eq(), ins()];
        assert_eq!(next_change_block(&v, 1), Some(4));
        // セパレータは隣接する2つの変更行を分断する。
        assert_eq!(next_change_block(&[ins(), sep(), ins()], 0), Some(2));
    }

    #[test]
    fn prev_change_block_cases() {
        assert_eq!(prev_change_block(&[], 0), None);
        let v = vec![eq(), ins(), eq(), ins(), eq()];
        assert_eq!(prev_change_block(&v, 4), Some(3));
        assert_eq!(prev_change_block(&v, 3), Some(1));
        assert_eq!(prev_change_block(&v, 1), None);
        // ブロックの途中からは、そのブロックの先頭に着地する。
        assert_eq!(prev_change_block(&[ins(), ins(), ins()], 2), Some(0));
        assert_eq!(prev_change_block(&[ins(), ins()], 0), None);
    }

    #[test]
    fn comment_navigation() {
        let comments: HashMap<usize, ()> = [(5, ()), (8, ())].into_iter().collect();
        let v = vec![
            line_no(DiffLineTag::Equal, 4),  // 0: コメントなし
            line_no(DiffLineTag::Insert, 5), // 1: コメントあり
            del_no_line(),                   // 2: 削除行、コメントは付かない
            line_no(DiffLineTag::Equal, 8),  // 3: コメントあり
        ];
        assert_eq!(next_comment_line(&v, &comments, 0), Some(1));
        assert_eq!(next_comment_line(&v, &comments, 1), Some(3));
        assert_eq!(next_comment_line(&v, &comments, 3), None);
        assert_eq!(prev_comment_line(&v, &comments, 3), Some(1));
        assert_eq!(prev_comment_line(&v, &comments, 1), None);
        // コメント集合が空なら何も見つからない。
        let none: HashMap<usize, ()> = HashMap::new();
        assert_eq!(next_comment_line(&v, &none, 0), None);
    }

    #[test]
    fn delete_line_is_never_commented() {
        // 削除行は新ファイル側の行番号を持たないため、その行番号がマップに
        // あってもコメントは付けられない。
        let comments: HashMap<usize, ()> = [(1, ())].into_iter().collect();
        assert!(!entry_has_comment(&del_no_line(), &comments));
    }

    #[test]
    fn comment_hidden_in_fold_is_reachable() {
        // 7行目のコメントは 5..=10 の折りたたまれたコンテキストの中にある。
        // ジャンプは展開できるよう折りたたみ自体に着地するべき。
        let comments: HashMap<usize, ()> = [(7, ())].into_iter().collect();
        let v = vec![ins(), fold(5, 10)];
        assert_eq!(next_comment_line(&v, &comments, 0), Some(1));
        // 折りたたみの範囲内にコメントがなければマッチしない。
        let comments: HashMap<usize, ()> = [(99, ())].into_iter().collect();
        assert_eq!(next_comment_line(&v, &comments, 0), None);
    }
}
