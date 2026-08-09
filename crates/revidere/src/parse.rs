// モデルの応答を成果物へ変える。
//
// 応答は「JSON だけ返せ」と言ってあるが、地の文やコードフェンスが付くことは
// 現実に起きる。ここで許容するのは取り出しまでで、中身の緩さは許容しない。

use crate::review::{Confidence, Coverage, Impact, Overview, Review, Section, Side};
use serde::Deserialize;

#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// coverage を持たない、検証前の応答。
/// coverage は検査の出力なので、モデルには書かせない。
#[derive(Debug, Deserialize)]
struct Generated {
    overview: Overview,
    #[serde(default)]
    sections: Vec<Section>,
    #[serde(default)]
    impacts: Vec<Impact>,
}

/// 応答を Review にする。coverage は空のまま返るので、呼ぶ側が検査して埋める。
pub fn review(raw: &str, base: &str, head: &str) -> Result<Review, ParseError> {
    let json = extract_object(raw)
        .ok_or_else(|| ParseError("応答に JSON オブジェクトが見つからない".into()))?;
    let g: Generated = serde_json::from_str(json)
        .map_err(|e| ParseError(format!("JSON を型に落とせない: {e}")))?;
    let mut r = Review {
        schema: crate::review::SCHEMA_VERSION,
        base: base.to_string(),
        head: head.to_string(),
        overview: g.overview,
        sections: g.sections,
        impacts: g.impacts,
        coverage: Coverage::default(),
    };
    validate(&r)?;
    r.sort_sections();
    Ok(r)
}

/// 形式は通ったが中身が約束を破っている場合を弾く。
fn validate(r: &Review) -> Result<(), ParseError> {
    let o = &r.overview;
    for (name, v) in [
        ("problem", &o.problem),
        ("change", &o.change),
        ("mechanism", &o.mechanism),
        ("placement", &o.placement),
        ("scope", &o.scope),
    ] {
        if v.trim().is_empty() {
            return Err(ParseError(format!("overview.{name} が空")));
        }
    }
    if r.sections.is_empty() {
        return Err(ParseError("sections が空".into()));
    }
    for (i, c) in r.sections.iter().enumerate() {
        if c.title.trim().is_empty() {
            return Err(ParseError(format!("sections[{i}].title が空")));
        }
        // 理由はどの重要度でも必須。一部にだけ課すと、書く手間を避けて課して
        // いない側へ逃げる。読み飛ばしてよい札が使われないと読む量が減らない。
        if c.reason.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err(ParseError(format!(
                "sections[{i}]（{}）に reason が無い",
                c.title
            )));
        }
        for (j, range) in c.ranges.iter().enumerate() {
            if range.path.trim().is_empty() {
                return Err(ParseError(format!("sections[{i}].ranges[{j}].path が空")));
            }
            match (range.side, range.start, range.end) {
                (Side::File, _, _) => {}
                (_, Some(s), Some(e)) if s <= e => {}
                (_, Some(_), None) => {}
                _ => {
                    return Err(ParseError(format!(
                        "sections[{i}].ranges[{j}] の行範囲が壊れている（{} start={:?} end={:?}）",
                        range.path, range.start, range.end
                    )))
                }
            }
        }
    }
    for (i, im) in r.impacts.iter().enumerate() {
        if im.feature.trim().is_empty() {
            return Err(ParseError(format!("impacts[{i}].feature が空")));
        }
        if im.confidence == Confidence::Fact && im.change.trim().is_empty() {
            return Err(ParseError(format!(
                "impacts[{i}] は fact なのに change が空"
            )));
        }
    }
    Ok(())
}

/// 地の文やコードフェンスに埋まった JSON オブジェクトを取り出す。
///
/// 文字列リテラルの中の波括弧を数えないよう、エスケープと引用符を見る。
pub fn extract_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::review::Importance;

    const MINIMAL: &str = r#"{
      "overview": {"problem":"p","change":"c","mechanism":"m","placement":"pl","scope":"s"},
      "sections": [
        {"title":"t","body":"b","importance":"core","reason":"r",
         "ranges":[{"path":"a.rs","side":"new","start":1,"end":2}]}
      ],
      "impacts": [
        {"feature":"f","change":"ch","verify":"v","confidence":"guess"}
      ]
    }"#;

    #[test]
    fn parses_a_minimal_answer() {
        let r = review(MINIMAL, "main", "HEAD").unwrap();
        assert_eq!(r.schema, crate::review::SCHEMA_VERSION);
        assert_eq!(r.sections.len(), 1);
        assert_eq!(r.sections[0].importance, Importance::Core);
        // coverage は検査の出力なので、パースの時点では空。
        assert_eq!(r.coverage.total, 0);
    }

    #[test]
    fn extracts_json_from_a_fenced_and_chatty_answer() {
        let raw = format!("わかりました。\n```json\n{MINIMAL}\n```\n以上です。");
        assert!(review(&raw, "main", "HEAD").is_ok());
    }

    #[test]
    fn braces_inside_strings_do_not_end_the_object() {
        let raw = r#"{"overview":{"problem":"format!(\"{}\", x) を使う","change":"c",
          "mechanism":"m","placement":"p","scope":"s"},
          "sections":[{"title":"t","body":"b","importance":"core","reason":"r","ranges":[]}],
          "impacts":[]}"#;
        let r = review(raw, "a", "b").unwrap();
        assert!(r.overview.problem.contains("format!"));
    }

    #[test]
    fn reason_is_required_at_every_importance() {
        for imp in ["core", "ripple", "follow", "minor"] {
            let raw = MINIMAL
                .replace("\"core\"", &format!("\"{imp}\""))
                .replace(",\"reason\":\"r\"", "");
            let err = review(&raw, "main", "HEAD")
                .unwrap_err_or_panic(&format!("{imp} で reason 無しが通ってしまった"));
            assert!(err.0.contains("reason"), "{}", err.0);
        }
    }

    #[test]
    fn follow_with_reason_is_accepted() {
        let raw = MINIMAL.replace("\"core\"", "\"follow\"");
        assert!(review(&raw, "main", "HEAD").is_ok());
    }

    /// テスト用: Err であることを、失敗時のメッセージ付きで確かめる。
    trait UnwrapErrOrPanic<E> {
        fn unwrap_err_or_panic(self, msg: &str) -> E;
    }
    impl<T, E> UnwrapErrOrPanic<E> for Result<T, E> {
        fn unwrap_err_or_panic(self, msg: &str) -> E {
            match self {
                Err(e) => e,
                Ok(_) => panic!("{msg}"),
            }
        }
    }

    #[test]
    fn empty_overview_field_is_rejected() {
        for (field, filled) in [
            ("problem", r#""problem":"p""#),
            ("change", r#""change":"c""#),
            ("mechanism", r#""mechanism":"m""#),
            ("placement", r#""placement":"pl""#),
            ("scope", r#""scope":"s""#),
        ] {
            let raw = MINIMAL.replace(filled, &format!("\"{field}\":\"  \""));
            let err = review(&raw, "main", "HEAD")
                .unwrap_err_or_panic(&format!("{field} が空でも通ってしまった"));
            assert!(err.0.contains(field), "{}", err.0);
        }
    }

    #[test]
    fn empty_sections_is_rejected() {
        let raw = r#"{"overview":{"problem":"p","change":"c","mechanism":"m","placement":"p","scope":"s"},
          "sections":[],"impacts":[]}"#;
        let err = review(raw, "a", "b").unwrap_err();
        assert!(err.0.contains("sections"), "{}", err.0);
    }

    #[test]
    fn an_impact_without_a_feature_name_is_rejected() {
        let raw = MINIMAL.replace("\"feature\":\"f\"", "\"feature\":\"  \"");
        let err = review(&raw, "main", "HEAD").unwrap_err();
        assert!(err.0.contains("feature"), "{}", err.0);
    }

    #[test]
    fn a_fact_impact_without_a_change_is_rejected() {
        // guess のときは change を空のままでよいが、fact（確定）は必須。
        let raw = MINIMAL
            .replace("\"confidence\":\"guess\"", "\"confidence\":\"fact\"")
            .replace("\"change\":\"ch\"", "\"change\":\"  \"");
        let err = review(&raw, "main", "HEAD").unwrap_err();
        assert!(err.0.contains("fact"), "{}", err.0);
    }

    #[test]
    fn a_guess_impact_may_leave_change_empty() {
        let raw = MINIMAL.replace("\"change\":\"ch\"", "\"change\":\"\"");
        assert!(review(&raw, "main", "HEAD").is_ok());
    }

    #[test]
    fn reversed_range_is_rejected_at_parse_time() {
        // 展開すると空になって unclassified に化けるので、ここで名指しで落とす。
        let raw = MINIMAL.replace("\"start\":1,\"end\":2", "\"start\":9,\"end\":2");
        let err = review(&raw, "main", "HEAD").unwrap_err();
        assert!(err.0.contains("行範囲"), "{}", err.0);
    }

    #[test]
    fn file_side_range_without_line_numbers_is_accepted() {
        let raw = MINIMAL.replace(
            "{\"path\":\"a.rs\",\"side\":\"new\",\"start\":1,\"end\":2}",
            "{\"path\":\"logo.png\",\"side\":\"file\"}",
        );
        assert!(review(&raw, "main", "HEAD").is_ok());
    }

    #[test]
    fn contexts_come_back_sorted_by_importance() {
        let raw = r#"{"overview":{"problem":"p","change":"c","mechanism":"m","placement":"p","scope":"s"},
          "sections":[
            {"title":"minor","body":"b","importance":"minor","reason":"r","ranges":[]},
            {"title":"core","body":"b","importance":"core","reason":"r","ranges":[]},
            {"title":"ripple","body":"b","importance":"ripple","reason":"r","ranges":[]}
          ],"impacts":[]}"#;
        let r = review(raw, "a", "b").unwrap();
        let titles: Vec<&str> = r.sections.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["core", "ripple", "minor"]);
    }

    #[test]
    fn answer_without_json_is_an_error_not_a_panic() {
        let err = review("すみません、できませんでした。", "a", "b").unwrap_err();
        assert!(err.0.contains("JSON"), "{}", err.0);
    }

    #[test]
    fn unknown_importance_value_is_rejected() {
        let raw = MINIMAL.replace("\"core\"", "\"important\"");
        assert!(review(&raw, "a", "b").is_err());
    }
}
