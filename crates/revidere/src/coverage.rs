// 説明もれ検査。ライブラリの存在理由はここにある。
//
// 見つかるのは分類漏れまでで、誤分類は機械では見つからない (項目の理由を
// 人が読んで見つける)。

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

    fn section(title: &str, ranges: &[(&str, Side, u32, u32)]) -> Section {
        Section {
            title: title.into(),
            body: "b".into(),
            importance: Importance::Core,
            reason: None,
            ranges: ranges
                .iter()
                .map(|&(path, side, start, end)| Range {
                    path: path.into(),
                    side,
                    start: Some(start),
                    end: Some(end),
                })
                .collect(),
            relations: Vec::new(),
        }
    }

    fn ledger(items: &[(&str, Side, u32)]) -> BTreeSet<Position> {
        items
            .iter()
            .map(|&(p, s, n)| Position::new(p, s, n))
            .collect()
    }

    #[test]
    fn 検査は3種類の破れを混ぜずに数える() {
        let x = "src/x.rs";
        let new4 = || Position::new(x, Side::New, 4);
        let cases = vec![
            (
                "全ての位置がちょうど 1 項目に属せば完全",
                vec![(x, Side::New, 4), (x, Side::New, 5)],
                vec![section("t", &[(x, Side::New, 4, 5)])],
                2,
                vec![],
                vec![],
                vec![],
            ),
            (
                "誰も名乗らない位置は未分類",
                vec![(x, Side::New, 4), (x, Side::New, 40)],
                vec![section("t", &[(x, Side::New, 4, 4)])],
                1,
                vec![Position::new(x, Side::New, 40)],
                vec![],
                vec![],
            ),
            (
                // 後像行番号だけで塗ろうとすると削除行が残る、という一番効く検査。
                "削除行は後像側の範囲では覆えない",
                vec![(x, Side::New, 4), (x, Side::Old, 4)],
                vec![section("t", &[(x, Side::New, 1, 6)])],
                1,
                vec![Position::new(x, Side::Old, 4)],
                vec![],
                [1, 2, 3, 5, 6]
                    .map(|n| Position::new(x, Side::New, n))
                    .to_vec(),
            ),
            (
                "2 項目が名乗れば衝突で、分類済みには数えない",
                vec![(x, Side::New, 4)],
                vec![
                    section("片方", &[(x, Side::New, 4, 4)]),
                    section("もう片方", &[(x, Side::New, 4, 4)]),
                ],
                0,
                vec![],
                vec![new4()],
                vec![],
            ),
            (
                // 同じ項目の中の重なりを衝突に出すと、本当の取り合いと区別が付かない。
                "同じ項目の中の範囲の重なりは衝突ではない",
                vec![(x, Side::New, 4), (x, Side::New, 5)],
                vec![section("t", &[(x, Side::New, 4, 5), (x, Side::New, 5, 5)])],
                2,
                vec![],
                vec![],
                vec![],
            ),
            (
                "台帳の外を指す範囲は未知 (行番号の作り話を捕まえる)",
                vec![(x, Side::New, 4)],
                vec![section("t", &[(x, Side::New, 4, 6)])],
                1,
                vec![],
                vec![],
                vec![
                    Position::new(x, Side::New, 5),
                    Position::new(x, Side::New, 6),
                ],
            ),
            (
                "項目が無ければ台帳は丸ごと未分類",
                vec![(x, Side::New, 4)],
                vec![],
                0,
                vec![new4()],
                vec![],
                vec![],
            ),
        ];

        for (name, items, sections, classified, unclassified, conflicts, unknown) in cases {
            let l = ledger(&items);
            let c = check(&l, &sections);
            assert_eq!(c.total, l.len(), "{name}: total");
            assert_eq!(c.classified, classified, "{name}: classified");
            assert_eq!(c.unclassified, unclassified, "{name}: unclassified");
            assert_eq!(c.conflicts, conflicts, "{name}: conflicts");
            assert_eq!(c.unknown, unknown, "{name}: unknown");
            let complete = unclassified.is_empty() && conflicts.is_empty() && unknown.is_empty();
            assert_eq!(c.is_complete(), complete, "{name}: is_complete");
        }
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
