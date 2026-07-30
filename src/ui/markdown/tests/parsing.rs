//! Tests for block parsing ([`parse_blocks`]), inline emphasis/link parsing
//! ([`inline_spans`]), task checkboxes, and GFM tables.

use super::*;

// ── Parsing ──

#[test]
fn plain_text_is_paragraphs_unchanged() {
    let blocks = parse_blocks("Just a normal sentence.\nSecond line.");
    assert_eq!(
        blocks,
        vec![
            MdBlock::Paragraph("Just a normal sentence.".to_string()),
            MdBlock::Paragraph("Second line.".to_string()),
        ]
    );
}

#[test]
fn hash_without_space_is_not_a_heading() {
    // Issue refs / C# / #nofilter must stay paragraphs.
    assert_eq!(
        parse_blocks("fix issue #242 now"),
        vec![MdBlock::Paragraph("fix issue #242 now".to_string())]
    );
    assert_eq!(
        parse_blocks("#nofilter"),
        vec![MdBlock::Paragraph("#nofilter".to_string())]
    );
    assert_eq!(
        parse_blocks("####### too many"),
        vec![MdBlock::Paragraph("####### too many".to_string())]
    );
}

#[test]
fn headings_parse_with_level() {
    assert_eq!(
        parse_blocks("# Title\n### Sub"),
        vec![
            MdBlock::Heading { level: 1, text: "Title".to_string() },
            MdBlock::Heading { level: 3, text: "Sub".to_string() },
        ]
    );
}

#[test]
fn list_items_bullet_and_ordered() {
    assert_eq!(
        parse_blocks("- a\n* b\n1. c\n2) d"),
        vec![
            MdBlock::ListItem { ordered: None, checked: None, text: "a".to_string(), indent: 0 },
            MdBlock::ListItem { ordered: None, checked: None, text: "b".to_string(), indent: 0 },
            MdBlock::ListItem { ordered: Some("1".to_string()), checked: None, text: "c".to_string(), indent: 0 },
            MdBlock::ListItem { ordered: Some("2".to_string()), checked: None, text: "d".to_string(), indent: 0 },
        ]
    );
    // No space after the marker → plain paragraph.
    assert_eq!(
        parse_blocks("-5 degrees"),
        vec![MdBlock::Paragraph("-5 degrees".to_string())]
    );
}

#[test]
fn unclosed_fence_consumes_to_eof() {
    let blocks = parse_blocks("```rust\nlet x = 1;\nfn y() {}");
    assert_eq!(
        blocks,
        vec![MdBlock::CodeBlock {
            lang: Some("rust".to_string()),
            lines: vec!["let x = 1;".to_string(), "fn y() {}".to_string()],
        }]
    );
}

#[test]
fn fence_without_lang_and_crlf() {
    let blocks = parse_blocks("```\r\ncode\r\n```\r\n");
    assert_eq!(
        blocks,
        vec![
            MdBlock::CodeBlock { lang: None, lines: vec!["code".to_string()] },
            MdBlock::Blank,
        ]
    );
}

#[test]
fn fence_does_not_interpret_inner_markdown() {
    let blocks = parse_blocks("```\n# not a heading\n- not a list\n```");
    assert_eq!(
        blocks,
        vec![MdBlock::CodeBlock {
            lang: None,
            lines: vec!["# not a heading".to_string(), "- not a list".to_string()],
        }]
    );
}

#[test]
fn horizontal_rule_vs_text() {
    assert_eq!(parse_blocks("---"), vec![MdBlock::Rule]);
    assert_eq!(parse_blocks("***"), vec![MdBlock::Rule]);
    assert_eq!(
        parse_blocks("a - b"),
        vec![MdBlock::Paragraph("a - b".to_string())]
    );
}

// ── Inline ──

#[test]
fn snake_case_and_bare_star_stay_literal() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    // Underscores are never emphasis.
    let spans = inline_spans("call set_change_summary here", base, &theme);
    assert_eq!(joined(&spans), "call set_change_summary here");
    assert_eq!(spans.len(), 1, "no styled split for snake_case");
    // `2 * 3`: space-flanked star is literal.
    let spans = inline_spans("rate is 2 * 3", base, &theme);
    assert_eq!(joined(&spans), "rate is 2 * 3");
    assert_eq!(spans.len(), 1);
}

#[test]
fn bold_italic_code_are_styled() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);

    let spans = inline_spans("a **b** c", base, &theme);
    assert_eq!(joined(&spans), "a b c");
    assert!(
        spans
            .iter()
            .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::BOLD))
    );

    let spans = inline_spans("a *b* c", base, &theme);
    assert!(
        spans
            .iter()
            .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::ITALIC))
    );

    let spans = inline_spans("use `git` now", base, &theme);
    // Inline code is padded with NBSP into a pink-on-card chip; match on the
    // trimmed content rather than the exact padded string.
    assert!(
        spans
            .iter()
            .any(|s| s.content.trim_matches('\u{a0}') == "git"
                && s.style.fg == Some(theme.code_fg)
                && s.style.bg == Some(theme.code_bg))
    );
    assert_eq!(joined(&spans), "use \u{a0}git\u{a0} now");
}

#[test]
fn unclosed_inline_delimiters_stay_literal() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    for input in [
        "a `b", "a *b", "a **b", "*", "`", "a ~~b", "~~", "~", "a ~ b", "~/foo",
        "a ~~ b ~~ c",
    ] {
        let spans = inline_spans(input, base, &theme);
        assert_eq!(joined(&spans), input, "input {input:?} should be literal");
    }
}

#[test]
fn strikethrough_is_styled_and_muted() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans("keep ~~drop~~ this", base, &theme);
    assert_eq!(joined(&spans), "keep drop this");
    let struck = spans.iter().find(|s| s.content == "drop").unwrap();
    assert!(struck.style.add_modifier.contains(Modifier::CROSSED_OUT));
    assert_eq!(struck.style.fg, Some(theme.muted));
}

#[test]
fn strikethrough_does_not_nest_inner_markup() {
    // Like bold/italic, strikethrough emits its content literally (no
    // nesting) — but inline markup OUTSIDE the strike still works.
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans("~~old **bold**~~ and **real**", base, &theme);
    assert!(
        spans
            .iter()
            .any(|s| s.content == "old **bold**"
                && s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
        "struck run keeps `**` literal"
    );
    assert!(
        spans
            .iter()
            .any(|s| s.content == "real" && s.style.add_modifier.contains(Modifier::BOLD)),
        "bold outside the strike still applies"
    );
}

#[test]
fn bare_tilde_run_is_fence_not_strikethrough() {
    // `~~~` at line start is a code fence, parsed before inline strike.
    assert!(matches!(
        parse_blocks("~~~\ncode\n~~~").as_slice(),
        [MdBlock::CodeBlock { .. }]
    ));
}

// ── Task checkboxes ──

#[test]
fn task_checkboxes_parse() {
    assert_eq!(
        parse_blocks("- [ ] todo\n- [x] done\n- [X] also\n1. [ ] num"),
        vec![
            MdBlock::ListItem { ordered: None, checked: Some(false), text: "todo".to_string(), indent: 0 },
            MdBlock::ListItem { ordered: None, checked: Some(true), text: "done".to_string(), indent: 0 },
            MdBlock::ListItem { ordered: None, checked: Some(true), text: "also".to_string(), indent: 0 },
            MdBlock::ListItem { ordered: Some("1".to_string()), checked: Some(false), text: "num".to_string(), indent: 0 },
        ]
    );
}

#[test]
fn non_checkboxes_stay_plain_items() {
    // Malformed markers must NOT become checkboxes; text is preserved verbatim.
    for (input, want_text) in [
        ("- [y] thing", "[y] thing"),
        ("- [] thing", "[] thing"),
        ("- [ ]nospace", "[ ]nospace"),
        ("- [ x] thing", "[ x] thing"),
    ] {
        assert_eq!(
            parse_blocks(input),
            vec![MdBlock::ListItem {
                ordered: None,
                checked: None,
                text: want_text.to_string(),
                indent: 0,
            }],
            "{input:?}"
        );
    }
}

#[test]
fn empty_task_checkbox_parses() {
    assert_eq!(
        parse_blocks("- [ ]"),
        vec![MdBlock::ListItem {
            ordered: None,
            checked: Some(false),
            text: String::new(),
            indent: 0,
        }]
    );
}

#[test]
fn checkbox_renders_within_width() {
    // At widths comfortably above the `[x] ` marker, lines stay in bounds.
    // (Like all list items, a width narrower than the marker can't be
    // honoured — that degenerate case is covered by `never_panics`.)
    for width in [8usize, 20, 40] {
        for line in render("- [x] done\n- [ ] あいうえお task", width) {
            assert!(display_width(&line_text(&line)) <= width);
        }
    }
}

// ── Tables ──

#[test]
fn table_parses_headers_aligns_rows() {
    assert_eq!(
        parse_blocks("| h1 | h2 |\n| --- | :--: |\n| a | b |\n| c | d |"),
        vec![MdBlock::Table {
            headers: vec!["h1".to_string(), "h2".to_string()],
            aligns: vec![Align::Left, Align::Center],
            rows: vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string(), "d".to_string()],
            ],
        }]
    );
}

#[test]
fn pipe_paragraph_is_not_a_table() {
    // No delimiter row → not a table; no source line is eaten.
    assert_eq!(
        parse_blocks("a | b\nc | d"),
        vec![
            MdBlock::Paragraph("a | b".to_string()),
            MdBlock::Paragraph("c | d".to_string()),
        ]
    );
    // Header-looking line at EOF with no delimiter.
    assert_eq!(
        parse_blocks("| h1 | h2 |"),
        vec![MdBlock::Paragraph("| h1 | h2 |".to_string())]
    );
    // Delimiter with zero dashes is not a delimiter.
    assert_eq!(
        parse_blocks("| a | b |\n| : | : |").len(),
        2,
        "no-dash second line means two paragraphs, not a table"
    );
}

#[test]
fn table_cell_splitting_normalizes_outer_pipes() {
    assert_eq!(split_table_row("| a | b |"), vec!["a", "b"]);
    assert_eq!(split_table_row("a | b"), vec!["a", "b"]);
    assert_eq!(split_table_row("| a | b"), vec!["a", "b"]);
    assert_eq!(split_table_row("a | b |"), vec!["a", "b"]);
}

#[test]
fn table_renders_within_width_and_truncates() {
    // Header + rule + 2 body rows = 4 lines, all within width.
    let table = "| name | id |\n| --- | --: |\n| alice | 1 |\n| bob | 22 |";
    for width in [0usize, 1, 2, 3, 8, 20, 80] {
        let lines = render(table, width);
        for line in &lines {
            assert!(
                display_width(&line_text(line)) <= width.max(1),
                "table line exceeds width {width}"
            );
        }
    }
}

/// The point of wrapping instead of truncating: **no content is lost.** Every
/// word of an over-wide cell must appear somewhere in the rendered table, at
/// every width that can hold the column at all.
#[test]
fn wide_table_cells_wrap_instead_of_losing_content() {
    let table = "| feature | notes |\n| --- | --- |\n\
        | toggle | switches a markdown file between raw source and rendered prose |\n\
        | scroll | independent of the raw view |";
    let words = [
        "switches", "markdown", "between", "source", "rendered", "prose", "independent",
    ];
    for width in [20usize, 30, 40, 60, 100] {
        let text: String = render(table, width)
            .iter()
            .map(|l| line_text(l))
            .collect::<Vec<_>>()
            .join("\n");
        for w in words {
            assert!(
                text.contains(w),
                "width {width}: lost {w:?} from the table\n{text}"
            );
        }
        assert!(
            !text.contains('\u{2026}'),
            "width {width}: cell was still elided\n{text}"
        );
    }
}

/// Wrapping makes a row taller, so the columns must stay a grid: every line of
/// a row has to be the same display width, or the second column ends up ragged.
#[test]
fn wrapped_table_rows_keep_their_columns_aligned() {
    let table = "| a | b |\n| --- | --- |\n\
        | one two three four five | six |\n\
        | x | seven eight nine ten eleven |";
    for width in [24usize, 36, 50] {
        let lines = render(table, width);
        // Skip the leading blank the renderer puts before a block.
        let body: Vec<usize> = lines
            .iter()
            .map(|l| display_width(&line_text(l)))
            .filter(|&w| w > 0)
            .collect();
        assert!(
            body.iter().all(|&w| w == body[0]),
            "width {width}: ragged row widths {body:?}"
        );
    }
}

/// A single unbreakable token wider than its column still has to appear in
/// full — hard-split across lines rather than cut short.
#[test]
fn overlong_unbreakable_cell_is_split_not_cut() {
    let url = "https://example.com/a/very/long/path/that/never/breaks";
    let table = format!("| link |\n| --- |\n| {url} |");
    let text: String = render(&table, 24)
        .iter()
        .map(|l| line_text(l))
        .collect::<Vec<_>>()
        .join("");
    // Reassembled across lines (padding stripped), the whole URL is present.
    let joined: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(joined.contains(url), "URL was cut: {text:?}");
}

#[test]
fn table_cell_truncation_never_splits_multibyte() {
    // Force CJK / accented cells below their content width — must not panic
    // and must respect the width bound.
    let table = "| name |\n| ---- |\n| café |\n| 日本語テスト |\n| 🧑‍🤝‍🧑x |";
    for width in [1usize, 2, 3, 4, 5, 6, 10] {
        for line in render(table, width) {
            assert!(display_width(&line_text(&line)) <= width.max(1));
        }
    }
}

#[test]
fn table_alignment_does_not_change_cell_width() {
    // Same content under each alignment yields identical column widths.
    let mk = |delim: &str| render(&format!("| h |\n| {delim} |\n| ab |"), 20);
    let widths: Vec<usize> = ["---", ":--", "--:", ":-:"]
        .iter()
        .map(|d| line_text(&mk(d)[2]).trim_end().chars().count())
        .collect();
    // Left/right/center pad differently but the trimmed body content is "ab".
    for d in ["---", ":--", "--:", ":-:"] {
        let body = line_text(&mk(d)[2]);
        assert!(body.contains("ab"), "alignment {d} lost content");
    }
    // The full (untrimmed) row width is identical across alignments.
    let full: Vec<usize> = ["---", ":--", "--:", ":-:"]
        .iter()
        .map(|d| display_width(&line_text(&mk(d)[2])))
        .collect();
    assert!(full.iter().all(|&w| w == full[0]), "row widths differ: {full:?}");
    let _ = widths;
}

#[test]
fn table_ragged_rows_are_normalized() {
    // Short and long rows render without panic, padded/truncated to header.
    let table = "| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |";
    let lines = render(table, 40);
    // header + rule + 2 rows.
    assert_eq!(lines.len(), 4);
}

#[test]
fn links_render_text_and_url() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);

    // Link text shown, URL kept in a recessive parenthetical.
    let spans = inline_spans("see [the docs](https://example.com) now", base, &theme);
    assert_eq!(joined(&spans), "see the docs (https://example.com) now");
    assert!(
        spans.iter().any(|s| s.content == "the docs"
            && s.style.add_modifier.contains(Modifier::UNDERLINED)),
        "link text is underlined"
    );

    // Inline markup inside the link text is still styled.
    let spans = inline_spans("[**bold** link](https://x.io)", base, &theme);
    assert!(
        spans
            .iter()
            .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)),
        "emphasis inside link text is preserved"
    );
    assert!(joined(&spans).contains("(https://x.io)"));
}

#[test]
fn self_titled_and_empty_links_show_url_once() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);

    let spans = inline_spans("[https://x.com](https://x.com)", base, &theme);
    assert_eq!(joined(&spans), "https://x.com");

    // Trailing-slash / case differences still collapse.
    let spans = inline_spans("[https://x.com/](https://x.com)", base, &theme);
    assert_eq!(joined(&spans), "https://x.com");

    let spans = inline_spans("[](https://x.com)", base, &theme);
    assert_eq!(joined(&spans), "https://x.com");
}

#[test]
fn malformed_links_stay_literal() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    for input in ["[text]", "[text](", "[text(url)", "a [b] c", "["] {
        let spans = inline_spans(input, base, &theme);
        assert_eq!(joined(&spans), input, "{input:?} should stay literal");
    }
}

#[test]
fn link_preserves_trailing_text() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans("[a](b)c", base, &theme);
    assert_eq!(joined(&spans), "a (b)c");
}

fn joined(spans: &[Span]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}
