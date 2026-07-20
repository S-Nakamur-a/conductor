//! Tests for span wrapping, adversarial-input robustness, heading/code-block
//! rendering, the Transcript flavor, and [`MarkdownCache`].

use super::*;

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
    let spans = inline_spans("run `cargo test` now", base, &theme);
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
