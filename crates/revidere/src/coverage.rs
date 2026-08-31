// 説明もれ検査。ライブラリの存在理由はここにある。
//
// 「diff の全ての変更に何らかの色が付いている」を守るのはこの関数だけで、
// 破れたときに黙って通さないことが唯一の仕事。ただし見つかるのは分類漏れまでで、
// 誤分類は機械では見つからない（項目の理由を人が読んで見つける）。

use crate::review::{Coverage, Position, Section};
use std::collections::{BTreeMap, BTreeSet};

/// 変更一覧と項目の集合を突き合わせる。
///
/// 3 種類の破れ（Coverage の各欄）は混ぜずに別々に数える。混ぜると原因が追えない。
pub fn check(ledger: &BTreeSet<Position>, sections: &[Section]) -> Coverage {
    let mut hits: BTreeMap<Position, usize> = BTreeMap::new();
    let mut unknown: BTreeSet<Position> = BTreeSet::new();

    for ctx in sections {
        // 同じ項目が範囲を重ねて書いても二重計上しない。二重計上を conflicts に
        // 出すと、本当に別々の項目が同じ行を取り合っている場合と区別が付かなくなる。
        let mut seen_here: BTreeSet<Position> = BTreeSet::new();
        for range in &ctx.ranges {
            for p in range.positions() {
                if !seen_here.insert(p.clone()) {
                    continue;
                }
                if ledger.contains(&p) {
                    *hits.entry(p).or_insert(0) += 1;
                } else {
                    unknown.insert(p);
                }
            }
        }
    }

    let unclassified: Vec<Position> = ledger
        .iter()
        .filter(|p| !hits.contains_key(*p))
        .cloned()
        .collect();
    let conflicts: Vec<Position> = hits
        .iter()
        .filter(|(_, &n)| n > 1)
        .map(|(p, _)| p.clone())
        .collect();
    let classified = hits.values().filter(|&&n| n == 1).count();

    Coverage {
        total: ledger.len(),
        classified,
        unclassified,
        conflicts,
        unknown: unknown.into_iter().collect(),
    }
}

/// 説明なしの位置だけを、モデルへ差し戻すための変更一覧へ畳む。
///
/// 全体をやり直させると、既に正しく分類できた部分まで揺れる。
pub fn gap_summary(unclassified: &[Position]) -> String {
    let mut by_file: BTreeMap<(&str, crate::review::Side), Vec<u32>> = BTreeMap::new();
    let mut file_level: Vec<&str> = Vec::new();
    for p in unclassified {
        match p.line {
            Some(n) => by_file
                .entry((p.path.as_str(), p.side))
                .or_default()
                .push(n),
            None => file_level.push(p.path.as_str()),
        }
    }
    let mut out = String::new();
    for path in file_level {
        out.push_str(&format!("{path} file\n"));
    }
    for ((path, side), mut lines) in by_file {
        lines.sort_unstable();
        let side = match side {
            crate::review::Side::New => "new",
            crate::review::Side::Old => "old",
            crate::review::Side::File => "file",
        };
        let list: Vec<String> = lines.iter().map(|n| n.to_string()).collect();
        out.push_str(&format!("{path} {side}: {}\n", list.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{Importance, Range, Side};

    fn section(title: &str, ranges: Vec<Range>) -> Section {
        Section {
            title: title.into(),
            body: "b".into(),
            importance: Importance::Core,
            reason: None,
            ranges,
            relations: Vec::new(),
        }
    }

    fn range(path: &str, side: Side, start: u32, end: u32) -> Range {
        Range {
            path: path.into(),
            side,
            start: Some(start),
            end: Some(end),
        }
    }

    fn ledger(items: &[(&str, Side, u32)]) -> BTreeSet<Position> {
        items
            .iter()
            .map(|&(p, s, n)| Position::new(p, s, n))
            .collect()
    }

    #[test]
    fn 台帳の全位置がちょうど1項目に属せば完全() {
        let l = ledger(&[("src/x.rs", Side::New, 4), ("src/x.rs", Side::New, 5)]);
        let c = check(
            &l,
            &[section("t", vec![range("src/x.rs", Side::New, 4, 5)])],
        );
        assert!(c.is_complete());
        assert_eq!((c.total, c.classified), (2, 2));
    }

    #[test]
    fn 誰も名乗らない位置は落とさず未分類にする() {
        let l = ledger(&[("src/x.rs", Side::New, 4), ("src/x.rs", Side::New, 40)]);
        let c = check(
            &l,
            &[section("t", vec![range("src/x.rs", Side::New, 4, 4)])],
        );
        assert_eq!(
            c.unclassified,
            vec![Position::new("src/x.rs", Side::New, 40)]
        );
        assert_eq!(c.classified, 1);
        assert!(!c.is_complete());
    }

    #[test]
    fn 削除行は後像側の範囲では覆えない() {
        // 後像行番号だけで塗ろうとすると削除行が残る、という一番効く検査。
        let l = ledger(&[("src/x.rs", Side::New, 4), ("src/x.rs", Side::Old, 4)]);
        let c = check(
            &l,
            &[section("t", vec![range("src/x.rs", Side::New, 1, 100)])],
        );
        assert_eq!(
            c.unclassified,
            vec![Position::new("src/x.rs", Side::Old, 4)]
        );
    }

    #[test]
    fn 同じ位置を2項目が名乗れば衝突であって分類済みではない() {
        let l = ledger(&[("src/x.rs", Side::New, 4)]);
        let c = check(
            &l,
            &[
                section("片方", vec![range("src/x.rs", Side::New, 4, 4)]),
                section("もう片方", vec![range("src/x.rs", Side::New, 4, 4)]),
            ],
        );
        assert_eq!(c.conflicts, vec![Position::new("src/x.rs", Side::New, 4)]);
        // 取り合った位置は classified 側からは除外される。
        assert_eq!(c.classified, 0);
        assert!(!c.is_complete());
    }

    #[test]
    fn 同じ項目の中の範囲の重なりは衝突ではない() {
        // 同じ項目が範囲を重ねて書いても、二重計上を conflicts に出さない。
        // 出すと本当の取り合いと区別が付かなくなる。
        let l = ledger(&[("src/x.rs", Side::New, 4), ("src/x.rs", Side::New, 5)]);
        let c = check(
            &l,
            &[section(
                "t",
                vec![
                    range("src/x.rs", Side::New, 4, 5),
                    range("src/x.rs", Side::New, 5, 5),
                ],
            )],
        );
        assert!(c.conflicts.is_empty());
        assert!(c.is_complete());
    }

    #[test]
    fn 台帳の外を指す範囲は未知として報告する() {
        // 行番号を作られたときに気付けるようにする。
        let l = ledger(&[("src/x.rs", Side::New, 4)]);
        let c = check(
            &l,
            &[section("t", vec![range("src/x.rs", Side::New, 4, 6)])],
        );
        assert_eq!(
            c.unknown,
            vec![
                Position::new("src/x.rs", Side::New, 5),
                Position::new("src/x.rs", Side::New, 6),
            ]
        );
        assert!(!c.is_complete());
    }

    #[test]
    fn 項目が無ければ台帳は丸ごと未分類になる() {
        let l = ledger(&[("src/x.rs", Side::New, 4)]);
        let c = check(&l, &[]);
        assert_eq!(c.unclassified.len(), 1);
        assert_eq!(c.classified, 0);
        assert!(!c.is_complete());
    }

    #[test]
    fn 説明もれの要約はファイルと側でまとめ行を並べる() {
        let gaps = vec![
            Position::new("src/x.rs", Side::New, 8),
            Position::new("src/x.rs", Side::New, 3),
            Position::new("src/x.rs", Side::Old, 20),
            Position::file("logo.png"),
        ];
        let s = gap_summary(&gaps);
        assert!(s.contains("src/x.rs new: 3, 8"), "{s}");
        assert!(s.contains("src/x.rs old: 20"), "{s}");
        assert!(s.contains("logo.png file"), "{s}");
    }
}
