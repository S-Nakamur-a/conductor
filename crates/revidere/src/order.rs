// 読む順。diff を 1 本の流れにして、重要度順に並べ替える。
//
// ループの主語は項目ではなく変更一覧（diff）。項目を回してその項目が触る行を出す形だと、
// 項目が漏らした行は画面から消える。変更一覧を回して行の持ち主を引く形なら、成果物が
// どれだけ壊れていても、ラベルの無い素の diff に退化するだけで行は消えない。
// order_covers_every_changed_line_exactly_once がこれを固定している。

use crate::annotate::Annotations;
use crate::diff::{Diff, DiffLine, Tag};
use crate::forest::Forest;
use crate::review::{Importance, Position};

/// 束の前後に添える文脈行の数。何をしている辺りなのかが分かる最小限。
const PAD: usize = 2;

/// 同じ持ち主でも、これより離れていたら別の束にする。
/// 繋げたままにすると、間の関係ない行まで一緒に出ることになる。
const GAP: usize = 4;

/// 流れの中の 1 行。
#[derive(Debug, Clone)]
pub struct OrderedLine {
    pub line: DiffLine,
    /// この束の持ち物か。false は読みやすさのために借りた前後の行。
    /// 借りた行は他の束にも現れうるが、持ち物の行はちょうど 1 回しか現れない。
    pub owned: bool,
}

/// 連続した 1 かたまり。
#[derive(Debug, Clone)]
pub struct Block {
    pub path: String,
    /// @@ の後ろに付く関数コンテキスト。行を持たない変更では空。
    pub hunk: String,
    /// 空なら「行を持たないファイル単位の変更」（バイナリ、モードのみ、rename）。
    pub lines: Vec<OrderedLine>,
    /// 行を持たない変更か。
    pub whole_file: bool,
}

/// 1 つの項目に属するかたまりの集まり。
#[derive(Debug, Clone)]
pub struct PlacedSection {
    /// sections() の添字。None は「どの項目も説明していない」束。
    pub section: Option<usize>,
    pub importance: Option<Importance>,
    pub blocks: Vec<Block>,
    /// この項目が持っている変更行の数。借りた行は数えない。
    pub changed: usize,
    /// 森の中での深さ。根が 0。持ち主の無い項目も 0。
    pub depth: usize,
}

impl PlacedSection {
    /// 項目は在るのに、その項目が指す行が diff に 1 つも無い状態。
    ///
    /// 黙って消すと「AI が在ると言った変更が無かった」ことに気付けないので、
    /// 空のまま残して画面に出す。
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// diff 全体を、読む順に並べ替えたもの。
#[derive(Debug, Clone, Default)]
pub struct ReadingOrder {
    pub sections: Vec<PlacedSection>,
    /// 項目の並びを決めた森。親子や、指す先の無かった関係を引くのに使う。
    pub forest: Forest,
}

impl ReadingOrder {
    /// 変更一覧を歩いて束に切り、重要度順に並べる。持ち主の無い束は末尾。
    ///
    /// 末尾に置くのは、置き場所が意味を持つのが「成果物が不完全なとき」
    /// だけだから。そのとき要るのは知ることであって最初に読むことではなく、
    /// 先頭に置くと失敗のたびにレビュー本体が下へ押し下げられる。
    pub fn build(diff: &Diff, ann: &Annotations) -> Self {
        // 持ち主ごとの束。None は持ち主なし。
        let mut by_owner: Vec<(Option<usize>, Vec<Block>, usize)> = Vec::new();
        let mut index_of: std::collections::HashMap<Option<usize>, usize> = Default::default();
        let mut push = |owner: Option<usize>, block: Block, changed: usize| {
            let i = *index_of.entry(owner).or_insert_with(|| {
                by_owner.push((owner, Vec::new(), 0));
                by_owner.len() - 1
            });
            by_owner[i].1.push(block);
            by_owner[i].2 += changed;
        };

        for file in &diff.files {
            let mut had_lines = false;
            for hunk in &file.hunks {
                for (owner, block, changed) in
                    split_hunk(&file.path, &hunk.header, &hunk.lines, ann)
                {
                    had_lines = true;
                    push(owner, block, changed);
                }
            }
            if !had_lines {
                // 行を持たない変更。ここで落とすと「変更が無かった」と
                // 区別が付かなくなる（FileDiff::positions と同じ判断）。
                let pos = Position::file(file.path.clone());
                push(
                    ann.owner(&pos),
                    Block {
                        path: file.path.clone(),
                        hunk: String::new(),
                        lines: Vec::new(),
                        whole_file: true,
                    },
                    1,
                );
            }
        }

        let forest = Forest::build(ann.sections());

        let mut sections: Vec<PlacedSection> = by_owner
            .into_iter()
            .map(|(section, blocks, changed)| PlacedSection {
                section,
                importance: section
                    .and_then(|i| ann.sections().get(i))
                    .map(|c| c.importance),
                blocks,
                changed,
                depth: section.map(|i| forest.depth(i)).unwrap_or(0),
            })
            .collect();

        // 指している行が diff に 1 つも無かった項目も、空の項目として残す。
        for (i, c) in ann.sections().iter().enumerate() {
            if !sections.iter().any(|s| s.section == Some(i)) {
                sections.push(PlacedSection {
                    section: Some(i),
                    importance: Some(c.importance),
                    blocks: Vec::new(),
                    changed: 0,
                    depth: forest.depth(i),
                });
            }
        }

        // 森を深さ優先で辿った順。根は重要度順に並び、子は自分の重要度に
        // 関わらず親の直後に来る（中核の隣にそのテストが出る）。関係が 1 本も
        // 無ければ全部が根なので、素の重要度順に退化する。持ち主なしは末尾。
        sections.sort_by_key(|s| match s.section {
            Some(i) => (0, forest.rank(i)),
            None => (1, 0),
        });
        ReadingOrder { sections, forest }
    }

    /// 流れが持っている変更箇所。借りた行は含まない。
    ///
    /// これが変更一覧と一致することが、このモジュールの存在理由。
    pub fn positions(&self) -> Vec<Position> {
        let mut out = Vec::new();
        for s in &self.sections {
            for b in &s.blocks {
                if b.whole_file {
                    out.push(Position::file(b.path.clone()));
                    continue;
                }
                for l in &b.lines {
                    if l.owned {
                        if let Some(p) = l.line.position(&b.path) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }

    /// 成果物の項目の添字から、それを置いた PlacedSection の位置を引く。
    /// 関係を辿って移動するのに使う。指す行が 1 つも無かった項目も空のまま
    /// 置いてあるので、どの項目にも必ず対応する位置がある。
    pub fn index_of(&self, section: usize) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| s.section == Some(section))
    }

    /// 変更行の総数。
    pub fn changed(&self) -> usize {
        self.sections.iter().map(|s| s.changed).sum()
    }

    /// どの項目も説明していない変更行の数。成果物が完全なら 0。
    ///
    /// 成果物に記録された説明もれ検査の数字ではなく、いま画面に出るものから
    /// 数え直した値。作業ツリーを見ている成果物では両者がずれる。
    pub fn unowned(&self) -> usize {
        self.sections
            .iter()
            .filter(|s| s.section.is_none())
            .map(|s| s.changed)
            .sum()
    }
}

/// 1 ハンクを、持ち主ごとの束に切る。
fn split_hunk(
    path: &str,
    header: &str,
    lines: &[DiffLine],
    ann: &Annotations,
) -> Vec<(Option<usize>, Block, usize)> {
    // 変更行の位置と持ち主。文脈行は持ち主を持たない。
    let owners: Vec<Option<Option<usize>>> = lines
        .iter()
        .map(|l| {
            if l.tag == Tag::Context {
                None
            } else {
                Some(l.position(path).and_then(|p| ann.owner(&p)))
            }
        })
        .collect();

    // 持ち主が同じ変更行の連なりを [先頭, 末尾] で拾う。
    let mut runs: Vec<(Option<usize>, usize, usize, usize)> = Vec::new();
    for (i, o) in owners.iter().enumerate() {
        let Some(owner) = *o else { continue };
        match runs.last_mut() {
            // 同じ持ち主で、間が GAP 行までなら同じ連なり。
            Some((prev, _, end, n)) if *prev == owner && i - *end <= GAP + 1 => {
                *end = i;
                *n += 1;
            }
            _ => runs.push((owner, i, i, 1)),
        }
    }

    runs.into_iter()
        .map(|(owner, start, end, changed)| {
            let from = start.saturating_sub(PAD);
            let to = (end + PAD).min(lines.len() - 1);
            let block = Block {
                path: path.to_string(),
                hunk: header.to_string(),
                lines: (from..=to)
                    .map(|i| OrderedLine {
                        line: lines[i].clone(),
                        // 借りた行は「この束の持ち物ではない」。持ち主の
                        // 違う変更行を借りることはある（実データでは稀）が、
                        // そのときも持ち物として二重に数えない。
                        owned: owners[i] == Some(owner),
                    })
                    .collect(),
                whole_file: false,
            };
            (owner, block, changed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;

    const DIFF: &str = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -8,6 +8,6 @@ fn outer()
 keep
-gone
+added
 tail
-also
+more
";

    const REVIEW_SECTIONS: &str = r#"[
        {"title":"追従","body":"b","importance":"follow","reason":"r",
         "ranges":[{"path":"src/a.rs","side":"old","start":9,"end":9}]},
        {"title":"中核","body":"b","importance":"core","reason":"r",
         "ranges":[{"path":"src/a.rs","side":"new","start":9,"end":9},
                   {"path":"src/a.rs","side":"new","start":11,"end":11},
                   {"path":"src/a.rs","side":"old","start":11,"end":11}]}
      ]"#;

    fn review() -> String {
        revidere_fixtures::review(REVIEW_SECTIONS)
    }

    fn built(review: &str) -> (diff::Diff, Annotations, ReadingOrder) {
        let d = diff::parse(DIFF);
        let a = Annotations::from_json(review).expect("読めること");
        let o = ReadingOrder::build(&d, &a);
        (d, a, o)
    }

    /// このモジュールの存在理由。成果物が何であれ、変更行はちょうど 1 回出る。
    #[test]
    fn 読む順は変更行をちょうど1回ずつ出す() {
        for review in [
            &review(),
            // 項目がゼロ
            &review().replace(
                r#""ranges":[{"path":"src/a.rs","side":"old","start":9,"end":9}]"#,
                r#""ranges":[]"#,
            ),
            // 実在しない行を指している
            &review().replace("\"start\":9,\"end\":9", "\"start\":900,\"end\":900"),
            // 関係が巡回している
            &review()
                .replace(
                    r#""title":"追従","body":"b","importance":"follow","reason":"r","#,
                    r#""title":"追従","body":"b","importance":"follow","reason":"r",
                     "relations":[{"to":"中核","reason":"r","primary":true}],"#,
                )
                .replace(
                    r#""title":"中核","body":"b","importance":"core","reason":"r","#,
                    r#""title":"中核","body":"b","importance":"core","reason":"r",
                     "relations":[{"to":"追従","reason":"r","primary":true}],"#,
                ),
            // 関係が実在しない項目を指している
            &review().replace(
                r#""title":"追従","body":"b","importance":"follow","reason":"r","#,
                r#""title":"追従","body":"b","importance":"follow","reason":"r",
                 "relations":[{"to":"架空","reason":"r","primary":true}],"#,
            ),
        ] {
            let (d, _, o) = built(review);
            let mut got = o.positions();
            let mut want = d.positions().into_iter().collect::<Vec<_>>();
            got.sort();
            want.sort();
            assert_eq!(got.len(), want.len(), "重複または取りこぼしがある: {got:?}");
            assert_eq!(got, want, "変更一覧と一致していない");
        }
    }

    /// 成果物が全滅しても、素の diff に退化するだけで行は消えない。
    #[test]
    fn 役に立たない成果物でも素のdiffには退化する() {
        let empty = revidere_fixtures::review("[]");
        let (d, _, o) = built(&empty);
        assert_eq!(o.positions().len(), d.positions().len());
        assert_eq!(o.unowned(), d.positions().len(), "全部が持ち主なしのはず");
        assert!(o.sections.iter().all(|s| s.section.is_none()));
    }

    #[test]
    fn 項目は重要度順に並ぶ() {
        // 成果物の並びは 追従 → 中核 だが、読む順では中核が先に来る。
        let (_, _, o) = built(&review());
        let order: Vec<Option<Importance>> = o.sections.iter().map(|s| s.importance).collect();
        assert_eq!(
            order,
            vec![Some(Importance::Core), Some(Importance::Follow)]
        );
    }

    #[test]
    fn 持ち主の無い束は末尾に来る() {
        // 追従が指していた old:9 を実在しない行へずらすと、その行は
        // 持ち主を失う。持ち主なしの項目は末尾。
        let review = review().replace(
            r#"{"path":"src/a.rs","side":"old","start":9,"end":9}"#,
            r#"{"path":"src/a.rs","side":"old","start":99,"end":99}"#,
        );
        let (_, _, o) = built(&review);
        assert_eq!(
            o.sections.last().map(|s| s.importance),
            Some(None),
            "持ち主なしが末尾に無い"
        );
        assert_eq!(o.unowned(), 1);
    }

    #[test]
    fn 借りた行は持ち物として数えない() {
        let (_, _, o) = built(&review());
        // 中核の項目には追従の行（-gone）が借り物として写り込むが、
        // 持ち物としては数えない。
        let core = &o.sections[0];
        let borrowed: usize = core
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .filter(|l| !l.owned && l.line.tag != Tag::Context)
            .count();
        assert!(borrowed > 0, "借り物の変更行が写り込んでいない");
        assert_eq!(core.changed, 3, "持ち物は new:9 / new:11 / old:11 の 3 行");
    }

    #[test]
    fn どこも指していない項目も空のまま残る() {
        let review = review().replace(
            r#"{"path":"src/a.rs","side":"old","start":9,"end":9}"#,
            r#"{"path":"src/nowhere.rs","side":"new","start":1,"end":1}"#,
        );
        let (_, a, o) = built(&review);
        let empty: Vec<&str> = o
            .sections
            .iter()
            .filter(|s| s.is_empty())
            .filter_map(|s| s.section)
            .map(|i| a.sections()[i].title.as_str())
            .collect();
        assert_eq!(empty, vec!["追従"]);
    }

    #[test]
    fn 行を持たないファイルは1つの束になる() {
        let d = diff::parse(
            "diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n",
        );
        let a = Annotations::from_json(&review()).unwrap();
        let o = ReadingOrder::build(&d, &a);
        assert_eq!(o.positions(), vec![Position::file("logo.png")]);
        let blocks: Vec<&Block> = o.sections.iter().flat_map(|s| &s.blocks).collect();
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].whole_file);
    }

    // 以下は split_hunk を直接呼んで PAD と GAP の境界を見る。diff テキストで
    // 行数を数えるより、行を直接組んだ方が境界がずれない。
    fn changed(new_line: u32) -> DiffLine {
        DiffLine {
            tag: Tag::Add,
            old_line: None,
            new_line: Some(new_line),
            text: String::new(),
        }
    }

    fn context(new_line: u32) -> DiffLine {
        DiffLine {
            tag: Tag::Context,
            old_line: Some(new_line),
            new_line: Some(new_line),
            text: String::new(),
        }
    }

    /// 一色（1 つの項目）が new:1 と new:end を指す成果物。間の行はすべて文脈行。
    fn single_owner_review(end: u32) -> String {
        format!(
            r#"{{
              "schema": {schema}, "base": "a", "head": "b",
              "overview": {{"problem":"p","change":"c","mechanism":"m","placement":"pl","scope":"s"}},
              "sections": [{{"title":"t","body":"b","importance":"core","reason":"r",
                "ranges":[{{"path":"a.rs","side":"new","start":1,"end":1}},
                          {{"path":"a.rs","side":"new","start":{end},"end":{end}}}]}}],
              "impacts": [],
              "coverage": {{"total":0,"classified":0,"unclassified":[],"conflicts":[],"unknown":[]}}
            }}"#,
            schema = crate::review::SCHEMA_VERSION,
        )
    }

    fn lines_up_to(last_new: u32) -> Vec<DiffLine> {
        (1..=last_new)
            .map(|n| {
                if n == 1 || n == last_new {
                    changed(n)
                } else {
                    context(n)
                }
            })
            .collect()
    }

    #[test]
    fn 間隔の内側の連なりは1つの束にまとまる() {
        // 間が 4 行の文脈行（index 差 5）までは同じ束にする。
        let lines = lines_up_to(6);
        let ann = Annotations::from_json(&single_owner_review(6)).unwrap();
        let runs = split_hunk("a.rs", "", &lines, &ann);
        assert_eq!(runs.len(), 1, "GAP の境界内なので 1 つの束のはず");
    }

    #[test]
    fn 間隔より離れた連なりは別の束に分かれる() {
        // 間が 5 行の文脈行（index 差 6）になると別の束にする。
        let lines = lines_up_to(7);
        let ann = Annotations::from_json(&single_owner_review(7)).unwrap();
        let runs = split_hunk("a.rs", "", &lines, &ann);
        assert_eq!(runs.len(), 2, "GAP を超えたので別の束のはず");
    }

    #[test]
    fn 束は前後に文脈行を2行ずつ添える() {
        // 前後に十分な文脈行があれば、束は PAD=2 行ずつ両側に伸びる。
        let lines: Vec<DiffLine> = (0..10)
            .map(|n| if n == 5 { changed(n) } else { context(n) })
            .collect();
        let review = revidere_fixtures::review(
            r#"[{"title":"t","body":"b","importance":"core","reason":"r",
                "ranges":[{"path":"a.rs","side":"new","start":5,"end":5}]}]"#,
        );
        let ann = Annotations::from_json(&review).unwrap();
        let runs = split_hunk("a.rs", "", &lines, &ann);
        assert_eq!(runs.len(), 1);
        let (_, block, _) = &runs[0];
        assert_eq!(block.lines.len(), 2 * PAD + 1, "変更行 1 つ + 両側 PAD 行");
    }
}
