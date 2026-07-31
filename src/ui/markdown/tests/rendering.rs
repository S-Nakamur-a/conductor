//! Tests for span wrapping, adversarial-input robustness, heading/code-block
//! rendering, the Transcript flavor, and [`MarkdownCache`].

use super::*;
use ratatui::style::Color;

// ── Wrapping / width ──

#[test]
fn wraps_to_width_and_preserves_words() {
    let lines = render("the quick brown fox", 9);
    for line in &lines {
        assert!(display_width(&line_text(line)) <= 9);
    }
    assert_eq!(lines.iter().map(line_text).collect::<Vec<_>>().join(" ").replace("  ", " ").trim(), "the quick brown fox");
}

#[test]
fn full_width_cjk_wraps_by_display_width() {
    // 6 full-width chars = 12 columns; at width 10 it must split.
    let lines = render("ああああああ", 10);
    for line in &lines {
        assert!(display_width(&line_text(line)) <= 10);
    }
    let joined: String = lines.iter().map(line_text).collect();
    assert_eq!(joined, "ああああああ");
}

#[test]
fn overlong_token_is_hard_split() {
    let lines = render(&"a".repeat(12), 5);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(texts, vec!["aaaaa", "aaaaa", "aa"]);
}

#[test]
fn blank_line_preserved_as_spacing() {
    let lines = render("a\n\nb", 20);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(texts, vec!["a", "", "b"]);
}

// ── Robustness ──

#[test]
fn never_panics_on_adversarial_input() {
    let inputs = [
        "",
        "```",
        "```rust",
        "~~~~",
        "###### ",
        "#",
        ">",
        "- ",
        "**",
        "`",
        "🧑‍🤝‍🧑 *x* `y`",
        "\t\tcode",
        "a\r\nb\r\n```\r\nc",
        "[",
        "[](",
        "[]()",
        "[x](y",
        "[**](http://ünïcode.example/path)",
        "~~",
        "~~~~",
        "a ~~b~~ c",
        "- [ ]",
        "- [x] あ",
        "|",
        "||",
        "| |",
        "|---|",
        "| a |\n|---|",
        "| 日本 | 🧑‍🤝‍🧑 |\n| :-: | --: |\n| あいうえお | x |",
        "a | b",
    ];
    for input in inputs {
        for width in [0usize, 1, 2, 3, 8, 80, 1000] {
            let _ = render(input, width);
        }
    }
}

#[test]
fn unknown_language_falls_back_without_panic() {
    let lines = render("```brainfuck\n+++.\n```", 40);
    let joined: String = lines.iter().map(line_text).collect();
    assert!(joined.contains("+++."));
}

#[test]
fn code_block_is_highlighted_and_carded() {
    let (theme, ss, st) = fixtures();
    let lines = render_markdown("```rust\nlet x = 1;\n```", 40, &theme, &ss, &st);
    // Padding rows above and below the code, each filled to full width with
    // the card background.
    assert!(lines.len() >= 3);
    for edge in [&lines[0], &lines[lines.len() - 1]] {
        assert!(display_width(&line_text(edge)) == 40, "pad row fills width");
        assert!(
            edge.spans.iter().all(|s| s.style.bg == Some(theme.code_bg)),
            "pad row carries the card background"
        );
    }
    // The content row sits between the pads: card background under every
    // span, and syntect splits it into multiple styled spans.
    let content = &lines[1];
    assert!(line_text(content).contains("let x = 1;"));
    assert!(content.spans.iter().all(|s| s.style.bg == Some(theme.code_bg)));
    assert!(content.spans.len() > 2);
    // The whole card fills the width edge to edge.
    assert_eq!(display_width(&line_text(content)), 40);
}

#[test]
fn inline_code_sits_on_card_background() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans("run `cargo test` now", base, &theme, MarkdownFlavor::Rich);
    let code = spans
        .iter()
        .find(|s| s.content.trim_matches('\u{a0}') == "cargo test")
        .unwrap();
    assert_eq!(code.style.bg, Some(theme.code_bg));
}

#[test]
fn top_level_headings_get_an_underline_rule() {
    // H1/H2 render their text plus a full-width rule; H3+ do not.
    let h1 = render("# Title", 20);
    assert_eq!(h1.len(), 2, "heading + rule");
    assert!(line_text(&h1[1]).chars().all(|c| c == '\u{2500}'));
    assert_eq!(display_width(&line_text(&h1[1])), 20);

    let h3 = render("### Sub", 20);
    assert_eq!(h3.len(), 1, "no rule under H3");
}

#[test]
fn headings_get_a_colour_bar_and_level_colour() {
    let (theme, _, _) = fixtures();
    // The first span of a heading is the solid colour bar; its colour and
    // the heading text's colour track the level.
    for (src, color) in [
        ("# H1", theme.accent),
        ("## H2", theme.info),
        ("### H3", theme.success),
    ] {
        let lines = render(src, 30);
        let bar = &lines[0].spans[0];
        assert_eq!(bar.content.as_ref(), "\u{2503} ");
        assert_eq!(bar.style.fg, Some(color), "bar colour for {src:?}");
        // The text after the bar carries the same level colour, bolded.
        let text = &lines[0].spans[1];
        assert_eq!(text.style.fg, Some(color));
        assert!(text.style.add_modifier.contains(Modifier::BOLD));
    }
}

// ── Transcript flavor (Claude scroll-up view) ──

#[test]
fn transcript_bullets_use_dash_in_body_colour() {
    let (theme, _, _) = fixtures();
    // Bullet marker is "- " (not "• ") and in body colour (not accent).
    let lines = render_transcript("- item", 20);
    let marker = &lines[0].spans[0];
    assert_eq!(marker.content.as_ref(), "- ");
    assert_eq!(marker.style.fg, Some(theme.fg));
    // The Rich flavor still uses the accent bullet, unchanged.
    let rich = render("- item", 20);
    assert_eq!(rich[0].spans[0].content.as_ref(), "\u{2022} ");
    assert_eq!(rich[0].spans[0].style.fg, Some(theme.accent));
}

#[test]
fn transcript_ordered_marker_in_body_colour() {
    let (theme, _, _) = fixtures();
    let lines = render_transcript("1. item", 20);
    let marker = &lines[0].spans[0];
    assert_eq!(marker.content.as_ref(), "1. ");
    assert_eq!(marker.style.fg, Some(theme.fg));
}

#[test]
fn transcript_headings_are_bold_body_colour_no_bar_no_rule() {
    let (theme, _, _) = fixtures();
    // Green H3 (and every other level) render as plain bold body-colour text:
    // first span is the text itself, not a "┃ " bar, and there is no rule.
    for src in ["# H1", "## H2", "### H3"] {
        let lines = render_transcript(src, 30);
        let first = &lines[0].spans[0];
        assert_ne!(first.content.as_ref(), "\u{2503} ", "{src}: no colour bar");
        assert_eq!(first.style.fg, Some(theme.fg), "{src}: body colour");
        assert!(
            first.style.add_modifier.contains(Modifier::BOLD),
            "{src}: bold"
        );
        assert!(
            !lines.iter().any(|l| {
                let t = line_text(l);
                !t.is_empty() && t.chars().all(|c| c == '\u{2500}')
            }),
            "{src}: no underline rule"
        );
    }
}

#[test]
fn transcript_h1_is_bold_italic_underlined_h2_h3_stay_bold_only() {
    // Native Claude Code renders H1 as bold+italic+underline; H2/H3 stay the
    // plain bold established by `transcript_headings_are_bold_body_colour_no_bar_no_rule`.
    let h1 = render_transcript("# Title", 30);
    let span = &h1[0].spans[0];
    assert!(span.style.add_modifier.contains(Modifier::BOLD), "H1 bold");
    assert!(span.style.add_modifier.contains(Modifier::ITALIC), "H1 italic");
    assert!(span.style.add_modifier.contains(Modifier::UNDERLINED), "H1 underlined");

    for src in ["## Sub", "### Subsub"] {
        let lines = render_transcript(src, 30);
        let span = &lines[0].spans[0];
        assert!(!span.style.add_modifier.contains(Modifier::ITALIC), "{src}: not italic");
        assert!(!span.style.add_modifier.contains(Modifier::UNDERLINED), "{src}: not underlined");
    }

    // The Rich flavor's H1 is unaffected (bold only, no italic/underline).
    let rich_h1 = render("# Title", 30);
    let rich_span = &rich_h1[0].spans[1]; // spans[0] is the colour bar
    assert!(!rich_span.style.add_modifier.contains(Modifier::ITALIC), "Rich H1 stays bold-only");
    assert!(!rich_span.style.add_modifier.contains(Modifier::UNDERLINED), "Rich H1 stays bold-only");
}

#[test]
fn transcript_task_checkbox_renders_literally_unstyled() {
    // Native Claude Code doesn't special-case GFM task-list syntax: the
    // checkbox marker stays in the body text, as an ordinary bullet item.
    let (theme, _, _) = fixtures();
    let lines = render_transcript("- [ ] unchecked task\n- [x] checked task", 40);
    assert_eq!(lines[0].spans[0].content.as_ref(), "- ", "ordinary dash bullet, not dropped");
    let unchecked_text: String = lines[0].spans[1..].iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(unchecked_text, "[ ] unchecked task");
    assert_eq!(lines[1].spans[0].content.as_ref(), "- ");
    let checked_text: String = lines[1].spans[1..].iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(checked_text, "[x] checked task");
    // No special colouring: bullet and body both stay body-colour, never
    // theme.success (the "completed" green) or theme.muted (the "recede" grey).
    for line in &lines {
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(theme.fg), "no special checkbox colour");
        }
    }

    // The Rich flavor keeps converting checkboxes (unaffected).
    let rich = render("- [ ] todo\n- [x] done", 40);
    assert_eq!(rich[0].spans[0].content.as_ref(), "[ ] ");
    assert_eq!(rich[1].spans[0].content.as_ref(), "[x] ");
    assert_eq!(rich[1].spans[0].style.fg, Some(theme.success), "Rich still colours [x] green");
}

#[test]
fn transcript_inline_code_uses_info_colour_with_no_padding() {
    let (theme, _, _) = fixtures();
    let lines = render_transcript("use `git` now", 40);
    let code = lines[0]
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "git")
        .expect("inline code span with no NBSP padding");
    assert_eq!(code.style.fg, Some(theme.info));
    assert_eq!(code.style.bg, None, "no card background in the transcript");

    // The Rich flavor keeps its NBSP-padded, code_fg/code_bg card (unaffected).
    let rich = render("use `git` now", 40);
    assert!(
        rich.iter().flat_map(|l| l.spans.iter()).any(|s| {
            s.content.trim_matches('\u{a0}') == "git"
                && s.style.fg == Some(theme.code_fg)
                && s.style.bg == Some(theme.code_bg)
        }),
        "Rich inline code keeps its padded card"
    );
}

#[test]
fn transcript_quote_uses_dim_glyph_and_default_colour_italic_body() {
    let (theme, _, _) = fixtures();
    let lines = render_transcript("> quoted text", 40);
    let glyph = &lines[0].spans[0];
    assert_eq!(glyph.content.as_ref(), "\u{258e} ", "▎ glyph, not │");
    assert!(glyph.style.add_modifier.contains(Modifier::DIM), "glyph is dim");
    assert_eq!(glyph.style.fg, None, "glyph carries no explicit colour");

    let body: String = lines[0].spans[1..].iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(body, "quoted text");
    for span in &lines[0].spans[1..] {
        assert_eq!(span.style.fg, Some(theme.fg), "body is default colour, not muted");
        assert!(span.style.add_modifier.contains(Modifier::ITALIC), "body stays italic");
    }

    // The Rich flavor keeps its muted "│ " bar and muted italic body (unaffected).
    let rich = render("> quoted text", 40);
    assert_eq!(rich[0].spans[0].content.as_ref(), "\u{2502} ");
    assert_eq!(rich[0].spans[0].style.fg, Some(theme.muted));
    assert_eq!(rich[0].spans[1].style.fg, Some(theme.muted));
}

#[test]
fn transcript_heading_has_blank_line_before_and_after() {
    // "body / ## Head / more" → blank inserted both above and below the head.
    let lines = render_transcript("body\n## Head\nmore", 30);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    let h = texts
        .iter()
        .position(|t| t == "Head")
        .expect("heading present");
    assert_eq!(texts[h - 1], "", "blank line above heading");
    assert_eq!(texts[h + 1], "", "blank line below heading");
    assert_eq!(texts[h + 2], "more");
}

#[test]
fn transcript_heading_does_not_stack_double_blank() {
    // An authored blank after the heading is swallowed, not stacked.
    let lines = render_transcript("## Head\n\nbody", 30);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    let h = texts.iter().position(|t| t == "Head").unwrap();
    assert_eq!(texts[h + 1], "", "one blank below");
    assert_eq!(texts[h + 2], "body", "body immediately after the single blank");
}

#[test]
fn transcript_lines_stay_within_width() {
    // The dash bullet and bold headings never overflow the wrap width.
    for width in [4usize, 8, 20, 40] {
        for line in render_transcript("### 見出し\n- あいうえお item\n1. another one", width) {
            assert!(display_width(&line_text(&line)) <= width);
        }
    }
}

#[test]
fn transcript_table_renders_as_boxed_grid() {
    // Matches native Claude Code's default table rendering byte-for-byte:
    // box-drawing border, a rule between every row (not just under the
    // header), columns padded to `max(cell width) + 2`, no colour, no bold.
    let table = "| Column A | Column B | Column C |\n\
        | --- | --- | --- |\n\
        | a1 | b1 | c1 |\n\
        | a2 | b2 | c2 |";
    let lines = render_transcript(table, 100);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(
        texts,
        vec![
            "\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}",
            "\u{2502} Column A \u{2502} Column B \u{2502} Column C \u{2502}",
            "\u{251c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2524}",
            "\u{2502} a1       \u{2502} b1       \u{2502} c1       \u{2502}",
            "\u{251c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2524}",
            "\u{2502} a2       \u{2502} b2       \u{2502} c2       \u{2502}",
            "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}",
        ],
        "boxed table lines"
    );
    // No colour, no bold anywhere in the table.
    for line in &lines {
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(Color::Reset), "table text carries no colour");
            assert_eq!(span.style.bg, None, "table has no background");
            assert!(
                !span.style.add_modifier.contains(Modifier::BOLD),
                "header is not bold in the Transcript flavor"
            );
        }
    }
}

#[test]
fn transcript_table_never_exceeds_width() {
    let table = "| feature | notes |\n| --- | --- |\n\
        | toggle | switches a markdown file between raw source and rendered prose |\n\
        | scroll | independent of the raw view |";
    for width in [1usize, 2, 3, 8, 20, 30, 40, 60, 100] {
        for line in render_transcript(table, width) {
            assert!(
                display_width(&line_text(&line)) <= width.max(1),
                "table line exceeds width {width}"
            );
        }
    }
}

#[test]
fn rich_table_stays_borderless_after_transcript_boxed_table_change() {
    // Guard: the Rich flavor must keep its bold-header / rule-only layout —
    // no box-drawing characters leak in from the Transcript path.
    let table = "| h1 | h2 |\n| --- | --- |\n| a | b |";
    let lines = render(table, 40);
    let joined: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
    for boxed_char in ['\u{250c}', '\u{2510}', '\u{2514}', '\u{2518}', '\u{251c}', '\u{2524}', '\u{252c}', '\u{2534}', '\u{253c}'] {
        assert!(!joined.contains(boxed_char), "Rich table must not use box-drawing borders");
    }
    let (theme, _, _) = fixtures();
    assert_eq!(lines[0].spans[0].style.fg, Some(theme.accent), "Rich header keeps its accent colour");
    assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD), "Rich header stays bold");
}

#[test]
fn apply_background_fills_only_bare_spans() {
    let (theme, ss, st) = fixtures();
    let mut lines =
        render_markdown("text with `code`", 40, &theme, &ss, &st);
    let bg = theme.comment_preview_bg;
    apply_background(&mut lines, bg);
    for line in &lines {
        for span in &line.spans {
            // Plain text gains the tint; the inline-code card keeps its own.
            assert!(span.style.bg == Some(bg) || span.style.bg == Some(theme.code_bg));
        }
    }
}

#[test]
fn markdown_cache_matches_fresh_and_invalidates_on_change() {
    let (theme, ss, st) = fixtures();
    let cache = MarkdownCache::new();
    let texts = |ls: &[Line]| ls.iter().map(line_text).collect::<Vec<_>>();

    // Cached output equals a fresh render.
    let fresh = render_markdown("a `b` c", 30, &theme, &ss, &st);
    let first = cache.render("id1", "a `b` c", 30, &theme, &ss, &st);
    assert_eq!(texts(&fresh), texts(&first));

    // A cache hit (same id/body/width) returns the same content.
    let second = cache.render("id1", "a `b` c", 30, &theme, &ss, &st);
    assert_eq!(texts(&first), texts(&second));

    // Changing the body re-renders (different output under the same id).
    let changed = cache.render("id1", "totally different text", 30, &theme, &ss, &st);
    assert_ne!(texts(&first), texts(&changed));

    // Changing the width re-wraps.
    let narrow = cache.render("id2", "the quick brown fox jumps", 8, &theme, &ss, &st);
    let wide = cache.render("id2", "the quick brown fox jumps", 40, &theme, &ss, &st);
    assert_ne!(narrow.len(), wide.len());
}
