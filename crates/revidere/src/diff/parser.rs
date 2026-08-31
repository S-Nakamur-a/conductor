// unified diff（`git diff` の出力）のパース。
//
// ここの責務はテキストを Diff の型へ写すことだけ。行から変更箇所を取り出す
// のは ledger.rs の仕事で、ここでは行番号の割り当てと種別の判定までしかしない。

use super::{Diff, DiffLine, FileDiff, FileKind, Hunk, Tag};

/// unified diff（`git diff` の出力）をパースする。
pub fn parse(text: &str) -> Diff {
    let mut diff = Diff::default();
    let mut cur: Option<FileDiff> = None;
    // ハンク内で次に割り当てる行番号。
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;
    // ---/+++ から拾った暫定のパス。
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
        // ハンクの中でなければ index 行などなので読み飛ばす。
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
            // 空行。git は空の文脈行を " " で出すが、途中で末尾空白を落とす経路が
            // あるので、空行は文脈行として扱う。ここで捨てると以降の行番号が全部ずれる。
            None => (Tag::Context, ""),
            // ハンク内に現れる想定外の行。行番号を進めずに読み飛ばす。
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
            // 削除ファイルは後像が無いので、前像のパスで呼べるようにする。
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
/// （バイナリ・モードのみ・純粋な rename）のための後詰め。
fn split_git_header(rest: &str) -> (Option<String>, Option<String>) {
    // パスが同一なら真ん中で割れる。rename ではそうならないので " b/" を探す。
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

    #[test]
    fn 行番号は前像と後像を別々に追う() {
        let d = parse(BASIC);
        let f = &d.files[0];
        assert_eq!(f.path, "src/a.rs");
        let l = &f.hunks[0].lines;
        // 文脈行は両側を進める。
        assert_eq!((l[0].old_line, l[0].new_line), (Some(10), Some(10)));
        // 削除行は前像だけ。
        assert_eq!((l[1].old_line, l[1].new_line), (Some(11), None));
        // 追加行は後像だけ。前像の 11 は消費済みなので追加は後像 11,12。
        assert_eq!((l[2].old_line, l[2].new_line), (None, Some(11)));
        assert_eq!((l[3].old_line, l[3].new_line), (None, Some(12)));
        // 続く文脈行は前像 12 / 後像 13。
        assert_eq!((l[4].old_line, l[4].new_line), (Some(12), Some(13)));
    }

    #[test]
    fn ハンクの見出しは関数の文脈を残す() {
        let d = parse(BASIC);
        assert_eq!(d.files[0].hunks[0].header, "fn outer()");
    }

    #[test]
    fn 件数の無いハンクの見出しも受け入れる() {
        let d = parse(
            "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -5 +5 @@
-x
+y
",
        );
        let l = &d.files[0].hunks[0].lines;
        assert_eq!(l[0].old_line, Some(5));
        assert_eq!(l[1].new_line, Some(5));
    }

    #[test]
    fn 追加されたファイルは追加種別で新しいパスの名前になる() {
        let d = parse(
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
        );
        assert_eq!(d.files[0].kind, FileKind::Added);
        assert_eq!(d.files[0].path, "src/new.rs");
    }

    #[test]
    fn 削除されたファイルは元のパスを名前に残す() {
        // 削除ファイルは後像が無いので、前像のパスで呼べるようにする。
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
        let f = &d.files[0];
        assert_eq!(f.kind, FileKind::Deleted);
        assert_eq!(f.path, "src/gone.rs");
    }

    #[test]
    fn ハンクの無い純粋なリネームはリネーム種別になる() {
        let d = parse(
            "\
diff --git a/src/old.rs b/src/new.rs
similarity index 100%
rename from src/old.rs
rename to src/new.rs
",
        );
        let f = &d.files[0];
        assert_eq!(f.kind, FileKind::Renamed);
        assert_eq!(f.path, "src/new.rs");
        assert_eq!(f.old_path.as_deref(), Some("src/old.rs"));
    }

    #[test]
    fn ハンクの無いモードだけの変更はモード種別になる() {
        let d = parse(
            "\
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
",
        );
        assert_eq!(d.files[0].kind, FileKind::ModeOnly);
    }

    #[test]
    fn リネームの見出しの分割は最後のbスラッシュに落ちる() {
        // old_path 自身が " b/" を含むと、真ん中で機械的に割ると別のパスに
        // なる。候補が複数あるときは最後（本当の区切り）を採る。
        let d = parse(
            "\
diff --git a/weird b/file.rs b/weird2.rs
similarity index 90%
rename from weird b/file.rs
rename to weird2.rs
",
        );
        let f = &d.files[0];
        assert_eq!(f.kind, FileKind::Renamed);
        assert_eq!(f.path, "weird2.rs");
        assert_eq!(f.old_path.as_deref(), Some("weird b/file.rs"));
    }

    #[test]
    fn 改行なしの印は行番号を進めない() {
        let d = parse(
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
        );
        let l = &d.files[0].hunks[0].lines;
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].old_line, Some(1));
        assert_eq!(l[1].new_line, Some(1));
    }

    #[test]
    fn ハンクの中の空行は文脈として数える() {
        // 空の文脈行を捨てると、それ以降の行番号が全部ずれる。
        let d = parse(
            "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
 first

-third
+THIRD
",
        );
        let l = &d.files[0].hunks[0].lines;
        assert_eq!(l[1].tag, Tag::Context);
        assert_eq!(l[2].old_line, Some(3));
        assert_eq!(l[3].new_line, Some(3));
    }

    #[test]
    fn ハンクの中の想定外の行は進めずに飛ばす() {
        let d = parse(
            "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,1 @@
%odd
-old
+new
",
        );
        let l = &d.files[0].hunks[0].lines;
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].old_line, Some(1));
        assert_eq!(l[1].new_line, Some(1));
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
        assert_eq!(d.files[1].path, "src/b.rs");
    }
}
