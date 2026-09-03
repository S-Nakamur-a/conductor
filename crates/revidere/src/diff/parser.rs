// unified diff（`git diff` の出力）のパース。テキストを Diff の型へ写すだけで、
// 変更箇所を取り出すのは ledger.rs の仕事。

use super::{Diff, DiffLine, FileDiff, FileKind, Hunk, Tag};

/// unified diff（`git diff` の出力）をパースする。
pub fn parse(text: &str) -> Diff {
    let mut diff = Diff::default();
    let mut cur: Option<FileDiff> = None;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;
    let mut hdr_old: Option<String> = None;
    let mut hdr_new: Option<String> = None;
    let mut saw_rename = false;
    let mut saw_mode = false;

    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            flush(
                &mut diff,
                &mut cur,
                &mut hdr_old,
                &mut hdr_new,
                &mut saw_rename,
                &mut saw_mode,
            );
            let (a, b) = split_git_header(rest);
            cur = Some(FileDiff {
                path: b.clone().unwrap_or_else(|| a.clone().unwrap_or_default()),
                old_path: None,
                kind: FileKind::Modified,
                hunks: Vec::new(),
            });
            hdr_old = a;
            hdr_new = b;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };

        if let Some(rest) = raw.strip_prefix("--- ") {
            hdr_old = strip_side(rest);
            continue;
        }
        if let Some(rest) = raw.strip_prefix("+++ ") {
            hdr_new = strip_side(rest);
            continue;
        }
        if raw.starts_with("rename from ") || raw.starts_with("rename to ") {
            saw_rename = true;
            continue;
        }
        if raw.starts_with("old mode ") || raw.starts_with("new mode ") {
            saw_mode = true;
            continue;
        }
        if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
            f.kind = FileKind::Binary;
            continue;
        }
        if let Some(h) = parse_hunk_header(raw) {
            old_no = h.old_start;
            new_no = h.new_start;
            f.hunks.push(Hunk {
                header: h.header,
                lines: Vec::new(),
            });
            continue;
        }
        // ハンクの外は index 行などなので読み飛ばす。
        let Some(hunk) = f.hunks.last_mut() else {
            continue;
        };
        // "\ No newline at end of file" は行番号を進めない。
        if raw.starts_with('\\') {
            continue;
        }
        let (tag, text) = match raw.as_bytes().first() {
            Some(b'+') => (Tag::Add, &raw[1..]),
            Some(b'-') => (Tag::Del, &raw[1..]),
            Some(b' ') => (Tag::Context, &raw[1..]),
            // git は空の文脈行を " " で出すが、途中で末尾空白を落とす経路がある。
            // 空行を捨てると以降の行番号が全部ずれる。
            None => (Tag::Context, ""),
            _ => continue,
        };
        let line = match tag {
            Tag::Add => {
                let l = DiffLine {
                    tag,
                    old_line: None,
                    new_line: Some(new_no),
                    text: text.to_string(),
                };
                new_no += 1;
                l
            }
            Tag::Del => {
                let l = DiffLine {
                    tag,
                    old_line: Some(old_no),
                    new_line: None,
                    text: text.to_string(),
                };
                old_no += 1;
                l
            }
            Tag::Context => {
                let l = DiffLine {
                    tag,
                    old_line: Some(old_no),
                    new_line: Some(new_no),
                    text: text.to_string(),
                };
                old_no += 1;
                new_no += 1;
                l
            }
        };
        hunk.lines.push(line);
    }
    flush(
        &mut diff,
        &mut cur,
        &mut hdr_old,
        &mut hdr_new,
        &mut saw_rename,
        &mut saw_mode,
    );
    diff
}

/// 直前のファイルを確定して diff に積む。
fn flush(
    diff: &mut Diff,
    cur: &mut Option<FileDiff>,
    hdr_old: &mut Option<String>,
    hdr_new: &mut Option<String>,
    saw_rename: &mut bool,
    saw_mode: &mut bool,
) {
    let Some(mut f) = cur.take() else {
        *hdr_old = None;
        *hdr_new = None;
        *saw_rename = false;
        *saw_mode = false;
        return;
    };
    let old = hdr_old.take();
    let new = hdr_new.take();
    match (&old, &new) {
        // /dev/null 側は strip_side が None を返す。
        (None, Some(n)) => {
            f.kind = FileKind::Added;
            f.path = n.clone();
        }
        (Some(o), None) => {
            f.kind = FileKind::Deleted;
            f.path = o.clone();
        }
        (Some(o), Some(n)) => {
            f.path = n.clone();
            if o != n {
                f.kind = FileKind::Renamed;
                f.old_path = Some(o.clone());
            }
        }
        (None, None) => {}
    }
    if *saw_rename && f.old_path.is_none() && f.kind == FileKind::Modified {
        f.kind = FileKind::Renamed;
    }
    if f.kind == FileKind::Modified && f.hunks.is_empty() && *saw_mode {
        f.kind = FileKind::ModeOnly;
    }
    *saw_rename = false;
    *saw_mode = false;
    diff.files.push(f);
}

/// "a/src/foo.rs" -> Some("src/foo.rs")、"/dev/null" -> None。
fn strip_side(s: &str) -> Option<String> {
    // タブ以降はタイムスタンプなどの付加情報。
    let s = s.split('\t').next().unwrap_or(s).trim_end();
    if s == "/dev/null" {
        return None;
    }
    let s = s
        .strip_prefix("a/")
        .or_else(|| s.strip_prefix("b/"))
        .unwrap_or(s);
    // git はパスに空白などが含まれるとダブルクォートで囲む。
    let s = s.trim_matches('"');
    Some(s.to_string())
}

/// `diff --git a/X b/Y` の残りから両側のパスを取る。
///
/// ---/+++ が続けばそちらで上書きされるので、ここは行を持たない変更
/// （バイナリ・モードのみ・純粋な rename）のための後詰め。パスが同一なら
/// 真ん中で割れるが、rename ではそうならないので " b/" を探す。
fn split_git_header(rest: &str) -> (Option<String>, Option<String>) {
    let candidates: Vec<usize> = rest.match_indices(" b/").map(|(i, _)| i).collect();
    for &i in &candidates {
        let (l, r) = rest.split_at(i);
        let l = l.trim_matches('"');
        let r = r[1..].trim_matches('"');
        if l.strip_prefix("a/") == r.strip_prefix("b/") {
            return (strip_side(l), strip_side(r));
        }
    }
    match candidates.last() {
        Some(&i) => {
            let (l, r) = rest.split_at(i);
            (
                strip_side(l.trim_matches('"')),
                strip_side(r[1..].trim_matches('"')),
            )
        }
        None => (None, None),
    }
}

struct HunkHeader {
    old_start: u32,
    new_start: u32,
    header: String,
}

/// `@@ -12,7 +12,9 @@ fn foo()` をパースする。
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let rest = line.strip_prefix("@@ ")?;
    let close = rest.find(" @@")?;
    let (spec, tail) = rest.split_at(close);
    let mut parts = spec.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some(HunkHeader {
        old_start: start_of(old)?,
        new_start: start_of(new)?,
        header: tail.trim_start_matches(" @@").trim().to_string(),
    })
}

/// "12,7" や "12" から開始行を取る。
fn start_of(spec: &str) -> Option<u32> {
    spec.split(',').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -10,4 +10,5 @@ fn outer()
 keep one
-gone
+added one
+added two
 keep two
";

    fn numbers(text: &str) -> Vec<(Tag, Option<u32>, Option<u32>)> {
        parse(text).files[0].hunks[0]
            .lines
            .iter()
            .map(|l| (l.tag, l.old_line, l.new_line))
            .collect()
    }

    #[test]
    fn 行番号は前像と後像を別々に追う() {
        assert_eq!(
            numbers(BASIC),
            vec![
                (Tag::Context, Some(10), Some(10)),
                (Tag::Del, Some(11), None),
                // 前像の 11 は削除で消費済みなので、追加は後像 11, 12。
                (Tag::Add, None, Some(11)),
                (Tag::Add, None, Some(12)),
                (Tag::Context, Some(12), Some(13)),
            ]
        );
    }

    #[test]
    fn ハンクの見出しは関数の文脈を残す() {
        assert_eq!(parse(BASIC).files[0].hunks[0].header, "fn outer()");
        // 件数の無い見出しも受け入れる。
        assert_eq!(
            numbers(
                "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -5 +5 @@
-x
+y
"
            ),
            vec![(Tag::Del, Some(5), None), (Tag::Add, None, Some(5))]
        );
    }

    #[test]
    fn ハンクの中の特殊な行は行番号を狂わせない() {
        for (name, text, want) in [
            (
                "改行なしの印は行番号を進めない",
                "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
",
                vec![(Tag::Del, Some(1), None), (Tag::Add, None, Some(1))],
            ),
            (
                "想定外の行は進めずに飛ばす",
                "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,1 @@
%odd
-old
+new
",
                vec![(Tag::Del, Some(1), None), (Tag::Add, None, Some(1))],
            ),
            (
                "空行は文脈として数える",
                "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
 first

-third
+THIRD
",
                vec![
                    (Tag::Context, Some(1), Some(1)),
                    (Tag::Context, Some(2), Some(2)),
                    (Tag::Del, Some(3), None),
                    (Tag::Add, None, Some(3)),
                ],
            ),
        ] {
            assert_eq!(numbers(text), want, "{name}");
        }
    }

    #[test]
    fn ファイルの種別と名前はヘッダから決まる() {
        for (name, text, kind, path, old_path) in [
            (
                "追加は新しいパスの名前になる",
                "\
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+one
+two
",
                FileKind::Added,
                "src/new.rs",
                None,
            ),
            (
                // 後像が無いので、前像のパスで呼べるようにする。
                "削除は元のパスを名前に残す",
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
                FileKind::Deleted,
                "src/gone.rs",
                None,
            ),
            (
                "ハンクの無い純粋なリネーム",
                "\
diff --git a/src/old.rs b/src/new.rs
similarity index 100%
rename from src/old.rs
rename to src/new.rs
",
                FileKind::Renamed,
                "src/new.rs",
                Some("src/old.rs"),
            ),
            (
                // old_path 自身が " b/" を含むと、真ん中で機械的に割ると別のパスに
                // なる。候補が複数あるときは最後 (本当の区切り) を採る。
                "リネームの見出しは最後の b スラッシュで割る",
                "\
diff --git a/weird b/file.rs b/weird2.rs
similarity index 90%
rename from weird b/file.rs
rename to weird2.rs
",
                FileKind::Renamed,
                "weird2.rs",
                Some("weird b/file.rs"),
            ),
            (
                "ハンクの無いモードだけの変更",
                "\
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
",
                FileKind::ModeOnly,
                "run.sh",
                None,
            ),
        ] {
            let f = &parse(text).files[0];
            assert_eq!(f.kind, kind, "{name}: 種別");
            assert_eq!(f.path, path, "{name}: パス");
            assert_eq!(f.old_path.as_deref(), old_path, "{name}: 前像のパス");
        }
    }

    #[test]
    fn 複数のファイルは分けて扱う() {
        let d = parse(&format!(
            "{BASIC}{}",
            "\
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1,1 +1,1 @@
-x
+y
"
        ));
        assert_eq!(d.files.len(), 2);
        assert_eq!(d.files[0].path, "src/a.rs");
        assert_eq!(d.files[1].path, "src/b.rs");
    }
}
