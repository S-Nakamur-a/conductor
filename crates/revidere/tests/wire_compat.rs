// 実データが読めることを固定する。手書きのフィクスチャは形を writer 側に
// 合わせて書いてしまうので、writer が壊れたときに一緒に壊れて気付けない。
// 中身はこのリポジトリ自身の差分を解析した成果物で、採り直すたびに変わる値は見ない。

fn demo() -> revidere::Annotations {
    revidere::Annotations::from_json(include_str!("data/review.json"))
        .expect("成果物として読めること")
}

#[test]
fn 実データから取った成果物が今も読める() {
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

/// 読めはするが 1 つも引けない、という壊れ方を捕まえる。
#[test]
fn 実成果物の項目は位置を指している() {
    let a = demo();
    let ranges: usize = a.sections().iter().map(|s| s.ranges.len()).sum();
    assert!(ranges > 0, "どの項目も範囲を持っていない");
    assert!(
        a.sections().iter().all(|s| s.reason.is_some()),
        "理由の無い項目がある"
    );
}
