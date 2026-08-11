// 実データが読めることを固定する。手書きのフィクスチャは形を writer 側に
// 合わせて書いてしまうので、writer が壊れたときに一緒に壊れて気付けない。
//
// 中身は同梱のデモ（このリポジトリ自身の差分を解析した成果物）。make demo-update
// で採り直されるので、項目の数のような採るたびに変わる値は見ない。

fn demo() -> revidere::Annotations {
    revidere::Annotations::from_json(include_str!("data/review.json"))
        .expect("成果物として読めること")
}

#[test]
fn the_artifact_taken_from_real_data_still_loads() {
    let a = demo();
    assert!(!a.sections().is_empty(), "項目が 1 つも無い");
    assert!(
        !a.base().is_empty() && !a.head().is_empty(),
        "base/head が空"
    );
    assert!(
        a.coverage().is_complete(),
        "説明もれ検査を通っている成果物のはず"
    );
}

/// 項目が指す位置を、ライブラリが実際に引ける形で持っていること。
/// 読めはするが 1 つも引けない、という壊れ方をここで捕まえる。
#[test]
fn the_sections_in_the_real_artifact_point_at_positions() {
    let a = demo();
    let ranges: usize = a.sections().iter().map(|s| s.ranges.len()).sum();
    assert!(ranges > 0, "どの項目も範囲を持っていない");
    assert!(
        a.sections().iter().all(|s| s.reason.is_some()),
        "理由の無い項目がある"
    );
}
