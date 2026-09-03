// 項目の親子。primary の関係だけを辿ってできる森。
//
// 採取は多重、表示は森。結合テストが中核 2 つをまとめて検証していることは
// 実際にあるので関係は複数書けるが、多重グラフのまま並べると深さ優先の順が
// 一意に決まらず、「上から下まで読めば diff の全部」という背骨が作れない。

use crate::review::Section;
use std::collections::HashMap;

/// 項目の並びと親子。
#[derive(Debug, Clone, Default)]
pub struct Forest {
    /// 項目ごとの親。無い・解決できない・巡回するものは None。
    parent: Vec<Option<usize>>,
    /// 深さ優先で辿った並び。全ての項目がちょうど 1 回出る。
    order: Vec<usize>,
    /// 項目ごとの深さ。根が 0。
    depth: Vec<usize>,
    /// 親から見た子。並びは重要度順。
    children: Vec<Vec<usize>>,
    /// title から添字。関係の相手を引くのに使う。
    by_title: HashMap<String, usize>,
    /// 解決できなかった関係。(項目の添字, 指していた title)
    ///
    /// 黙って捨てない。モデルが「在る」と言った相手が無かったことは、説明もれ
    /// 検査の unknown と同じ種類の破れで、人が読んで気付ける必要がある。
    dangling: Vec<(usize, String)>,
}

impl Forest {
    pub fn build(sections: &[Section]) -> Self {
        let n = sections.len();
        // 同じ title が 2 つあるときに後着を採ると、関係の相手が成果物の
        // 並び順で変わる。
        let mut by_title: HashMap<&str, usize> = HashMap::with_capacity(n);
        for (i, c) in sections.iter().enumerate() {
            by_title.entry(c.title.as_str()).or_insert(i);
        }

        let mut parent = vec![None; n];
        let mut dangling = Vec::new();
        for (i, c) in sections.iter().enumerate() {
            let Some(rel) = c.relations.iter().find(|r| r.primary) else {
                continue;
            };
            match by_title.get(rel.to.as_str()) {
                // 自分を指すのは親なし扱い。
                Some(&p) if p != i => parent[i] = Some(p),
                Some(_) => {}
                None => dangling.push((i, rel.to.clone())),
            }
        }
        for (i, c) in sections.iter().enumerate() {
            for r in c.relations.iter().filter(|r| !r.primary) {
                if !by_title.contains_key(r.to.as_str()) {
                    dangling.push((i, r.to.clone()));
                }
            }
        }

        // 巡回を切る。親を辿って自分へ戻るなら、その項目を根にする。
        for i in 0..n {
            let mut at = parent[i];
            for _ in 0..n {
                match at {
                    None => break,
                    Some(p) if p == i => {
                        parent[i] = None;
                        break;
                    }
                    Some(p) => at = parent[p],
                }
            }
        }

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots: Vec<usize> = Vec::new();
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => children[*p].push(i),
                None => roots.push(i),
            }
        }
        let rank = |i: &usize| (sections[*i].importance as u8, *i);
        roots.sort_by_key(rank);
        for c in &mut children {
            c.sort_by_key(rank);
        }

        // 子は自分の重要度に関わらず親の直後に来る。テストが minor でも、
        // 検証している中核の隣に出るのはこのため。
        let mut order = Vec::with_capacity(n);
        let mut depth = vec![0; n];
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            for &c in children[i].iter().rev() {
                depth[c] = depth[i] + 1;
                stack.push(c);
            }
        }

        Forest {
            parent,
            order,
            depth,
            children,
            by_title: by_title
                .into_iter()
                .map(|(t, i)| (t.to_string(), i))
                .collect(),
            dangling,
        }
    }

    /// primary の関係が指す親。
    pub fn parent(&self, i: usize) -> Option<usize> {
        self.parent.get(i).copied().flatten()
    }

    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// 根からの深さ。並びに無い添字は 0。
    pub fn depth(&self, i: usize) -> usize {
        self.depth.get(i).copied().unwrap_or(0)
    }

    /// 並びの中での位置。項目を並べ替えるのに使う。
    pub fn rank(&self, i: usize) -> usize {
        self.order
            .iter()
            .position(|&x| x == i)
            .unwrap_or(self.order.len())
    }

    pub fn children(&self, i: usize) -> &[usize] {
        self.children.get(i).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 関係の相手を引く。
    pub fn resolve(&self, title: &str) -> Option<usize> {
        self.by_title.get(title).copied()
    }

    /// 指す先が無かった関係。
    pub fn dangling(&self) -> &[(usize, String)] {
        &self.dangling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{Importance, Relation};
    use Importance::{Core, Follow, Minor, Ripple};

    fn section(title: &str, imp: Importance, rels: &[(&str, bool)]) -> Section {
        Section {
            title: title.into(),
            body: String::new(),
            importance: imp,
            reason: None,
            ranges: Vec::new(),
            relations: rels
                .iter()
                .map(|&(to, primary)| Relation {
                    to: to.into(),
                    reason: "r".into(),
                    primary,
                })
                .collect(),
        }
    }

    /// 森の並びを、深さのインデント付きの見出し列にする。
    fn outline(f: &Forest, sections: &[Section]) -> Vec<String> {
        f.order()
            .iter()
            .map(|&i| format!("{}{}", "  ".repeat(f.depth(i)), sections[i].title))
            .collect()
    }

    #[test]
    fn 並びを決めるのは主の関係だけで子は親の直後に来る() {
        struct Case {
            name: &'static str,
            sections: Vec<Section>,
            outline: Vec<&'static str>,
            parents: Vec<Option<usize>>,
        }
        let case = |name, sections, outline, parents| Case {
            name,
            sections,
            outline,
            parents,
        };
        let cases = vec![
            case(
                // これをやらないと、テストが重要度順で末尾へ沈む。
                "子は重要度に関わらず親の直後",
                vec![
                    section("実装A", Core, &[]),
                    section("実装B", Core, &[]),
                    section("実装Aのテスト", Minor, &[("実装A", true)]),
                ],
                vec!["実装A", "  実装Aのテスト", "実装B"],
                vec![None, None, Some(0)],
            ),
            case(
                "親子の鎖は 3 段以上でも成り立つ",
                vec![
                    section("下位API", Core, &[]),
                    section("呼び出し側", Follow, &[("下位API", true)]),
                    section("呼び出し側のテスト", Minor, &[("呼び出し側", true)]),
                ],
                vec!["下位API", "  呼び出し側", "    呼び出し側のテスト"],
                vec![None, Some(0), Some(1)],
            ),
            case(
                // 結合テストが 2 つの中核をまとめて検証していても、背骨は 1 本。
                "primary 以外の関係は並びに影響しない",
                vec![
                    section("実装A", Core, &[]),
                    section("実装B", Core, &[]),
                    section("結合テスト", Minor, &[("実装A", true), ("実装B", false)]),
                ],
                vec!["実装A", "  結合テスト", "実装B"],
                vec![None, None, Some(0)],
            ),
            case(
                "関係が無ければ素の重要度順に落ちる",
                vec![
                    section("周辺の変更", Minor, &[]),
                    section("主目的", Core, &[]),
                    section("その帰結", Ripple, &[]),
                ],
                vec!["主目的", "その帰結", "周辺の変更"],
                vec![None, None, None],
            ),
            case(
                "同じ題が 2 つあれば先の項目に解決する",
                vec![
                    section("同じ名前", Core, &[]),
                    section("同じ名前", Core, &[]),
                    section("子", Minor, &[("同じ名前", true)]),
                ],
                vec!["同じ名前", "  子", "同じ名前"],
                vec![None, None, Some(0)],
            ),
            case(
                "自分を主に指す項目には親が無い",
                vec![section("A", Core, &[("A", true)])],
                vec!["A"],
                vec![None],
            ),
        ];

        for c in cases {
            let f = Forest::build(&c.sections);
            assert_eq!(outline(&f, &c.sections), c.outline, "{}", c.name);
            let got: Vec<Option<usize>> = (0..c.sections.len()).map(|i| f.parent(i)).collect();
            assert_eq!(got, c.parents, "{}: 親", c.name);
        }
    }

    /// 並びに出ない項目があると、その項目が持つ変更行が画面から消える。
    #[test]
    fn 循環しても項目を落とさず重複もしない() {
        let sections = vec![
            section("A", Core, &[("B", true)]),
            section("B", Core, &[("A", true)]),
        ];
        let f = Forest::build(&sections);
        let mut seen: Vec<usize> = f.order().to_vec();
        seen.sort();
        assert_eq!(seen, [0, 1]);
    }

    #[test]
    fn 知らない題を指す関係は主でも脇でも宙ぶらりんとして報告する() {
        let sections = vec![section("A", Core, &[("架空1", true), ("架空2", false)])];
        let f = Forest::build(&sections);
        assert_eq!(f.parent(0), None);
        assert_eq!(
            f.dangling(),
            [(0, "架空1".to_string()), (0, "架空2".to_string())]
        );
    }
}
