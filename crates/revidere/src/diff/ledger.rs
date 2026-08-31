// 変更一覧。パースした Diff から変更箇所の集合を取り出し、プロンプトに載せる
// 要約（ledger_summary）を作る。テキストは読まず、parser.rs が作った型だけを見る。

use super::{DiffLine, FileDiff, FileKind, Tag};
use crate::review::{Position, Side};
use std::collections::BTreeSet;

impl DiffLine {
    /// この行が指す変更箇所。文脈行は位置を持たない。
    pub fn position(&self, path: &str) -> Option<Position> {
        match self.tag {
            Tag::Context => None,
            Tag::Add => self.new_line.map(|n| Position::new(path, Side::New, n)),
            Tag::Del => self.old_line.map(|n| Position::new(path, Side::Old, n)),
        }
    }
}

impl FileDiff {
    /// このファイルが持つ変更箇所。行を持たない変更はファイル単位の位置 1 つになる。
    ///
    /// ここで空を返すと「変更が無かった」と区別が付かなくなるので、
    /// 行が無くても必ず 1 つは出す。
    pub fn positions(&self) -> Vec<Position> {
        let from_lines: Vec<Position> = self
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter_map(|l| l.position(&self.path))
            .collect();
        if from_lines.is_empty() {
            vec![Position::file(self.path.clone())]
        } else {
            from_lines
        }
    }
}

impl super::Diff {
    /// 全ての変更箇所。並びは安定（パス、side、行番号の順）。
    pub fn positions(&self) -> BTreeSet<Position> {
        self.files.iter().flat_map(|f| f.positions()).collect()
    }

    /// プロンプトに載せる変更一覧。1 行ずつ並べると長くなりすぎるので、
    /// 連続した行番号を範囲へ畳んで出す。
    pub fn ledger_summary(&self) -> String {
        let mut out = String::new();
        for f in &self.files {
            let kind = match f.kind {
                FileKind::Modified => "modified",
                FileKind::Added => "added",
                FileKind::Deleted => "deleted",
                FileKind::Renamed => "renamed",
                FileKind::Binary => "binary",
                FileKind::ModeOnly => "mode-only",
            };
            out.push_str(&format!("{} [{}]\n", f.path, kind));
            if let Some(old) = &f.old_path {
                out.push_str(&format!("  (renamed from {old})\n"));
            }
            let new_lines = collect(f, Tag::Add, |l| l.new_line);
            let old_lines = collect(f, Tag::Del, |l| l.old_line);
            if new_lines.is_empty() && old_lines.is_empty() {
                out.push_str("  file (no line-level change)\n");
            }
            if !new_lines.is_empty() {
                out.push_str(&format!("  new: {}\n", fold_ranges(&new_lines)));
            }
            if !old_lines.is_empty() {
                out.push_str(&format!("  old: {}\n", fold_ranges(&old_lines)));
            }
        }
        out
    }
}

fn collect(f: &FileDiff, tag: Tag, pick: fn(&DiffLine) -> Option<u32>) -> Vec<u32> {
    let mut v: Vec<u32> = f
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.tag == tag)
        .filter_map(pick)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// 昇順の行番号列を "1-3, 7, 10-12" の形に畳む。
fn fold_ranges(sorted: &[u32]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::super::parser::parse;
    use super::*;

    #[test]
    fn 位置には追加と削除が入り文脈は入らない() {
        let d = parse(
            "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -10,4 +10,5 @@ fn outer()
 keep one
-gone
+added one
+added two
 keep two
",
        );
        let ps = d.positions();
        assert!(ps.contains(&Position::new("src/a.rs", Side::Old, 11)));
        assert!(ps.contains(&Position::new("src/a.rs", Side::New, 11)));
        assert!(ps.contains(&Position::new("src/a.rs", Side::New, 12)));
        // 文脈行は位置を持たない。
        assert!(!ps.contains(&Position::new("src/a.rs", Side::New, 10)));
        assert_eq!(ps.len(), 3);
    }

    #[test]
    fn 削除されたファイルは前像側の位置だけを出す() {
        // ファイル全体の削除でも、後像の行番号を作らない。
        let d = parse(
            "\
diff --git a/src/gone.rs b/src/gone.rs
deleted file mode 100644
index 1111111..0000000
--- a/src/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-one
-two
",
        );
        let ps = d.files[0].positions();
        assert_eq!(
            ps,
            vec![
                Position::new("src/gone.rs", Side::Old, 1),
                Position::new("src/gone.rs", Side::Old, 2),
            ]
        );
        assert!(ps.iter().all(|p| p.side == Side::Old));
    }

    #[test]
    fn バイナリはファイル単位の位置をちょうど1つ出す() {
        // 行が無くても黙って消さない。消すと「変更が無かった」と区別が付かない。
        let d = parse(
            "\
diff --git a/logo.png b/logo.png
index 1111111..2222222 100644
Binary files a/logo.png and b/logo.png differ
",
        );
        assert_eq!(d.files[0].positions(), vec![Position::file("logo.png")]);
    }

    #[test]
    fn ハンクの無い純粋なリネームはファイル単位の位置になる() {
        let d = parse(
            "\
diff --git a/src/old.rs b/src/new.rs
similarity index 100%
rename from src/old.rs
rename to src/new.rs
",
        );
        let f = &d.files[0];
        assert_eq!(f.positions(), vec![Position::file(f.path.clone())]);
    }

    #[test]
    fn モードだけの変更もファイル単位の位置になる() {
        let d = parse(
            "\
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
",
        );
        assert_eq!(d.files[0].positions(), vec![Position::file("run.sh")]);
    }

    #[test]
    fn 位置はパスと側と行の順に並ぶ() {
        let d = parse(
            "\
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,1 @@
-x
+y
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,1 +1,1 @@
-x
+y
",
        );
        let got: Vec<Position> = d.positions().into_iter().collect();
        assert_eq!(
            got,
            vec![
                Position::new("a.rs", Side::New, 1),
                Position::new("a.rs", Side::Old, 1),
                Position::new("b.rs", Side::New, 1),
                Position::new("b.rs", Side::Old, 1),
            ]
        );
    }

    #[test]
    fn 台帳は連続した行を範囲に畳む() {
        let d = parse(
            "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,4 @@
 keep
+p
+q
+r
",
        );
        let s = d.ledger_summary();
        assert!(s.contains("new: 2-4"), "{s}");
    }

    #[test]
    fn 範囲の畳みは間が空くと分かれる() {
        assert_eq!(fold_ranges(&[1, 2, 3, 7, 10, 11]), "1-3, 7, 10-11");
        assert_eq!(fold_ranges(&[]), "");
        assert_eq!(fold_ranges(&[4]), "4");
    }
}
