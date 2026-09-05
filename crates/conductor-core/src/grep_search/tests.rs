use super::*;
use crate::test_support::TestRepo;

fn re(pattern: &str, regex_mode: bool, case_sensitive: bool) -> Regex {
    compile_pattern(pattern, regex_mode, case_sensitive).unwrap()
}

#[test]
fn gitignoreされたファイルは検索対象から外れる() {
    let repo = TestRepo::with_base_commit();
    repo.file(".gitignore", "ignored.txt\n");
    repo.file("ignored.txt", "needle here\n");
    repo.file("visible.txt", "needle here\n");

    let matches = search_tree(&repo.path, &re("needle", false, true));
    let files: Vec<&str> = matches.iter().map(|m| m.file_path.as_str()).collect();
    assert_eq!(files, vec!["visible.txt"]);
}

#[test]
fn リテラルモードでは正規表現の特殊文字がそのまま扱われる() {
    let repo = TestRepo::with_base_commit();
    repo.file("data.txt", "a.b\nc+d\n");

    // '.' がリテラルなら "a.b" にしか当たらない。
    let literal = search_tree(&repo.path, &re("a.b", false, true));
    assert_eq!(literal.len(), 1);
    assert_eq!(literal[0].line_content, "a.b");

    // regex_mode なら '.' は任意の 1 文字にマッチし、"c+d" の行にも当たる。
    let as_regex = search_tree(&repo.path, &re("a.b|c.d", true, true));
    assert_eq!(as_regex.len(), 2);
}

#[test]
fn 大文字小文字の区別はオプションで切り替わる() {
    let repo = TestRepo::with_base_commit();
    repo.file("data.txt", "Needle\n");

    assert!(search_tree(&repo.path, &re("needle", false, true)).is_empty());
    assert_eq!(
        search_tree(&repo.path, &re("needle", false, false)).len(),
        1
    );
}

#[test]
fn search_fileは指定した1ファイルだけを検索しオフセットを刻む() {
    let repo = TestRepo::with_base_commit();
    repo.file("a.txt", "prefix needle\n");
    repo.file("b.txt", "needle\n");

    let matches = search_file(&repo.path, "a.txt", &re("needle", false, true));
    assert_eq!(
        matches,
        vec![GrepMatch {
            file_path: "a.txt".to_string(),
            line_number: 1,
            line_content: "prefix needle".to_string(),
            match_start: 7,
            match_end: 13,
        }]
    );
}
