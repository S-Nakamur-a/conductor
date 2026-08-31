//! コードの折りたたみ — 折りたたみ可能な範囲の算出と、その開閉状態。
//!
//! 範囲は tree-sitter で求める。文法を持たない言語やブロックを1つも取れなかった
//! ファイルは、インデント幅から求めるフォールバックに落ちる。
//!
//! # 行の可視性はここが唯一の答えを持つ
//!
//! 折りたたみは「ファイルの行」と「画面に出る行」を初めて食い違わせる。両者を
//! 別々に数える場所が2つできた時点で、描画とマウスのヒットテスト、検索の着地点が
//! 静かにずれる。そのため可視行の列挙 ([FoldState::visible_from]) と歩幅
//! ([FoldState::step]) はここにしかなく、描画・キー・マウスはすべてこれを通る。

use std::collections::HashSet;
use std::path::Path;

/// 折りたたみ可能な1ブロック。行番号はどちらも1始まり。
///
/// 畳んだときに隠れるのは start+1..=end で、start（ブロックの見出し行）は
/// 残る。閉じ括弧の行まで隠して見出し行に "{ … }" を出すのは VSCode と同じ。
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

/// ホバー中の折りたたみ範囲の、その行での位置。マーカーの1列だけで範囲の
/// 端から端までを示すために使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldRule {
    /// ホバーされている見出し行。
    Head,
    /// 範囲の途中。
    Body,
    /// 範囲の最終行。
    Tail,
}

/// 開いているファイルの折りたたみ範囲と、そのうち今閉じているもの。
#[derive(Debug, Default, Clone)]
pub struct FoldState {
    /// start 昇順・同一 start は無し。
    ranges: Vec<FoldRange>,
    /// 閉じている範囲の見出し行。
    collapsed: HashSet<usize>,
    /// 深さ単位でまとめて畳んだとき、閉じている中で最も浅い深さ。個別の開閉が
    /// 入ると None に戻り、次の深さ操作は最も深い層からやり直す。
    collapsed_from_depth: Option<usize>,
    /// hidden[line-1]。collapsed から導出したキャッシュで、collapsed を
    /// 触ったら必ず rebuild_hidden() で作り直す。
    hidden: Vec<bool>,
    /// この状態が誰のものか。ファイルを跨いで開閉を持ち越さないための鍵。
    path: Option<String>,
    /// マウスが乗っているマーカーの見出し行。範囲を罫線で示すためだけの
    /// 状態で、開閉には関わらない。
    hover: Option<usize>,
}

impl FoldState {
    /// source から範囲を計算し直す。
    ///
    /// path が変わっていなければ開閉状態は引き継ぐ（ファイルウォッチャーによる
    /// 再読み込みで、読んでいた場所が勝手に開かないようにする）。引き継ぐのは
    /// 「今もそこがブロックの見出しである」ものだけで、編集で消えた折りたたみは
    /// 落ちる。別のファイルなら開閉は捨てる。
    pub fn rebuild(&mut self, source: &str, path: &str) {
        let same_file = self.path.as_deref() == Some(path);
        self.ranges = compute_ranges(source, path);
        self.hover = None;
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

    /// 範囲も開閉も捨てる（メディアファイルや読み込み失敗など、行が無いとき）。
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// line_1 がブロックの見出し行か。
    pub fn is_foldable(&self, line_1: usize) -> bool {
        self.range_starting_at(line_1).is_some()
    }

    /// line_1 のブロックが閉じているか。
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

    // ホバー中の範囲

    /// マーカーにマウスが乗っている見出し行を覚える。見出し行でなければ忘れる。
    pub fn set_hover(&mut self, line_1: Option<usize>) {
        self.hover = line_1.filter(|l| self.is_foldable(*l));
    }

    /// ホバー中の範囲に line_1 が含まれるなら、その中でどの位置か。
    pub fn hover_rule(&self, line_1: usize) -> Option<FoldRule> {
        let range = self.range_starting_at(self.hover?)?;
        if line_1 == range.start {
            Some(FoldRule::Head)
        } else if line_1 == range.end {
            Some(FoldRule::Tail)
        } else if line_1 > range.start && line_1 < range.end {
            Some(FoldRule::Body)
        } else {
            None
        }
    }

    // 開閉

    /// line_1 のブロックを開閉する。line_1 が見出し行でなければ、それを含む
    /// 最も内側のブロックを対象にする（vim の za と同じ）。
    pub fn toggle(&mut self, line_1: usize) -> bool {
        let Some(start) = self.target_start(line_1) else {
            return false;
        };
        if self.collapsed.contains(&start) {
            self.collapsed.remove(&start);
        } else {
            self.collapsed.insert(start);
        }
        self.collapsed_from_depth = None;
        self.rebuild_hidden();
        true
    }

    /// line_1 のブロック（なければそれを含む最も内側のブロック）を閉じる。
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

    /// line_1 のブロック（なければそれを含む最も内側のブロック）を開く。
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

    /// すべて開く（zR）。
    pub fn open_all(&mut self) {
        self.collapsed.clear();
        self.collapsed_from_depth = None;
        self.rebuild_hidden();
    }

    /// すべて閉じる（zM）。
    pub fn close_all(&mut self) {
        self.collapsed = self.ranges.iter().map(|r| r.start).collect();
        self.collapsed_from_depth = Some(1);
        self.rebuild_hidden();
    }

    /// まだ畳んでいない中で最も深い層を、全箇所まとめて畳む（zm）。
    ///
    /// 畳める範囲が1つも無ければ None。
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

    /// 深さ単位で畳んだ層のうち、最も浅いものを開き戻す（zr）。
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

    /// line_1 が画面に出るまで、それを隠しているブロックを外側から開く。
    ///
    /// 検索・定義ジャンプ・grep のヒットはどれも隠れた行に着地し得る。そこで
    /// 黙って別の行を見せるより、畳んだ本人に開いて見せる方が驚きが小さい。
    pub fn reveal(&mut self, line_1: usize) -> bool {
        if !self.is_hidden(line_1) {
            return false;
        }
        let opened: Vec<usize> = self
            .ranges
            .iter()
            .filter(|r| r.start < line_1 && line_1 <= r.end)
            .map(|r| r.start)
            .collect();
        let mut changed = false;
        for start in opened {
            changed |= self.collapsed.remove(&start);
        }
        if changed {
            self.collapsed_from_depth = None;
            self.rebuild_hidden();
        }
        changed
    }

    // 可視行のマッピング

    /// start_1 から total までの、画面に出る行番号（1始まり）。
    pub fn visible_from(&self, start_1: usize, total: usize) -> impl Iterator<Item = usize> + '_ {
        (start_1.max(1)..=total).filter(move |l| !self.is_hidden(*l))
    }

    /// 画面に出る行の総数。スクロールバーの尺として使う。
    pub fn visible_count(&self, total: usize) -> usize {
        (1..=total).filter(|l| !self.is_hidden(*l)).count()
    }

    /// line_1 が可視行の何番目か（0始まり）。スクロールバーのつまみの位置。
    pub fn visible_index(&self, line_1: usize, total: usize) -> usize {
        (1..=line_1.min(total))
            .filter(|l| !self.is_hidden(*l))
            .count()
            .saturating_sub(1)
    }

    /// line_1 から可視行を delta 行ぶん進める（負なら戻る）。端で止まる。
    ///
    /// j/k もマウスホイールも Ctrl+D もここを通るので、「1行進む」の意味が
    /// 折りたたみの有無で変わらない。
    pub fn step(&self, line_1: usize, delta: isize, total: usize) -> usize {
        if total == 0 {
            return 1;
        }
        let mut cur = line_1.clamp(1, total);
        let forward = delta > 0;
        for _ in 0..delta.unsigned_abs() {
            let next = if forward {
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

    /// line_1 より後ろにある最初の可視行。
    pub fn next_visible(&self, line_1: usize, total: usize) -> Option<usize> {
        (line_1 + 1..=total).find(|l| !self.is_hidden(*l))
    }

    /// line_1 より手前にある最後の可視行。
    pub fn prev_visible(&self, line_1: usize) -> Option<usize> {
        (1..line_1).rev().find(|l| !self.is_hidden(*l))
    }

    /// 最後の可視行（G の行き先）。
    pub fn last_visible(&self, total: usize) -> usize {
        (1..=total).rev().find(|l| !self.is_hidden(*l)).unwrap_or(1)
    }

    /// line_1 が隠れているなら、それを隠している見出し行へ寄せる。
    ///
    /// 閉じる操作でカーソル行が隠れたときに使う。隠れた行のすぐ上の可視行は
    /// 定義上その行を隠している見出し行なので、探し直す必要はない。
    pub fn visible_anchor(&self, line_1: usize) -> usize {
        if !self.is_hidden(line_1) {
            return line_1;
        }
        self.prev_visible(line_1).unwrap_or(1)
    }

    /// 畳める範囲の最も深い深さ。1つも無ければ None。
    pub fn max_depth(&self) -> Option<usize> {
        self.ranges.iter().map(|r| r.depth).max()
    }

    /// 深さ単位で畳んでいる最中なら、その段数。個別の開閉だけなら None。
    pub fn depth(&self) -> Option<FoldDepth> {
        let max = self.max_depth()?;
        self.collapsed_from_depth.map(|_| self.depth_level(max))
    }

    // 内部

    fn starts_at_depth(&self, depth: usize) -> Vec<usize> {
        self.ranges
            .iter()
            .filter(|r| r.depth == depth)
            .map(|r| r.start)
            .collect()
    }

    /// 内部の深さのしきい値を、ユーザに見せる「何段畳んだか」に直す。
    fn depth_level(&self, max: usize) -> FoldDepth {
        let level = self
            .collapsed_from_depth
            .map_or(0, |d| (max + 1).saturating_sub(d));
        FoldDepth { level, max }
    }

    fn range_starting_at(&self, line_1: usize) -> Option<&FoldRange> {
        self.ranges.iter().find(|r| r.start == line_1)
    }

    /// 操作の対象になる見出し行。見出し行そのものを優先し、無ければ line_1 を
    /// 含む最も内側のブロック。
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

// 範囲の算出

/// source の折りたたみ範囲。tree-sitter で1つも取れなければインデントで求める。
fn compute_ranges(source: &str, path: &str) -> Vec<FoldRange> {
    let ranges = syntax_ranges(source, path).unwrap_or_default();
    let mut ranges = if ranges.is_empty() {
        normalize(indent_ranges(source))
    } else {
        ranges
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

/// tree-sitter の構文木からブロックを拾う。文法が無い・パースできない場合は None。
fn syntax_ranges(source: &str, path: &str) -> Option<Vec<FoldRange>> {
    let ext = Path::new(path).extension()?.to_str()?;
    let language = crate::symbol_index::language_for_ext(ext)?;
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
            // 1行に収まっているブロックは畳んでも何も減らない。
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

/// 「複数行のノード全部」ではなく列挙なのは、式や引数リストまで畳めると1行あたりの
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
        if trimmed.is_empty() {
            None
        } else {
            Some(line.len() - trimmed.len())
        }
    }

    let mut out = Vec::new();
    // (インデント幅, 開始行の0始まりインデックス) を浅い順に積む。
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut prev_nonblank: Option<usize> = None;

    // limit より浅いところまで積みを崩し、崩した範囲を確定させる。
    // limit が None なら全部崩す（ファイル末尾）。
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

/// 見出し行がひとつなら折りたたみもひとつ。同じ行に2つあると開閉が collapsed の
/// 1エントリを取り合い、開いたのにまだ隠れている行が出る。
fn normalize(mut ranges: Vec<FoldRange>) -> Vec<FoldRange> {
    ranges.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    ranges.dedup_by(|a, b| a.start == b.start);
    ranges
}

// ViewerState 側の入口

use super::state::ViewerState;

impl ViewerState {
    /// カーソル行（＝ビューポート最上行、1始まり）。Viewer は独立したカーソルを
    /// 持たず file_scroll がその役目を兼ねている。
    pub fn cursor_line(&self) -> usize {
        self.content.file_scroll + 1
    }

    /// カーソルを可視行で delta 行ぶん動かす（負なら上へ）。
    pub fn move_cursor_lines(&mut self, delta: isize) {
        let total = self.content.file_content.len();
        if total == 0 {
            return;
        }
        let line = self.content.folds.step(self.cursor_line(), delta, total);
        self.content.file_scroll = line - 1;
    }

    /// カーソルを最後の可視行へ（G）。
    pub fn goto_last_visible_line(&mut self) {
        let total = self.content.file_content.len();
        if total == 0 {
            return;
        }
        self.content.file_scroll = self.content.folds.last_visible(total) - 1;
    }

    /// 画面に出る行の総数（スクロールバーの尺）。
    pub fn visible_line_count(&self) -> usize {
        self.content
            .folds
            .visible_count(self.content.file_content.len())
    }

    /// カーソル行が可視行の何番目か（スクロールバーのつまみ）。
    pub fn cursor_visible_index(&self) -> usize {
        self.content
            .folds
            .visible_index(self.cursor_line(), self.content.file_content.len())
    }

    /// カーソル行が隠れているなら、隠しているブロックを開く。
    ///
    /// 描画の直前に一度だけ呼ぶ。file_scroll を書く経路は検索・定義ジャンプ・
    /// grep・履歴復元と多く、そのすべてに「開いてから飛ぶ」を書いて回ると
    /// 必ずどれかが漏れる。可視性の判断を描画の一歩手前に集めておけば、
    /// 行き先が隠れていたときの扱いは1か所で決まる。
    ///
    /// j/k やホイールは可視行を歩くので、ここには決して到達しない — つまり
    /// これが開くのは常に「畳んだ場所の外から飛んできた」ときだけ。
    pub fn reveal_cursor_line(&mut self) {
        let line = self.cursor_line();
        self.content.folds.reveal(line);
    }

    /// カーソル行のブロックを開閉する（za）。
    pub fn fold_toggle_cursor(&mut self) -> bool {
        let line = self.cursor_line();
        let changed = self.content.folds.toggle(line);
        self.clamp_cursor_to_visible();
        changed
    }

    /// カーソル行のブロックを閉じる（zc）。
    pub fn fold_close_cursor(&mut self) -> bool {
        let line = self.cursor_line();
        let changed = self.content.folds.close(line);
        self.clamp_cursor_to_visible();
        changed
    }

    /// カーソル行のブロックを開く（zo）。
    pub fn fold_open_cursor(&mut self) -> bool {
        let line = self.cursor_line();
        self.content.folds.open(line)
    }

    /// すべて開く（zR）。
    pub fn fold_open_all(&mut self) {
        self.content.folds.open_all();
    }

    /// すべて閉じる（zM）。
    pub fn fold_close_all(&mut self) {
        self.content.folds.close_all();
        self.clamp_cursor_to_visible();
    }

    /// 折りたたみを操作できる状態か。レンダリング済み markdown と diff 表示は
    /// 行の畳みを持たない（diff は ExpandableContext という別の仕組み）。
    pub fn folds_available(&self) -> bool {
        !self.diff_view.diff_mode
            && !self.is_showing_rendered_markdown()
            && self.content.folds.max_depth().is_some()
    }

    /// タイトル行に出す、深さ単位の畳み具合。
    pub fn active_fold_depth(&self) -> Option<FoldDepth> {
        self.folds_available().then(|| self.content.folds.depth())?
    }

    /// 最も深い層をもう一段まとめて畳む（zm）。
    pub fn fold_collapse_deepest(&mut self) -> Option<FoldDepth> {
        let depth = self.content.folds.collapse_deepest();
        self.clamp_cursor_to_visible();
        depth
    }

    /// 深さ単位の畳み込みを一段開き戻す（zr）。
    pub fn fold_expand_shallowest(&mut self) -> Option<FoldDepth> {
        self.content.folds.expand_shallowest()
    }

    /// マウスで折りたたみマーカーが押されたときの入口。
    pub fn fold_toggle_at(&mut self, line_1: usize) -> bool {
        let changed = self.content.folds.toggle(line_1);
        self.clamp_cursor_to_visible();
        changed
    }

    /// 開くのではなく寄せる。閉じたのは操作した本人なので、直後に開き直すのでは
    /// 操作が無かったことになる。
    fn clamp_cursor_to_visible(&mut self) {
        let line = self.content.folds.visible_anchor(self.cursor_line());
        self.content.file_scroll = line.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(ranges: &[FoldRange]) -> Vec<(usize, usize)> {
        ranges.iter().map(|r| (r.start, r.end)).collect()
    }

    /// マーカーにマウスが乗ると、その範囲の見出し・途中・最終行が区別できる。
    /// 描画側はこの3つだけを見て罫線の字形を決める。
    #[test]
    fn hovering_a_marker_marks_its_whole_range() {
        let mut folds = FoldState::default();
        folds.rebuild(
            "fn outer() {\n    if cond {\n        inner();\n    }\n}\n",
            "a.rs",
        );
        folds.set_hover(Some(1));
        assert_eq!(folds.hover_rule(1), Some(FoldRule::Head));
        assert_eq!(folds.hover_rule(2), Some(FoldRule::Body));
        assert_eq!(folds.hover_rule(3), Some(FoldRule::Body));
        assert_eq!(folds.hover_rule(4), Some(FoldRule::Body));
        assert_eq!(folds.hover_rule(5), Some(FoldRule::Tail));
        assert_eq!(folds.hover_rule(6), None);
    }

    /// 見出し行でないところを指しても範囲は出ない。ホバーは「マーカーを
    /// 狙っている」ことの印なので、za のように内側のブロックへ寄せない。
    #[test]
    fn hovering_a_non_header_line_shows_nothing() {
        let mut folds = FoldState::default();
        folds.rebuild("fn outer() {\n    body();\n}\n", "a.rs");
        folds.set_hover(Some(2));
        assert_eq!(folds.hover_rule(1), None);
        assert_eq!(folds.hover_rule(2), None);
    }

    /// ファイルを読み直したらホバーは消える（マウスはもうそこに無い）。
    #[test]
    fn reload_forgets_the_hover() {
        let mut folds = FoldState::default();
        folds.rebuild("fn outer() {\n    body();\n}\n", "a.rs");
        folds.set_hover(Some(1));
        folds.rebuild("fn outer() {\n    body();\n}\n", "a.rs");
        assert_eq!(folds.hover_rule(1), None);
    }

    /// ネストしたブロックはそれぞれ独立した範囲になる。外側を畳んでも内側の
    /// 範囲は残り、外側を開けば内側の状態がそのまま見える。
    #[test]
    fn rust_nesting_yields_one_range_per_block() {
        let src = "\
fn outer() {
    if cond {
        inner();
    }
}
";
        let ranges = compute_ranges(src, "a.rs");
        assert_eq!(starts(&ranges), vec![(1, 5), (2, 4)]);
    }

    /// 1行に収まったブロックは畳んでも隠れる行が無いので、範囲にしない
    /// （マーカーだけ出て押しても何も起きない、を避ける）。
    #[test]
    fn single_line_blocks_are_not_foldable() {
        let src = "fn empty() {}\nfn other() { call(); }\n";
        let ranges = compute_ranges(src, "a.rs");
        assert!(ranges.is_empty(), "{ranges:?}");
    }

    /// ファイル末尾で閉じ括弧のあとに改行が無くても、範囲は最終行まで届く。
    #[test]
    fn a_block_reaching_the_end_of_file_is_complete() {
        let src = "fn last() {\n    body();\n}";
        let ranges = compute_ranges(src, "a.rs");
        assert_eq!(starts(&ranges), vec![(1, 3)]);
    }

    /// 構造体リテラルの本体も畳める。let 束縛の右辺に長いリテラルが来るのは
    /// よくある形で、ここが畳めないと本文の大半が畳めないファイルが出る。
    #[test]
    fn rust_struct_literal_bodies_fold() {
        let src = "fn f() {\n    let c = C {\n        a: 1,\n    };\n}\n";
        let ranges = compute_ranges(src, "a.rs");
        assert!(starts(&ranges).contains(&(2, 4)), "{:?}", starts(&ranges));
    }

    /// struct / impl / match も畳める（関数の block だけではない）。
    #[test]
    fn rust_type_and_match_bodies_fold() {
        let src = "\
struct S {
    a: u32,
}
impl S {
    fn f(&self) {
        match self.a {
            0 => {}
            _ => {}
        }
    }
}
";
        let ranges = compute_ranges(src, "a.rs");
        let s = starts(&ranges);
        assert!(s.contains(&(1, 3)), "struct body: {s:?}");
        assert!(s.contains(&(4, 11)), "impl body: {s:?}");
        assert!(s.contains(&(5, 10)), "fn body: {s:?}");
        assert!(s.contains(&(6, 9)), "match body: {s:?}");
    }

    /// 文法を持たない言語はインデントで範囲を出す。ここが空になると、
    /// 対応言語以外では機能そのものが消える。
    #[test]
    fn unsupported_language_falls_back_to_indentation() {
        let src = "\
def outer():
    if cond:
        inner()
    done()
after()
";
        let ranges = compute_ranges(src, "a.py");
        let s = starts(&ranges);
        assert!(s.contains(&(1, 4)), "def body: {s:?}");
        assert!(s.contains(&(2, 3)), "if body: {s:?}");
        // 一番浅い最終行は誰の子でもない。
        assert!(!s.iter().any(|(start, _)| *start == 5), "{s:?}");
    }

    /// パースできない中身でも（拡張子は対応言語でも）折りたたみは出る。
    /// tree-sitter がブロックを1つも取れなかったときにインデントへ落ちる。
    #[test]
    fn unparsable_source_still_folds_by_indentation() {
        let src = "\
outer:
    child one
    child two
tail
";
        let ranges = compute_ranges(src, "a.rs");
        assert_eq!(starts(&ranges), vec![(1, 3)]);
    }

    /// インデントのフォールバックでは、ブロックのあとの空行を範囲に含めない。
    /// 含めると畳んだ跡に空行だけが残って、詰まったように見えない。
    #[test]
    fn indentation_ranges_exclude_trailing_blank_lines() {
        let src = "outer:\n    child\n\n\nafter\n";
        let ranges = compute_ranges(src, "a.txt");
        assert_eq!(starts(&ranges), vec![(1, 2)]);
    }

    /// 空行はブロックを切らない（段落で分けて書かれた本文が細切れにならない）。
    #[test]
    fn indentation_ranges_span_interior_blank_lines() {
        let src = "outer:\n    a\n\n    b\nafter\n";
        let ranges = compute_ranges(src, "a.txt");
        assert_eq!(starts(&ranges), vec![(1, 4)]);
    }

    fn state(src: &str, path: &str) -> FoldState {
        let mut fs = FoldState::default();
        fs.rebuild(src, path);
        fs
    }

    const NEST: &str = "\
fn outer() {
    if cond {
        inner();
    }
}
";

    /// 畳んだ範囲は見出し行を残して隠れる。
    #[test]
    fn collapsing_hides_the_body_but_keeps_the_header() {
        let mut fs = state(NEST, "a.rs");
        assert!(fs.toggle(1));
        assert!(!fs.is_hidden(1));
        for line in 2..=5 {
            assert!(fs.is_hidden(line), "line {line}");
        }
        assert_eq!(fs.hidden_count(1), Some(4));
    }

    /// 外側を開いても、内側の畳みは畳まれたまま（vim と同じ）。
    #[test]
    fn opening_an_outer_fold_preserves_the_inner_one() {
        let mut fs = state(NEST, "a.rs");
        fs.close(2);
        fs.close(1);
        fs.open(1);
        assert!(!fs.is_hidden(2), "inner header is visible again");
        assert!(fs.is_hidden(3), "inner body stays folded");
        assert!(!fs.is_hidden(5));
    }

    /// 見出し行でない行を対象にすると、それを含む最も内側のブロックが動く。
    #[test]
    fn operating_inside_a_block_targets_the_innermost_one() {
        let mut fs = state(NEST, "a.rs");
        assert!(fs.close(3));
        assert!(fs.is_collapsed(2), "innermost block closed");
        assert!(!fs.is_collapsed(1));
    }

    /// 可視行の列挙・歩幅・番号はすべて同じ隠れ方を見る。
    #[test]
    fn visible_mapping_skips_folded_lines() {
        let mut fs = state(NEST, "a.rs");
        fs.close(2);

        assert_eq!(fs.visible_from(1, 5).collect::<Vec<_>>(), vec![1, 2, 5]);
        assert_eq!(fs.visible_count(5), 3);
        assert_eq!(fs.visible_index(5, 5), 2);
        // 2 の次の可視行は 5 — 畳んだ中を通らない。
        assert_eq!(fs.step(1, 1, 5), 2);
        assert_eq!(fs.step(1, 2, 5), 5);
        // 端では止まる（行き過ぎて末尾を超えない）。
        assert_eq!(fs.step(1, 99, 5), 5);
        assert_eq!(fs.step(5, -99, 5), 1);
    }

    /// 畳んだ中へ飛んできたときは、それを隠している範囲だけを開く。
    #[test]
    fn revealing_opens_only_the_folds_that_hide_the_line() {
        let mut fs = state(NEST, "a.rs");
        fs.close_all();
        assert!(fs.is_hidden(3));
        assert!(fs.reveal(3));
        assert!(!fs.is_hidden(3));
        assert!(!fs.is_collapsed(1) && !fs.is_collapsed(2));
        // すでに見えている行に対しては何もしない。
        assert!(!fs.reveal(1));
    }

    /// zM のあとカーソルが隠れたら、それを隠している見出し行へ寄せる。
    #[test]
    fn the_anchor_of_a_hidden_line_is_its_header() {
        let mut fs = state(NEST, "a.rs");
        fs.close_all();
        assert_eq!(fs.visible_anchor(4), 1);
        assert_eq!(fs.visible_anchor(1), 1);
    }

    /// 同じファイルの再読み込みでは開閉を保つ。読んでいた場所が、ウォッチャーが
    /// 走るたびに勝手に開き直さないため。
    #[test]
    fn reloading_the_same_file_keeps_the_folds() {
        let mut fs = state(NEST, "a.rs");
        fs.close(1);
        fs.rebuild(NEST, "a.rs");
        assert!(fs.is_collapsed(1));
    }

    /// 編集でブロックでなくなった行の畳みは落とす。残すと、開く手段の無い
    /// 隠れた行ができる。
    #[test]
    fn reloading_drops_folds_that_no_longer_exist() {
        let mut fs = state(NEST, "a.rs");
        fs.close(2);
        fs.rebuild("fn outer() {\n    done();\n}\n", "a.rs");
        assert!(!fs.is_collapsed(2));
        assert!(!fs.is_hidden(2));
    }

    /// 別のファイルへ移ったら開閉は捨てる。行番号は移った先で別の意味を持つ。
    #[test]
    fn switching_files_discards_the_folds() {
        let mut fs = state(NEST, "a.rs");
        fs.close(1);
        fs.rebuild(NEST, "b.rs");
        assert!(!fs.is_collapsed(1));
    }

    /// 折りたたみが無いファイルでは、すべての行がそのまま可視行になる。
    #[test]
    fn a_file_without_folds_is_entirely_visible() {
        let fs = FoldState::default();
        assert_eq!(fs.visible_from(1, 3).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(fs.visible_count(3), 3);
        assert_eq!(fs.step(1, 1, 3), 2);
        assert_eq!(fs.last_visible(3), 3);
        assert!(!fs.is_foldable(1));
    }

    /// 深さ単位の畳み込みは行ではなく段を対象にするので、同じ深さのブロックが
    /// 何か所にあっても1回で全部畳まれる。
    #[test]
    fn collapsing_a_depth_folds_every_block_at_it() {
        let src = "\
fn a() {
    if x {
        p();
    }
}
fn b() {
    if y {
        q();
    }
}
";
        let mut fs = state(src, "a.rs");
        assert_eq!(fs.collapse_deepest(), Some(FoldDepth { level: 1, max: 2 }));
        assert!(fs.is_collapsed(2));
        assert!(fs.is_collapsed(7));
        assert!(!fs.is_collapsed(1) && !fs.is_collapsed(6));
    }

    /// 深さは両端で止まる。最も浅い段まで畳んだら zm はそれ以上進まず、
    /// 全部開き切ったら zr は何も動かさない。
    #[test]
    fn the_depth_stops_at_both_ends() {
        let mut fs = state(NEST, "a.rs");
        assert_eq!(fs.collapse_deepest().map(|d| d.level), Some(1));
        assert_eq!(fs.collapse_deepest().map(|d| d.level), Some(2));
        assert_eq!(fs.collapse_deepest().map(|d| d.level), Some(2));
        assert!(fs.is_collapsed(1) && fs.is_collapsed(2));

        assert_eq!(fs.expand_shallowest().map(|d| d.level), Some(1));
        assert_eq!(fs.expand_shallowest().map(|d| d.level), Some(0));
        assert_eq!(fs.expand_shallowest().map(|d| d.level), Some(0));
        assert!(!fs.is_collapsed(1) && !fs.is_collapsed(2));
    }

    /// 畳める範囲が1つも無いファイルでは深さ操作そのものが成立しない。
    #[test]
    fn a_file_without_folds_has_no_depth() {
        let mut fs = FoldState::default();
        assert_eq!(fs.collapse_deepest(), None);
        assert_eq!(fs.expand_shallowest(), None);
    }

    /// 個別に開閉したら深さの位置は失われ、次の zm は最も深い段からやり直す。
    #[test]
    fn folding_one_block_by_hand_forgets_the_depth() {
        let mut fs = state(NEST, "a.rs");
        fs.collapse_deepest();
        fs.collapse_deepest();
        fs.toggle(1);
        assert_eq!(fs.collapse_deepest().map(|d| d.level), Some(1));
    }

    /// タイトル行の段数表示は深さ単位で畳んでいる間だけ出す。個別に畳んだだけの
    /// ファイルに段数を出すと、そこから zr で開けるように見えてしまう。
    #[test]
    fn the_depth_shows_only_while_folding_by_depth() {
        let mut fs = state(NEST, "a.rs");
        assert_eq!(fs.depth(), None);
        fs.close(1);
        assert_eq!(fs.depth(), None);
        fs.collapse_deepest();
        assert_eq!(fs.depth(), Some(FoldDepth { level: 1, max: 2 }));
        fs.toggle(2);
        assert_eq!(fs.depth(), None);
    }

    /// zM は最も浅い段まで畳んだのと同じなので、そこから zr で1段ずつ開ける。
    #[test]
    fn closing_all_leaves_the_depth_at_its_deepest_level() {
        let mut fs = state(NEST, "a.rs");
        fs.close_all();
        assert_eq!(fs.expand_shallowest().map(|d| d.level), Some(1));
        assert!(!fs.is_collapsed(1));
        assert!(fs.is_collapsed(2));
    }
}
