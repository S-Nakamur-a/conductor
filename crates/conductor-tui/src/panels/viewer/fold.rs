//! コードの折りたたみ — 畳める範囲の算出と、その開閉状態。
//!
//! 折りたたみは「ファイルの行」と「画面に出る行」を初めて食い違わせる。両者を別々に
//! 数える場所が 2 つできた時点で、描画とヒットテストと検索の着地点が静かにずれる。
//! そのため可視行の列挙 ([FoldState::visible_from]) と歩幅 ([FoldState::step]) は
//! ここにしかない。

use std::collections::HashSet;
use std::path::Path;

use conductor_core::symbol_index::language_for_ext;

/// 畳める 1 ブロック。行番号はどちらも 1 始まり。
///
/// 畳んだときに隠れるのは start+1..=end で、見出し行の start は残る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRange {
    pub start: usize,
    pub end: usize,
    /// 入れ子の深さ。最も外側が 1。
    pub depth: usize,
}

impl FoldRange {
    /// 深さは他の範囲との包含関係でしか決まらないので、単体では最も外側とみなす。
    fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            depth: 1,
        }
    }
}

/// 深さ単位でどこまで畳んだか。level は畳んだ段数で、全部開いていれば 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldDepth {
    pub level: usize,
    pub max: usize,
}

/// 開いているファイルの畳める範囲と、そのうち今閉じているもの。
#[derive(Debug, Default, Clone)]
pub struct FoldState {
    /// start 昇順・同一 start は無し。
    ranges: Vec<FoldRange>,
    /// 閉じている範囲の見出し行。
    collapsed: HashSet<usize>,
    /// 深さ単位でまとめて畳んだとき、閉じている中で最も浅い深さ。個別の開閉が入ると
    /// None に戻り、次の深さ操作は最も深い層からやり直す。
    collapsed_from_depth: Option<usize>,
    /// hidden[line-1]。collapsed から導出したキャッシュ。
    hidden: Vec<bool>,
    /// この状態が誰のものか。ファイルを跨いで開閉を持ち越さないための鍵。
    path: Option<String>,
}

impl FoldState {
    /// 算出済みの範囲を受け取る。
    ///
    /// path が変わっていなければ開閉状態は引き継ぐ (ファイルウォッチャーによる
    /// 読み直しで、読んでいた場所が勝手に開かないようにする)。引き継ぐのは「今も
    /// そこがブロックの見出しである」ものだけ。
    pub fn install(&mut self, ranges: Vec<FoldRange>, path: &str) {
        let same_file = self.path.as_deref() == Some(path);
        self.ranges = ranges;
        self.path = Some(path.to_string());
        if same_file {
            let starts: HashSet<usize> = self.ranges.iter().map(|r| r.start).collect();
            self.collapsed.retain(|line| starts.contains(line));
        } else {
            self.collapsed.clear();
            self.collapsed_from_depth = None;
        }
        self.rebuild_hidden();
    }

    /// 範囲も開閉も捨てる (読み込み失敗などで行が無いとき)。
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_foldable(&self, line_1: usize) -> bool {
        self.range_starting_at(line_1).is_some()
    }

    pub fn is_collapsed(&self, line_1: usize) -> bool {
        self.collapsed.contains(&line_1)
    }

    /// line_1 が閉じたブロックの内側にあって画面に出ないか。
    pub fn is_hidden(&self, line_1: usize) -> bool {
        line_1 > 0 && self.hidden.get(line_1 - 1).copied().unwrap_or(false)
    }

    /// line_1 を見出しとする閉じたブロックが隠している行数。
    pub fn hidden_count(&self, line_1: usize) -> Option<usize> {
        if !self.is_collapsed(line_1) {
            return None;
        }
        self.range_starting_at(line_1).map(|r| r.end - r.start)
    }

    /// line_1 のブロックを開閉する。見出し行でなければ、それを含む最も内側の
    /// ブロックを対象にする (vim の za と同じ)。
    pub fn toggle(&mut self, line_1: usize) -> bool {
        let Some(start) = self.target_start(line_1) else {
            return false;
        };
        if !self.collapsed.remove(&start) {
            self.collapsed.insert(start);
        }
        self.collapsed_from_depth = None;
        self.rebuild_hidden();
        true
    }

    pub fn close(&mut self, line_1: usize) -> bool {
        let Some(start) = self.target_start(line_1) else {
            return false;
        };
        let changed = self.collapsed.insert(start);
        if changed {
            self.collapsed_from_depth = None;
            self.rebuild_hidden();
        }
        changed
    }

    pub fn open(&mut self, line_1: usize) -> bool {
        let Some(start) = self.target_start(line_1) else {
            return false;
        };
        let changed = self.collapsed.remove(&start);
        if changed {
            self.collapsed_from_depth = None;
            self.rebuild_hidden();
        }
        changed
    }

    /// すべて開く (zR)。
    pub fn open_all(&mut self) {
        self.collapsed.clear();
        self.collapsed_from_depth = None;
        self.rebuild_hidden();
    }

    /// すべて閉じる (zM)。
    pub fn close_all(&mut self) {
        self.collapsed = self.ranges.iter().map(|r| r.start).collect();
        self.collapsed_from_depth = Some(1);
        self.rebuild_hidden();
    }

    /// まだ畳んでいない中で最も深い層を、全箇所まとめて畳む (zm)。
    pub fn collapse_deepest(&mut self) -> Option<FoldDepth> {
        let max = self.max_depth()?;
        let next = self.collapsed_from_depth.unwrap_or(max + 1);
        if next > 1 {
            for start in self.starts_at_depth(next - 1) {
                self.collapsed.insert(start);
            }
            self.collapsed_from_depth = Some(next - 1);
            self.rebuild_hidden();
        }
        Some(self.depth_level(max))
    }

    /// 深さ単位で畳んだ層のうち、最も浅いものを開き戻す (zr)。
    pub fn expand_shallowest(&mut self) -> Option<FoldDepth> {
        let max = self.max_depth()?;
        if let Some(depth) = self.collapsed_from_depth {
            for start in self.starts_at_depth(depth) {
                self.collapsed.remove(&start);
            }
            self.collapsed_from_depth = (depth < max).then_some(depth + 1);
            self.rebuild_hidden();
        }
        Some(self.depth_level(max))
    }

    /// line_1 が画面に出るまで、それを隠しているブロックを開く。
    ///
    /// 検索・定義ジャンプ・grep のヒットはどれも隠れた行に着地し得る。黙って別の行を
    /// 見せるより、畳んだ本人に開いて見せる方が驚きが小さい。
    pub fn reveal(&mut self, line_1: usize) -> bool {
        if !self.is_hidden(line_1) {
            return false;
        }
        let enclosing: Vec<usize> = self
            .ranges
            .iter()
            .filter(|r| r.start < line_1 && line_1 <= r.end)
            .map(|r| r.start)
            .collect();
        let mut changed = false;
        for start in enclosing {
            changed |= self.collapsed.remove(&start);
        }
        if changed {
            self.collapsed_from_depth = None;
            self.rebuild_hidden();
        }
        changed
    }

    /// start_1 から total までの、画面に出る行番号 (1 始まり)。
    pub fn visible_from(&self, start_1: usize, total: usize) -> impl Iterator<Item = usize> + '_ {
        (start_1.max(1)..=total).filter(move |l| !self.is_hidden(*l))
    }

    pub fn visible_count(&self, total: usize) -> usize {
        (1..=total).filter(|l| !self.is_hidden(*l)).count()
    }

    /// line_1 が可視行の何番目か (0 始まり)。
    pub fn visible_index(&self, line_1: usize, total: usize) -> usize {
        (1..=line_1.min(total))
            .filter(|l| !self.is_hidden(*l))
            .count()
            .saturating_sub(1)
    }

    /// line_1 から可視行を delta 行ぶん進める (負なら戻る)。端で止まる。
    ///
    /// j/k もホイールも Ctrl+D もここを通るので、「1 行進む」の意味が折りたたみの
    /// 有無で変わらない。
    pub fn step(&self, line_1: usize, delta: isize, total: usize) -> usize {
        if total == 0 {
            return 1;
        }
        let mut cur = line_1.clamp(1, total);
        for _ in 0..delta.unsigned_abs() {
            let next = if delta > 0 {
                self.next_visible(cur, total)
            } else {
                self.prev_visible(cur)
            };
            match next {
                Some(l) => cur = l,
                None => break,
            }
        }
        cur
    }

    pub fn next_visible(&self, line_1: usize, total: usize) -> Option<usize> {
        (line_1 + 1..=total).find(|l| !self.is_hidden(*l))
    }

    pub fn prev_visible(&self, line_1: usize) -> Option<usize> {
        (1..line_1).rev().find(|l| !self.is_hidden(*l))
    }

    pub fn last_visible(&self, total: usize) -> usize {
        (1..=total).rev().find(|l| !self.is_hidden(*l)).unwrap_or(1)
    }

    /// line_1 が隠れているなら、それを隠している見出し行へ寄せる。
    ///
    /// 隠れた行のすぐ上の可視行は定義上その行を隠している見出し行なので、探し直さない。
    pub fn visible_anchor(&self, line_1: usize) -> usize {
        if !self.is_hidden(line_1) {
            return line_1;
        }
        self.prev_visible(line_1).unwrap_or(1)
    }

    pub fn max_depth(&self) -> Option<usize> {
        self.ranges.iter().map(|r| r.depth).max()
    }

    /// 深さ単位で畳んでいる最中なら、その段数。個別の開閉だけなら None。
    pub fn depth(&self) -> Option<FoldDepth> {
        let max = self.max_depth()?;
        self.collapsed_from_depth.map(|_| self.depth_level(max))
    }

    fn starts_at_depth(&self, depth: usize) -> Vec<usize> {
        self.ranges
            .iter()
            .filter(|r| r.depth == depth)
            .map(|r| r.start)
            .collect()
    }

    fn depth_level(&self, max: usize) -> FoldDepth {
        let level = self
            .collapsed_from_depth
            .map_or(0, |d| (max + 1).saturating_sub(d));
        FoldDepth { level, max }
    }

    fn range_starting_at(&self, line_1: usize) -> Option<&FoldRange> {
        self.ranges.iter().find(|r| r.start == line_1)
    }

    /// 操作の対象になる見出し行。見出し行そのものを優先し、無ければ line_1 を含む
    /// 最も内側のブロック。
    fn target_start(&self, line_1: usize) -> Option<usize> {
        if self.is_foldable(line_1) {
            return Some(line_1);
        }
        self.ranges
            .iter()
            .filter(|r| r.start < line_1 && line_1 <= r.end)
            .max_by_key(|r| r.start)
            .map(|r| r.start)
    }

    fn rebuild_hidden(&mut self) {
        let total = self.ranges.iter().map(|r| r.end).max().unwrap_or(0);
        self.hidden = vec![false; total];
        for r in &self.ranges {
            if !self.collapsed.contains(&r.start) {
                continue;
            }
            for line in r.start + 1..=r.end {
                self.hidden[line - 1] = true;
            }
        }
    }
}

/// source の畳める範囲。tree-sitter で 1 つも取れなければインデントで求める。
pub fn compute(source: &str, path: &str) -> Vec<FoldRange> {
    let syntax = syntax_ranges(source, path).unwrap_or_default();
    let mut ranges = if syntax.is_empty() {
        normalize(indent_ranges(source))
    } else {
        syntax
    };
    assign_depths(&mut ranges);
    ranges
}

/// ranges は start 昇順で入れ子が壊れていないこと (normalize 済み) が前提。
fn assign_depths(ranges: &mut [FoldRange]) {
    let mut enclosing: Vec<usize> = Vec::new();
    for range in ranges.iter_mut() {
        enclosing.retain(|end| *end >= range.start);
        range.depth = enclosing.len() + 1;
        enclosing.push(range.end);
    }
}

fn syntax_ranges(source: &str, path: &str) -> Option<Vec<FoldRange>> {
    let ext = Path::new(path).extension()?.to_str()?;
    let language = language_for_ext(ext)?;
    let kinds = foldable_kinds(ext);

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;

    let mut out = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if kinds.contains(&node.kind()) {
            let start = node.start_position().row + 1;
            let end = node.end_position().row + 1;
            // 1 行に収まっているブロックは畳んでも何も減らない。
            if end > start {
                out.push(FoldRange::new(start, end));
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Some(normalize(out));
            }
        }
    }
}

/// 「複数行のノード全部」ではなく列挙なのは、式や引数リストまで畳めると 1 行あたりの
/// マーカーが増えすぎて、どれがブロックなのか読めなくなるため。
fn foldable_kinds(ext: &str) -> &'static [&'static str] {
    const RUST: &[&str] = &[
        "block",
        "declaration_list",
        "field_declaration_list",
        "ordered_field_declaration_list",
        "enum_variant_list",
        "match_block",
        "use_list",
        "field_initializer_list",
    ];
    const GO: &[&str] = &[
        "block",
        "field_declaration_list",
        "interface_type",
        "expression_switch_statement",
        "type_switch_statement",
        "select_statement",
        "import_spec_list",
        "const_declaration",
        "var_declaration",
    ];
    const TS: &[&str] = &[
        "statement_block",
        "class_body",
        "switch_body",
        "object",
        "array",
        "object_type",
        "enum_body",
        "named_imports",
    ];

    match ext {
        "rs" => RUST,
        "go" => GO,
        "ts" | "js" | "tsx" | "jsx" => TS,
        _ => &[],
    }
}

/// 文法を持たない言語向け。空行はブロックを切らないが、末尾の空行は範囲に含めない
/// (畳んだときに空行だけが残るのを避ける)。
fn indent_ranges(source: &str) -> Vec<FoldRange> {
    fn indent_of(line: &str) -> Option<usize> {
        let trimmed = line.trim_start();
        (!trimmed.is_empty()).then(|| line.len() - trimmed.len())
    }

    /// limit より浅いところまで積みを崩し、崩した範囲を確定させる。
    /// limit が None なら全部崩す (ファイル末尾)。
    fn close_to(
        stack: &mut Vec<(usize, usize)>,
        out: &mut Vec<FoldRange>,
        prev_nonblank: Option<usize>,
        limit: Option<usize>,
    ) {
        while let Some(&(top_indent, top_start)) = stack.last() {
            if limit.is_some_and(|ind| top_indent < ind) {
                break;
            }
            stack.pop();
            if let Some(last) = prev_nonblank
                && last > top_start
            {
                out.push(FoldRange::new(top_start + 1, last + 1));
            }
        }
    }

    let mut out = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut prev_nonblank: Option<usize> = None;

    for (i, line) in source.lines().enumerate() {
        let Some(indent) = indent_of(line) else {
            continue;
        };
        close_to(&mut stack, &mut out, prev_nonblank, Some(indent));
        stack.push((indent, i));
        prev_nonblank = Some(i);
    }
    close_to(&mut stack, &mut out, prev_nonblank, None);
    out
}

/// 見出し行がひとつなら折りたたみもひとつ。同じ行に 2 つあると開閉が collapsed の
/// 1 エントリを取り合い、開いたのにまだ隠れている行が出る。
fn normalize(mut ranges: Vec<FoldRange>) -> Vec<FoldRange> {
    ranges.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    ranges.dedup_by(|a, b| a.start == b.start);
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(source: &str, path: &str) -> FoldState {
        let mut state = FoldState::default();
        state.install(compute(source, path), path);
        state
    }

    fn spans(source: &str, path: &str) -> Vec<(usize, usize)> {
        compute(source, path)
            .iter()
            .map(|r| (r.start, r.end))
            .collect()
    }

    const NESTED_RS: &str = "fn a() {\n    if x {\n        y();\n    }\n}\n";

    #[test]
    fn 範囲の算出は言語ごとに決まる() {
        /// 説明, パス, ソース, 期待する範囲。
        type Case = (
            &'static str,
            &'static str,
            &'static str,
            &'static [(usize, usize)],
        );
        let cases: [Case; 7] = [
            ("rust の入れ子", "a.rs", NESTED_RS, &[(1, 5), (2, 4)]),
            ("1 行のブロックは畳めない", "a.rs", "fn a() { b(); }\n", &[]),
            (
                "末尾に改行が無くても最終行まで届く",
                "a.rs",
                "fn a() {\n    b();\n}",
                &[(1, 3)],
            ),
            (
                "構造体リテラルの本体",
                "a.rs",
                "fn a() {\n    S {\n        x: 1,\n    };\n}\n",
                &[(1, 5), (2, 4)],
            ),
            (
                "型定義と match の本体",
                "a.rs",
                "struct S {\n    x: u8,\n}\nfn f(v: S) {\n    match v {\n        _ => {}\n    }\n}\n",
                &[(1, 3), (4, 8), (5, 7)],
            ),
            (
                "対応しない言語はインデントに落ちる",
                "a.py",
                "def f():\n    if x:\n        y()\n    z()\n",
                &[(1, 4), (2, 3)],
            ),
            (
                "パースできない中身もインデントで畳める",
                "a.rs",
                "!!!\n    broken\n",
                &[(1, 2)],
            ),
        ];
        for (label, path, source, expected) in cases {
            assert_eq!(spans(source, path), expected, "{label}");
        }
    }

    #[test]
    fn インデントの範囲は空行の扱いが2通りある() {
        assert_eq!(
            spans("a:\n    b\n\n\n", "a.txt"),
            [(1, 2)],
            "末尾の空行は含めない"
        );
        assert_eq!(
            spans("a:\n    b\n\n    c\n", "a.txt"),
            [(1, 4)],
            "途中の空行はまたぐ"
        );
    }

    #[test]
    fn 畳むと本体は隠れ見出しは残る() {
        let mut fold = built("a:\n    b\n    c\n    d\n    e\n", "a.txt");
        assert!(fold.toggle(1));
        assert!(!fold.is_hidden(1));
        for line in 2..=5 {
            assert!(fold.is_hidden(line), "{line}");
        }
        assert_eq!(fold.hidden_count(1), Some(4));
    }

    #[test]
    fn 外側を開いても内側の畳みは残る() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(2);
        fold.close(1);
        fold.open(1);
        assert!(fold.is_collapsed(2));
        assert!(fold.is_hidden(3));
    }

    #[test]
    fn ブロックの中での操作は最も内側を対象にする() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(3);
        assert!(fold.is_collapsed(2), "内側でなく外側が閉じた");
        assert!(!fold.is_collapsed(1));
    }

    #[test]
    fn 可視行の対応は畳まれた行を飛ばす() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(2);
        assert_eq!(fold.visible_from(1, 5).collect::<Vec<_>>(), [1, 2, 5]);
        assert_eq!(fold.visible_count(5), 3);
        assert_eq!(fold.visible_index(5, 5), 2);
        assert_eq!(fold.step(2, 1, 5), 5);
        assert_eq!(fold.step(5, -1, 5), 2);
        assert_eq!(fold.last_visible(5), 5);
    }

    #[test]
    fn revealはその行を隠す畳みだけを開く() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(1);
        fold.close(2);
        assert!(fold.reveal(3));
        assert!(!fold.is_hidden(3));
        assert!(!fold.reveal(3), "既に見えている行では何も起きない");
    }

    #[test]
    fn 隠れた行の寄せ先はその見出し() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(2);
        assert_eq!(fold.visible_anchor(3), 2);
        assert_eq!(fold.visible_anchor(5), 5);
    }

    #[test]
    fn 同じファイルの読み直しは畳みを保ち消えた畳みは落とす() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(1);
        fold.close(2);

        fold.install(compute(NESTED_RS, "a.rs"), "a.rs");
        assert!(fold.is_collapsed(1));
        assert!(fold.is_collapsed(2));

        // 内側の if を消すと、その見出しはもうブロックではない。
        fold.install(compute("fn a() {\n    y();\n}\n", "a.rs"), "a.rs");
        assert!(fold.is_collapsed(1));
        assert!(!fold.is_collapsed(2));
    }

    #[test]
    fn ファイルを移ると畳みは捨てる() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close(1);
        fold.install(compute(NESTED_RS, "b.rs"), "b.rs");
        assert!(!fold.is_collapsed(1));
    }

    #[test]
    fn 畳みの無いファイルは全行が見える() {
        let fold = FoldState::default();
        assert_eq!(fold.visible_count(4), 4);
        assert_eq!(fold.visible_from(1, 4).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(fold.step(1, 2, 4), 3);
        assert_eq!(fold.max_depth(), None);
    }

    #[test]
    fn 深さ単位の畳みはその段の全ブロックに効く() {
        let source = "fn a() {\n    if x {\n        y();\n    }\n}\nfn b() {\n    if z {\n        w();\n    }\n}\n";
        let mut fold = built(source, "a.rs");
        assert_eq!(fold.max_depth(), Some(2));

        let depth = fold.collapse_deepest().unwrap();
        assert_eq!((depth.level, depth.max), (1, 2));
        assert!(fold.is_collapsed(2) && fold.is_collapsed(7), "同じ段が両方");
        assert!(!fold.is_collapsed(1));

        let depth = fold.collapse_deepest().unwrap();
        assert_eq!(depth.level, 2);
        assert!(fold.is_collapsed(1) && fold.is_collapsed(6));

        // 両端で頭打ち。
        assert_eq!(fold.collapse_deepest().unwrap().level, 2);
        fold.expand_shallowest();
        fold.expand_shallowest();
        assert_eq!(fold.expand_shallowest().unwrap().level, 0);
    }

    #[test]
    fn 段数は深さ単位で畳んでいる間だけ出る() {
        let mut fold = built(NESTED_RS, "a.rs");
        assert_eq!(fold.depth(), None);
        fold.collapse_deepest();
        assert!(fold.depth().is_some());
        fold.toggle(1);
        assert_eq!(fold.depth(), None, "手で 1 つ畳むと深さの位置は失われる");
        assert!(fold.is_collapsed(2), "深さ単位の畳みはそのまま残る");
    }

    #[test]
    fn 畳みの無いファイルには深さが無い() {
        let mut fold = FoldState::default();
        assert_eq!(fold.collapse_deepest(), None);
        assert_eq!(fold.expand_shallowest(), None);
    }

    #[test]
    fn 全部畳むと深さは最も浅い段に来る() {
        let mut fold = built(NESTED_RS, "a.rs");
        fold.close_all();
        assert_eq!(fold.depth().map(|d| d.level), Some(2));
        assert_eq!(fold.expand_shallowest().map(|d| d.level), Some(1));
        fold.open_all();
        assert_eq!(fold.depth(), None);
        assert!(!fold.is_hidden(3));
    }
}
