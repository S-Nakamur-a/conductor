// 節の親子。primary の関係だけを辿ってできる森。
//
// 関係は複数書ける。結合テストが中核 2 つをまとめて検証していることは
// 実際にあるし、それを 1 本に潰すのは嘘になる。ただし読む順が辿るのは
// primary 1 本だけにしてある。多重グラフのまま並べようとすると深さ優先の
// 順が一意に決まらず、「上から下まで読めば diff の全部」という背骨が作れない。
//
// 採取は多重、表示は森。primary 以外は節が 2 か所に出る形ではなく、
// 節の脇の言及として出す。

use crate::review::Section;
use std::collections::HashMap;

/// 節の並びと親子。
#[derive(Debug, Clone, Default)]
pub struct Forest {
    /// 節ごとの親。無い・解決できない・巡回するものは None。
    parent: Vec<Option<usize>>,
    /// 深さ優先で辿った節の並び。全ての節がちょうど 1 回出る。
    order: Vec<usize>,
    /// 節ごとの深さ。根が 0。
    depth: Vec<usize>,
    /// 親から見た子。並びは重要度順。
    children: Vec<Vec<usize>>,
    /// title から添字。関係の相手を引くのに使う。
    by_title: HashMap<String, usize>,
    /// 解決できなかった関係。(節の添字, 指していた title)
    ///
    /// 黙って捨てない。モデルが「在る」と言った相手が無かったことは、
    /// 充足検査の unknown と同じ種類の破れで、人が読んで気付ける必要がある。
    dangling: Vec<(usize, String)>,
}

impl Forest {
    pub fn build(sections: &[Section]) -> Self {
        let n = sections.len();
        // title から添字。同じ title が 2 つあれば先着を採る。
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
        // primary 以外にも、指す先が無いものがある。
        for (i, c) in sections.iter().enumerate() {
            for r in c.relations.iter().filter(|r| !r.primary) {
                if !by_title.contains_key(r.to.as_str()) {
                    dangling.push((i, r.to.clone()));
                }
            }
        }

        // 巡回を切る。親を辿って自分へ戻るなら、その節を根にする。
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

        // 子の一覧。並びは重要度順、同じなら成果物の並び。
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

        // 深さ優先。子は自分の重要度に関わらず親の直後に来る。
        // テストが minor でも、検証している中核の隣に出るのはこのため。
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

    /// 深さ優先で辿った節の並び。
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// 根からの深さ。並びに無い添字は 0。
    pub fn depth(&self, i: usize) -> usize {
        self.depth.get(i).copied().unwrap_or(0)
    }

    /// 並びの中での位置。節を並べ替えるのに使う。
    pub fn rank(&self, i: usize) -> usize {
        self.order
            .iter()
            .position(|&x| x == i)
            .unwrap_or(self.order.len())
    }

    /// この節を親に持つ節。並びは重要度順。
    pub fn children(&self, i: usize) -> &[usize] {
        self.children.get(i).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// title から節の添字。関係の相手を引く。
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

    fn section(title: &str, imp: Importance, rels: Vec<(&str, bool)>) -> Section {
        Section {
            title: title.into(),
            body: String::new(),
            importance: imp,
            reason: None,
            ranges: Vec::new(),
            relations: rels
                .into_iter()
                .map(|(to, primary)| Relation {
                    to: to.into(),
                    reason: "r".into(),
                    primary,
                })
                .collect(),
        }
    }

    /// 森の並びを、深さのインデント付きの見出し列にする。目視で確認しやすくする。
    fn outline(f: &Forest, sections: &[Section]) -> Vec<String> {
        f.order()
            .iter()
            .map(|&i| format!("{}{}", "  ".repeat(f.depth(i)), sections[i].title))
            .collect()
    }

    /// 子は自分の重要度に関わらず親の直後。周辺のテストでも、検証している
    /// 中核の隣に出る。これをやらないと重要度順で末尾へ沈む。
    #[test]
    fn a_child_follows_its_parent_regardless_of_importance() {
        let sections = vec![
            section("実装A", Importance::Core, vec![]),
            section("実装B", Importance::Core, vec![]),
            section("実装Aのテスト", Importance::Minor, vec![("実装A", true)]),
        ];
        let f = Forest::build(&sections);
        assert_eq!(
            outline(&f, &sections),
            ["実装A", "  実装Aのテスト", "実装B"]
        );
    }

    /// 親子の鎖は 3 段以上でも森として成り立つ。深さは制限しない。
    #[test]
    fn a_chain_of_three_or_more_is_a_forest_too() {
        let sections = vec![
            section("下位API", Importance::Core, vec![]),
            section("呼び出し側", Importance::Follow, vec![("下位API", true)]),
            section(
                "呼び出し側のテスト",
                Importance::Minor,
                vec![("呼び出し側", true)],
            ),
        ];
        let f = Forest::build(&sections);
        assert_eq!(
            outline(&f, &sections),
            ["下位API", "  呼び出し側", "    呼び出し側のテスト"]
        );
    }

    /// primary 以外の関係は並びに影響しない。結合テストが 2 つの中核を
    /// まとめて検証していても、背骨は primary の 1 本だけ。
    #[test]
    fn only_the_primary_relation_shapes_the_order() {
        let sections = vec![
            section("実装A", Importance::Core, vec![]),
            section("実装B", Importance::Core, vec![]),
            section(
                "結合テスト",
                Importance::Minor,
                vec![("実装A", true), ("実装B", false)],
            ),
        ];
        let f = Forest::build(&sections);
        assert_eq!(outline(&f, &sections), ["実装A", "  結合テスト", "実装B"]);
        assert_eq!(f.parent(2), Some(0));
    }

    /// 親を辿って巡回しても、全ての節がちょうど 1 回だけ並びに出る。
    /// 出ない節があると、その節が持つ変更行が画面から消える。
    #[test]
    fn a_cycle_does_not_lose_or_repeat_any_section() {
        let sections = vec![
            section("A", Importance::Core, vec![("B", true)]),
            section("B", Importance::Core, vec![("A", true)]),
        ];
        let f = Forest::build(&sections);
        let mut seen: Vec<usize> = f.order().to_vec();
        seen.sort();
        assert_eq!(seen, [0, 1]);
    }

    #[test]
    fn a_section_naming_itself_as_primary_has_no_parent() {
        let sections = vec![section("A", Importance::Core, vec![("A", true)])];
        let f = Forest::build(&sections);
        assert_eq!(f.parent(0), None);
        assert_eq!(f.order(), [0]);
    }

    /// 実在しない相手を指したことは黙って捨てず dangling に残す。
    /// 捨てると、モデルが在ると言った節が無かったことに気付けない。
    #[test]
    fn a_relation_pointing_at_an_unknown_title_is_reported_as_dangling() {
        let sections = vec![section("A", Importance::Core, vec![("架空の節", true)])];
        let f = Forest::build(&sections);
        assert_eq!(f.parent(0), None);
        assert_eq!(f.dangling(), [(0, "架空の節".to_string())]);
    }

    /// 同じ title が 2 つあるときは先着で解決する。後着を採ると、関係の相手が
    /// 成果物の並び順で変わる。
    #[test]
    fn a_duplicate_title_resolves_to_the_first_section() {
        let sections = vec![
            section("同じ名前", Importance::Core, vec![]),
            section("同じ名前", Importance::Core, vec![]),
            section("子", Importance::Minor, vec![("同じ名前", true)]),
        ];
        let f = Forest::build(&sections);
        assert_eq!(f.parent(2), Some(0));
    }

    #[test]
    fn no_relations_at_all_falls_back_to_plain_importance_order() {
        let sections = vec![
            section("周辺の変更", Importance::Minor, vec![]),
            section("主目的", Importance::Core, vec![]),
            section("その帰結", Importance::Ripple, vec![]),
        ];
        let f = Forest::build(&sections);
        assert_eq!(outline(&f, &sections), ["主目的", "その帰結", "周辺の変更"]);
    }
}
