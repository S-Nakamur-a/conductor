// テストが使う成果物 JSON の骨組み。
//
// schema・base/head・overview・coverage は検証の差分にならないのに、呼ぶ側で
// 書き写すとスキーマ版が古いまま残る。骨組みはここへ集めて、差分になる
// sections（と要るときだけ impacts）だけを渡してもらう。

/// sections の JSON 配列を渡すと、決まった骨組みで包んだ成果物 JSON を返す。
pub fn review(sections_json: &str) -> String {
    review_with_impacts(sections_json, "[]")
}

/// impacts も差し替えたいときはこちら。
pub fn review_with_impacts(sections_json: &str, impacts_json: &str) -> String {
    format!(
        r#"{{
  "schema": {schema}, "base": "aaa", "head": "bbb",
  "overview": {{"problem":"p","change":"c","mechanism":"m","placement":"pl","scope":"s"}},
  "sections": {sections_json},
  "impacts": {impacts_json},
  "coverage": {{"total":2,"classified":2,"unclassified":[],"conflicts":[],"unknown":[]}}
}}"#,
        schema = revidere::review::SCHEMA_VERSION,
    )
}
