use super::*;
use crate::diff_state::{DiffHunk, DiffLine, DiffLineTag, DiffSource, FileDiff};

/// path のハンク 1 つが new_lines の行番号だけを持つ差分。
fn diff_with_hunk(path: &str, new_lines: &[usize]) -> DiffState {
    let mut ds = DiffState::new(DiffSource::working_tree("main"));
    ds.files = vec![FileDiff {
        path: path.to_string(),
        added_lines: new_lines.len(),
        deleted_lines: 0,
        hunks: vec![DiffHunk {
            lines: new_lines
                .iter()
                .map(|&n| DiffLine {
                    tag: DiffLineTag::Insert,
                    old_line_no: None,
                    new_line_no: Some(n),
                    inline_segments: Vec::new(),
                    content: String::new(),
                })
                .collect(),
            func_header: None,
        }],
    }];
    ds
}

fn comment(file_path: &str, line_start: u32, line_end: Option<u32>) -> PublishComment {
    PublishComment {
        id: format!("{file_path}:{line_start}"),
        file_path: file_path.to_string(),
        line_start,
        line_end,
        body: "looks good".to_string(),
    }
}

#[test]
fn diffのハンクに収まるコメントだけを残す() {
    let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
    let cases = [
        ("単一行", comment("src/a.rs", 11, None), true),
        (
            "両端がハンク内の範囲",
            comment("src/a.rs", 10, Some(12)),
            true,
        ),
        ("ハンク外の行", comment("src/a.rs", 99, None), false),
        (
            "差分に無いファイル",
            comment("src/missing.rs", 10, None),
            false,
        ),
    ];
    for (name, c, want_kept) in cases {
        let (kept, skipped) = filter_publishable(vec![c], &diff);
        assert_eq!(kept.len(), usize::from(want_kept), "{name}");
        assert_eq!(skipped, usize::from(!want_kept), "{name}");
    }
}

#[test]
fn prのurlからownerとrepoを取る() {
    let cases = [
        (
            "https://github.com/S-Nakamur-a/conductor/pull/279",
            Some(("S-Nakamur-a", "conductor")),
        ),
        ("https://example.com/o/r/pull/1", None),
        ("https://github.com//r/pull/1", None),
        ("https://github.com/o", None),
    ];
    for (url, want) in cases {
        let want = want.map(|(o, r)| (o.to_string(), r.to_string()));
        assert_eq!(owner_repo_from_pr_url(url), want, "{url}");
    }
}

#[test]
fn 範囲のコメントだけがstart_lineを持つ() {
    let single = serde_json::to_value(ReviewCommentPayload::from_comment(&comment(
        "src/a.rs", 10, None,
    )))
    .unwrap();
    assert_eq!(single["line"], 10);
    assert_eq!(single["side"], "RIGHT");
    assert!(single.get("start_line").is_none());
    assert!(single.get("start_side").is_none());

    let range = serde_json::to_value(ReviewCommentPayload::from_comment(&comment(
        "src/a.rs",
        10,
        Some(15),
    )))
    .unwrap();
    assert_eq!(range["line"], 15);
    assert_eq!(range["start_line"], 10);
    assert_eq!(range["start_side"], "RIGHT");
}

/// コメントが空ならコミット ID の問い合わせも起きない。gh もネットワークも無い
/// テスト環境で publish() を通せる唯一の経路でもある。
#[test]
fn コメントが無ければghを呼ばずに成功する() {
    let outcome = publish(PublishRequest {
        owner: "o".to_string(),
        repo: "r".to_string(),
        pr_number: 1,
        comments: Vec::new(),
    });
    assert_eq!(
        outcome,
        PublishOutcome::Succeeded {
            published_ids: Vec::new()
        }
    );
}
