//! span の折り返し、敵対的入力への堅牢性、見出し/コードブロックの描画、
//! Transcript フレーバー、[MarkdownCache] のテスト。

use super::*;
use ratatui::style::Color;

// 折り返し / 幅

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
    // 全角文字6個 = 12桁。幅10では必ず分割される。
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

// 堅牢性

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
    // コードの上下にパディング行があり、それぞれカードの背景色で
    // 全幅まで埋められている。
    assert!(lines.len() >= 3);
    for edge in [&lines[0], &lines[lines.len() - 1]] {
        assert!(display_width(&line_text(edge)) == 40, "pad row fills width");
        assert!(
            edge.spans.iter().all(|s| s.style.bg == Some(theme.code_bg)),
            "pad row carries the card background"
        );
    }
    // 内容行はパディングの間に位置する: どの span の下にもカードの背景色があり、
    // syntect が複数のスタイル付き span に分割する。
    let content = &lines[1];
    assert!(line_text(content).contains("let x = 1;"));
    assert!(content.spans.iter().all(|s| s.style.bg == Some(theme.code_bg)));
    assert!(content.spans.len() > 2);
    // カード全体が端から端まで幅を埋める。
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
    // H1/H2 はテキストに加えて全幅の区切り線を描画する。H3以降は描画しない。
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
    // 見出しの最初の span は塗りつぶしのカラーバーで、その色と見出しテキストの
    // 色はレベルに応じて変わる。
    for (src, color) in [
        ("# H1", theme.accent),
        ("## H2", theme.info),
        ("### H3", theme.success),
    ] {
        let lines = render(src, 30);
        let bar = &lines[0].spans[0];
        assert_eq!(bar.content.as_ref(), "\u{2503} ");
        assert_eq!(bar.style.fg, Some(color), "bar colour for {src:?}");
        // バーの後ろのテキストは同じレベル色を太字で持つ。
        let text = &lines[0].spans[1];
        assert_eq!(text.style.fg, Some(color));
        assert!(text.style.add_modifier.contains(Modifier::BOLD));
    }
}

// Transcript フレーバー（Claude のスクロールアップ表示）

#[test]
fn transcript_bullets_use_dash_in_body_colour() {
    let (theme, _, _) = fixtures();
    // 箇条書きマーカーは「- 」（「• 」ではない）で、本文色（アクセントではない）。
    let lines = render_transcript("- item", 20);
    let marker = &lines[0].spans[0];
    assert_eq!(marker.content.as_ref(), "- ");
    assert_eq!(marker.style.fg, Some(theme.fg));
    // Rich フレーバーは変わらずアクセント色の箇条書きマーカーを使う。
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
    // H3（他の全レベルも同様）は、太字の本文色プレーンテキストとして描画される:
    // 最初の span は「┃ 」バーではなくテキスト自体で、区切り線もない。
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
    // 実物の Claude Code は H1 を太字+斜体+下線で描画する。H2/H3 は
    // transcript_headings_are_bold_body_colour_no_bar_no_rule で確認した通りの
    // プレーンな太字のまま。
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

    // Rich フレーバーの H1 は影響を受けない（太字のみ、斜体/下線なし）。
    let rich_h1 = render("# Title", 30);
    let rich_span = &rich_h1[0].spans[1]; // spans[0] はカラーバー
    assert!(!rich_span.style.add_modifier.contains(Modifier::ITALIC), "Rich H1 stays bold-only");
    assert!(!rich_span.style.add_modifier.contains(Modifier::UNDERLINED), "Rich H1 stays bold-only");
}

#[test]
fn transcript_task_checkbox_renders_literally_unstyled() {
    // 実物の Claude Code は GFM のタスクリスト構文を特別扱いしない:
    // チェックボックスのマーカーは本文テキストのまま、普通の箇条書き項目として残る。
    let (theme, _, _) = fixtures();
    let lines = render_transcript("- [ ] unchecked task\n- [x] checked task", 40);
    assert_eq!(lines[0].spans[0].content.as_ref(), "- ", "ordinary dash bullet, not dropped");
    let unchecked_text: String = lines[0].spans[1..].iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(unchecked_text, "[ ] unchecked task");
    assert_eq!(lines[1].spans[0].content.as_ref(), "- ");
    let checked_text: String = lines[1].spans[1..].iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(checked_text, "[x] checked task");
    // 特別な色付けはなし: 箇条書きマーカーも本文も本文色のままで、
    // theme.success（「完了」を表す緑）や theme.muted（「後退」を表すグレー）には
    // 決してならない。
    for line in &lines {
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(theme.fg), "no special checkbox colour");
        }
    }

    // Rich フレーバーは引き続きチェックボックスを変換する（影響を受けない）。
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

    // Rich フレーバーは NBSP パディング付きの code_fg/code_bg カードを維持する
    // （影響を受けない）。
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

    // Rich フレーバーは muted 色の「│ 」バーと muted 斜体の本文を維持する
    // （影響を受けない）。
    let rich = render("> quoted text", 40);
    assert_eq!(rich[0].spans[0].content.as_ref(), "\u{2502} ");
    assert_eq!(rich[0].spans[0].style.fg, Some(theme.muted));
    assert_eq!(rich[0].spans[1].style.fg, Some(theme.muted));
}

#[test]
fn transcript_heading_has_blank_line_before_and_after() {
    // "body / ## Head / more" → 見出しの前後どちらにも空行が挿入される。
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
    // ソース側で見出しの後に書かれた空行は吸収され、積み重ならない。
    let lines = render_transcript("## Head\n\nbody", 30);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    let h = texts.iter().position(|t| t == "Head").unwrap();
    assert_eq!(texts[h + 1], "", "one blank below");
    assert_eq!(texts[h + 2], "body", "body immediately after the single blank");
}

#[test]
fn transcript_lines_stay_within_width() {
    // ダッシュの箇条書きと太字見出しは折り返し幅を決してはみ出さない。
    for width in [4usize, 8, 20, 40] {
        for line in render_transcript("### 見出し\n- あいうえお item\n1. another one", width) {
            assert!(display_width(&line_text(&line)) <= width);
        }
    }
}

#[test]
fn transcript_table_renders_as_boxed_grid() {
    // 実物の Claude Code のデフォルトのテーブル描画とバイト単位で一致する:
    // 罫線文字の枠線、（ヘッダー下だけでなく）行と行の間すべての区切り線、
    // max(セル幅) + 2 にパディングされた列、色なし、太字なし。
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
    // テーブルのどこにも色や太字はない。
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
    // ガード: Rich フレーバーは太字ヘッダー/区切り線のみのレイアウトを
    // 維持しなければならない — Transcript 側から罫線文字が漏れてはいけない。
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
            // プレーンテキストは色味を得るが、インラインコードのカードは
            // 自身の色を維持する。
            assert!(span.style.bg == Some(bg) || span.style.bg == Some(theme.code_bg));
        }
    }
}

#[test]
fn markdown_cache_matches_fresh_and_invalidates_on_change() {
    let (theme, ss, st) = fixtures();
    let cache = MarkdownCache::new();
    let texts = |ls: &[Line]| ls.iter().map(line_text).collect::<Vec<_>>();

    // キャッシュされた出力は新規描画と一致する。
    let fresh = render_markdown("a `b` c", 30, &theme, &ss, &st);
    let first = cache.render("id1", "a `b` c", 30, &theme, &ss, &st);
    assert_eq!(texts(&fresh), texts(&first));

    // キャッシュヒット（同じ id/body/width）は同じ内容を返す。
    let second = cache.render("id1", "a `b` c", 30, &theme, &ss, &st);
    assert_eq!(texts(&first), texts(&second));

    // body を変えると再描画される（同じ id でも出力は変わる）。
    let changed = cache.render("id1", "totally different text", 30, &theme, &ss, &st);
    assert_ne!(texts(&first), texts(&changed));

    // width を変えると再度折り返される。
    let narrow = cache.render("id2", "the quick brown fox jumps", 8, &theme, &ss, &st);
    let wide = cache.render("id2", "the quick brown fox jumps", 40, &theme, &ss, &st);
    assert_ne!(narrow.len(), wide.len());
}
