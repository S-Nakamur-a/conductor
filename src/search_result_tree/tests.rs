//! [SearchResultTree] の構築、行のフラット化、展開/折りたたみ/移動の
//! 挙動に関するテスト。

use super::*;
use crate::grep_search::GrepMatch;

fn make_match(file_path: &str, line_number: usize, content: &str) -> GrepMatch {
    GrepMatch {
        file_path: file_path.to_string(),
        line_number,
        line_content: content.to_string(),
        match_start: 0,
        match_end: 1,
    }
}

#[test]
fn test_build_tree_structure() {
    let matches = vec![
        make_match("src/app.rs", 42, "fn search_text()"),
        make_match("src/app.rs", 108, "let result = search()"),
        make_match("src/app.rs", 205, "// TODO: search optimization"),
        make_match("src/ui/viewer.rs", 55, "highlight_search_result()"),
        make_match("lib/utils.rs", 12, "pub fn fuzzy_search()"),
        make_match("lib/utils.rs", 89, "search_index.update()"),
        make_match("README.md", 5, "search documentation"),
    ];

    let mut tree = SearchResultTree::build(&matches);
    assert_eq!(tree.match_count(), 7);

    let rows = tree.visible_rows();
    // 想定内訳: root-file(README.md) + dir(lib) + file(utils.rs) + 2マッチ +
    //          dir(src) + file(app.rs) + 3マッチ + dir(ui) + file(viewer.rs) + 1マッチ
    assert!(!rows.is_empty());

    // ディレクトリが現れることを確認する。
    let dir_names: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            SearchTreeRow::Dir { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert!(dir_names.contains(&"lib".to_string()));
    assert!(dir_names.contains(&"src".to_string()));
    assert!(dir_names.contains(&"ui".to_string()));

    // ファイルが現れることを確認する。
    let file_names: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            SearchTreeRow::File { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert!(file_names.contains(&"app.rs".to_string()));
    assert!(file_names.contains(&"viewer.rs".to_string()));
    assert!(file_names.contains(&"utils.rs".to_string()));
    assert!(file_names.contains(&"README.md".to_string()));
}

#[test]
fn test_collapse_dir_hides_children() {
    let matches = vec![
        make_match("src/app.rs", 42, "fn search()"),
        make_match("src/app.rs", 108, "search()"),
        make_match("lib/utils.rs", 12, "search()"),
    ];

    let mut tree = SearchResultTree::build(&matches);
    let initial_count = tree.visible_rows().len();

    // "src" ディレクトリ行を見つけて折りたたむ。
    let src_idx = tree
        .visible_rows()
        .iter()
        .position(|r| matches!(r, SearchTreeRow::Dir { name, .. } if name == "src"))
        .unwrap();
    tree.collapse(src_idx);

    let collapsed_count = tree.visible_rows().len();
    // 折りたたむと src/ 配下のファイル+マッチ行が隠れるはず。
    assert!(collapsed_count < initial_count);

    // 再展開する。
    tree.expand(src_idx);
    assert_eq!(tree.visible_rows().len(), initial_count);
}

#[test]
fn test_collapse_file_hides_matches() {
    let matches = vec![
        make_match("src/app.rs", 42, "fn search()"),
        make_match("src/app.rs", 108, "search()"),
    ];

    let mut tree = SearchResultTree::build(&matches);
    let initial_count = tree.visible_rows().len();

    // "app.rs" ファイル行を見つける。
    let file_idx = tree
        .visible_rows()
        .iter()
        .position(|r| matches!(r, SearchTreeRow::File { name, .. } if name == "app.rs"))
        .unwrap();
    tree.collapse(file_idx);

    let collapsed_count = tree.visible_rows().len();
    // 折りたたむと2件のマッチ行が隠れるはず。
    assert_eq!(collapsed_count, initial_count - 2);
}

#[test]
fn test_next_sibling_skips_collapsed() {
    let matches = vec![
        make_match("src/app.rs", 42, "fn search()"),
        make_match("src/app.rs", 108, "search()"),
        make_match("lib/utils.rs", 12, "search()"),
    ];

    let mut tree = SearchResultTree::build(&matches);

    // 最初の dir 行を見つける。
    let rows = tree.visible_rows().to_vec();
    let first_dir_idx = rows
        .iter()
        .position(|r| matches!(r, SearchTreeRow::Dir { .. }))
        .unwrap();

    // next_sibling は同じ深さの次の dir を見つけるはず。
    let sibling = tree.next_sibling_index(first_dir_idx);
    assert!(sibling.is_some());
}

#[test]
fn test_empty_matches() {
    let mut tree = SearchResultTree::build(&[]);
    assert_eq!(tree.match_count(), 0);
    assert_eq!(tree.visible_rows().len(), 0);
}

#[test]
fn test_root_level_files() {
    let matches = vec![
        make_match("README.md", 5, "search"),
        make_match("Cargo.toml", 10, "search"),
    ];

    let mut tree = SearchResultTree::build(&matches);
    let rows = tree.visible_rows();

    // トップレベルのファイルについてはファイル行があるはず。
    let file_names: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            SearchTreeRow::File { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert!(file_names.contains(&"README.md".to_string()));
    assert!(file_names.contains(&"Cargo.toml".to_string()));
}

#[test]
fn test_match_counts() {
    let matches = vec![
        make_match("src/app.rs", 42, "fn search()"),
        make_match("src/app.rs", 108, "search()"),
        make_match("src/app.rs", 205, "search opt"),
        make_match("src/ui/viewer.rs", 55, "search"),
    ];

    let mut tree = SearchResultTree::build(&matches);
    let rows = tree.visible_rows();

    // src/ ディレクトリはマッチ合計4件のはず。
    let src_dir = rows
        .iter()
        .find(|r| matches!(r, SearchTreeRow::Dir { name, .. } if name == "src"));
    assert!(matches!(
        src_dir,
        Some(SearchTreeRow::Dir { match_count: 4, .. })
    ));

    // app.rs ファイルはマッチ3件のはず。
    let app_file = rows
        .iter()
        .find(|r| matches!(r, SearchTreeRow::File { name, .. } if name == "app.rs"));
    assert!(matches!(
        app_file,
        Some(SearchTreeRow::File { match_count: 3, .. })
    ));
}

#[test]
fn a_directory_holding_only_subdirectories_is_still_collapsible() {
    // リグレッション: サブディレクトリのみを含む(直接のファイルを持たない)
    // ディレクトリも折りたたみ可能でなければならない。
    let matches = vec![
        make_match("src/ui/viewer.rs", 55, "search"),
        make_match("src/ui/explorer.rs", 10, "search"),
    ];

    let mut tree = SearchResultTree::build(&matches);
    let initial_count = tree.visible_rows().len();

    // "src" はディレクトリのみを含む dir(サブディレクトリ "ui" のみで直接のファイルを持たない)。
    let src_idx = tree
        .visible_rows()
        .iter()
        .position(|r| matches!(r, SearchTreeRow::Dir { name, .. } if name == "src"))
        .expect("src dir should exist");

    // src を折りたたむと配下全てが隠れるはず。
    tree.collapse(src_idx);
    let collapsed_count = tree.visible_rows().len();
    assert!(
        collapsed_count < initial_count,
        "collapsing directory-only dir should hide children: before={initial_count}, after={collapsed_count}"
    );
    // "src" の dir 行だけが残るはず。
    assert_eq!(collapsed_count, 1);

    // 再展開すると全ての行が復元されるはず。
    tree.expand(src_idx);
    assert_eq!(tree.visible_rows().len(), initial_count);
}
