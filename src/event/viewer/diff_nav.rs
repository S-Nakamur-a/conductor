//! Pure navigation helpers for the unified diff view: locating the next/
//! previous changed block and the next/previous commented line, independent
//! of any `App` state so they're cheap to test in isolation.

use std::collections::HashMap;

use crate::diff_state::DiffLineTag;
use crate::viewer::UnifiedDiffEntry;

/// A changed (added or removed) line in the unified diff.
fn is_change_line(entry: &UnifiedDiffEntry) -> bool {
    matches!(
        entry,
        UnifiedDiffEntry::Line {
            tag: DiffLineTag::Insert | DiffLineTag::Delete,
            ..
        }
    )
}

/// `true` when index `i` is the first line of a contiguous block of changes.
fn is_change_block_start(lines: &[UnifiedDiffEntry], i: usize) -> bool {
    is_change_line(&lines[i]) && (i == 0 || !is_change_line(&lines[i - 1]))
}

/// Index of the next change block strictly after `from`.
pub(super) fn next_change_block(lines: &[UnifiedDiffEntry], from: usize) -> Option<usize> {
    (from + 1..lines.len()).find(|&i| is_change_block_start(lines, i))
}

/// Index of the previous change block strictly before `from`.
pub(super) fn prev_change_block(lines: &[UnifiedDiffEntry], from: usize) -> Option<usize> {
    (0..from).rev().find(|&i| is_change_block_start(lines, i))
}

/// `true` when the diff entry carries a review comment. A `Line` matches when
/// its new-file line number has a comment; a collapsed `ExpandableContext`
/// matches when any hidden line in its range does, so a comment that currently
/// sits inside a fold is still reachable (the jump lands on the fold to expand).
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

/// Index of the next commented diff entry strictly after `from`.
pub(super) fn next_comment_line<V>(
    lines: &[UnifiedDiffEntry],
    comments: &HashMap<usize, V>,
    from: usize,
) -> Option<usize> {
    (from + 1..lines.len()).find(|&i| entry_has_comment(&lines[i], comments))
}

/// Index of the previous commented diff entry strictly before `from`.
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
    /// A `Line` carrying an explicit new-file line number.
    fn line_no(tag: DiffLineTag, n: usize) -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag,
            new_line_no: Some(n),
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }
    /// A `Line` for a deletion (no new-file line number).
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
        assert!(!is_change_block_start(&[ins(), ins()], 1)); // mid-block
        assert!(!is_change_block_start(&[eq()], 0)); // not a change line
        assert!(is_change_block_start(&[sep(), ins()], 1)); // separator breaks runs
        assert!(!is_change_block_start(&[del(), ins()], 1)); // del+ins = one modified block
    }

    #[test]
    fn next_change_block_cases() {
        assert_eq!(next_change_block(&[], 0), None);
        assert_eq!(next_change_block(&[eq(), eq()], 0), None);
        let v = vec![eq(), ins(), eq(), ins(), eq()];
        assert_eq!(next_change_block(&v, 0), Some(1));
        assert_eq!(next_change_block(&v, 1), Some(3));
        assert_eq!(next_change_block(&v, 3), None);
        // From mid-block, skip the rest of the current block.
        let v = vec![ins(), ins(), ins(), eq(), ins()];
        assert_eq!(next_change_block(&v, 1), Some(4));
        // A separator splits two otherwise-adjacent change lines.
        assert_eq!(next_change_block(&[ins(), sep(), ins()], 0), Some(2));
    }

    #[test]
    fn prev_change_block_cases() {
        assert_eq!(prev_change_block(&[], 0), None);
        let v = vec![eq(), ins(), eq(), ins(), eq()];
        assert_eq!(prev_change_block(&v, 4), Some(3));
        assert_eq!(prev_change_block(&v, 3), Some(1));
        assert_eq!(prev_change_block(&v, 1), None);
        // From mid-block, land on the current block's start.
        assert_eq!(prev_change_block(&[ins(), ins(), ins()], 2), Some(0));
        assert_eq!(prev_change_block(&[ins(), ins()], 0), None);
    }

    #[test]
    fn comment_navigation() {
        let comments: HashMap<usize, ()> = [(5, ()), (8, ())].into_iter().collect();
        let v = vec![
            line_no(DiffLineTag::Equal, 4),  // 0: no comment
            line_no(DiffLineTag::Insert, 5), // 1: commented
            del_no_line(),                   // 2: delete, never commented
            line_no(DiffLineTag::Equal, 8),  // 3: commented
        ];
        assert_eq!(next_comment_line(&v, &comments, 0), Some(1));
        assert_eq!(next_comment_line(&v, &comments, 1), Some(3));
        assert_eq!(next_comment_line(&v, &comments, 3), None);
        assert_eq!(prev_comment_line(&v, &comments, 3), Some(1));
        assert_eq!(prev_comment_line(&v, &comments, 1), None);
        // Empty comment set → nothing found.
        let none: HashMap<usize, ()> = HashMap::new();
        assert_eq!(next_comment_line(&v, &none, 0), None);
    }

    #[test]
    fn delete_line_is_never_commented() {
        // A deletion has no new-file line number, so it can't carry a comment
        // even if that line number is in the map.
        let comments: HashMap<usize, ()> = [(1, ())].into_iter().collect();
        assert!(!entry_has_comment(&del_no_line(), &comments));
    }

    #[test]
    fn comment_hidden_in_fold_is_reachable() {
        // A comment on line 7 sits inside a collapsed context spanning 5..=10;
        // the jump should land on the fold so it can be expanded.
        let comments: HashMap<usize, ()> = [(7, ())].into_iter().collect();
        let v = vec![ins(), fold(5, 10)];
        assert_eq!(next_comment_line(&v, &comments, 0), Some(1));
        // No comment in the fold's range → not matched.
        let comments: HashMap<usize, ()> = [(99, ())].into_iter().collect();
        assert_eq!(next_comment_line(&v, &comments, 0), None);
    }
}
