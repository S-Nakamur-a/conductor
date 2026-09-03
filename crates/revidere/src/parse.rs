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
        // 前回からの進みは git から引くもので、応答には無い。
        since_previous: None,
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
    use revidere_fixtures::Section as S;

    const RANGE: (&str, &str, u32, Option<u32>) = ("a.rs", "new", 1, Some(2));

    fn one_section() -> S {
        S::new("t", "core").lines(RANGE.0, RANGE.1, RANGE.2, RANGE.3)
    }

    const IMPACT: &str = r#"[{"feature":"f","change":"ch","verify":"v","confidence":"guess"}]"#;

    fn answer_of(sections: &[S], impacts: &str) -> String {
        revidere_fixtures::answer_with_impacts(&revidere_fixtures::sections(sections), impacts)
    }

    fn minimal() -> String {
        answer_of(&[one_section()], IMPACT)
    }

    #[test]
    fn 最小の答えを読める() {
        let r = review(&minimal(), "main", "HEAD").unwrap();
        assert_eq!(r.schema, crate::review::SCHEMA_VERSION);
        assert_eq!(r.sections.len(), 1);
        assert_eq!(r.sections[0].importance, Importance::Core);
        // coverage は検査の出力なので、パースの時点では空。
        assert_eq!(r.coverage.total, 0);
    }

    #[test]
    fn 約束を守っている答えは受け入れる() {
        let file_range = S::new("t", "core").whole_file("logo.png");
        let no_change = r#"[{"feature":"f","change":"","verify":"v","confidence":"guess"}]"#;
        for (name, raw) in [
            (
                "フェンスと地の文に埋もれている",
                format!("わかりました。\n```json\n{}\n```\n以上です。", minimal()),
            ),
            (
                "理由付きの follow",
                answer_of(
                    &[S::new("t", "follow").lines("a.rs", "new", 1, Some(2))],
                    IMPACT,
                ),
            ),
            (
                "ファイル側の範囲は行番号が無い",
                answer_of(&[file_range], IMPACT),
            ),
            (
                "予想の影響なら変化が空でもよい",
                answer_of(&[one_section()], no_change),
            ),
        ] {
            assert!(review(&raw, "main", "HEAD").is_ok(), "{name}");
        }
    }

    /// 波括弧を数えるだけだと、文字列リテラルの中の括弧でオブジェクトが終わる。
    #[test]
    fn 文字列の中の波括弧でオブジェクトは終わらない() {
        let raw = r#"{"overview":{"problem":"format!(\"{}\", x) を使う","change":"c",
          "mechanism":"m","placement":"p","scope":"s"},
          "sections":[{"title":"t","body":"b","importance":"core","reason":"r","ranges":[]}],
          "impacts":[]}"#;
        let r = review(raw, "a", "b").unwrap();
        assert!(r.overview.problem.contains("format!"));
    }

    #[test]
    fn 約束を破っている答えは名指しで拒む() {
        let impact = |change: &str, confidence: &str| {
            format!(
                r#"[{{"feature":"f","change":"{change}","verify":"v","confidence":"{confidence}"}}]"#
            )
        };
        let mut cases: Vec<(&str, String, &str)> = vec![
            (
                "JSON が 1 つも無い",
                "すみません、できませんでした。".to_string(),
                "JSON",
            ),
            ("項目が空", answer_of(&[], IMPACT), "sections"),
            (
                "見出しが空",
                answer_of(&[S::new("  ", "core")], IMPACT),
                "title",
            ),
            (
                "知らない重要度",
                answer_of(&[S::new("t", "important")], IMPACT),
                "JSON",
            ),
            (
                // 展開すると空になって unclassified に化けるので、ここで落とす。
                "逆向きの範囲",
                answer_of(
                    &[S::new("t", "core").lines("a.rs", "new", 9, Some(2))],
                    IMPACT,
                ),
                "行範囲",
            ),
            (
                "範囲のパスが空",
                answer_of(
                    &[S::new("t", "core").lines("  ", "new", 1, Some(2))],
                    IMPACT,
                ),
                "path",
            ),
            (
                "機能名の無い影響",
                answer_of(
                    &[one_section()],
                    r#"[{"feature":"  ","change":"ch","verify":"v","confidence":"guess"}]"#,
                ),
                "feature",
            ),
            (
                // guess のときは change を空のままでよいが、fact は必須。
                "確定の影響で変化が空",
                answer_of(&[one_section()], &impact("  ", "fact")),
                "fact",
            ),
        ];
        // 理由はどの重要度でも必須。一部にだけ課すと課していない側へ逃げる。
        for imp in ["core", "ripple", "follow", "minor"] {
            cases.push((
                "理由が無い",
                answer_of(
                    &[S::new("t", imp)
                        .reason(None)
                        .lines("a.rs", "new", 1, Some(2))],
                    IMPACT,
                ),
                "reason",
            ));
        }

        for (name, raw, want) in cases {
            let err = review(&raw, "main", "HEAD")
                .err()
                .unwrap_or_else(|| panic!("{name}: 通ってしまった"));
            assert!(err.0.contains(want), "{name}: {}", err.0);
        }
    }

    #[test]
    fn 概要はどの欄が空でも拒む() {
        for (field, filled) in [
            ("problem", r#""problem":"p""#),
            ("change", r#""change":"c""#),
            ("mechanism", r#""mechanism":"m""#),
            ("placement", r#""placement":"pl""#),
            ("scope", r#""scope":"s""#),
        ] {
            let raw = minimal().replace(filled, &format!("\"{field}\":\"  \""));
            let err = review(&raw, "main", "HEAD")
                .err()
                .unwrap_or_else(|| panic!("{field} が空でも通ってしまった"));
            assert!(err.0.contains(field), "{}", err.0);
        }
    }

    #[test]
    fn 項目は重要度順に並んで返る() {
        let raw = answer_of(
            &[
                S::new("minor", "minor"),
                S::new("core", "core"),
                S::new("ripple", "ripple"),
            ],
            "[]",
        );
        let r = review(&raw, "a", "b").unwrap();
        let titles: Vec<&str> = r.sections.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["core", "ripple", "minor"]);
    }
}
