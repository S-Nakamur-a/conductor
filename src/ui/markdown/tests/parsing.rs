//! ブロック解析 (parse_blocks)、インラインの強調/リンク解析
//! (inline_spans)、タスクチェックボックス、GFM テーブルのテスト。

use super::*;

// パース

#[test]
fn 素のテキストは段落のまま() {
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
fn 空白の無いハッシュは見出しにしない() {
    // issue 参照 / C# / #nofilter は段落のままでなければならない。
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
fn 見出しは階層つきで読む() {
    assert_eq!(
        parse_blocks("# Title\n### Sub"),
        vec![
            MdBlock::Heading {
                level: 1,
                text: "Title".to_string()
            },
            MdBlock::Heading {
                level: 3,
                text: "Sub".to_string()
            },
        ]
    );
}

#[test]
fn 箇条書きと番号付きの項目() {
    assert_eq!(
        parse_blocks("- a\n* b\n1. c\n2) d"),
        vec![
            MdBlock::ListItem {
                ordered: None,
                checked: None,
                text: "a".to_string(),
                indent: 0
            },
            MdBlock::ListItem {
                ordered: None,
                checked: None,
                text: "b".to_string(),
                indent: 0
            },
            MdBlock::ListItem {
                ordered: Some("1".to_string()),
                checked: None,
                text: "c".to_string(),
                indent: 0
            },
            MdBlock::ListItem {
                ordered: Some("2".to_string()),
                checked: None,
                text: "d".to_string(),
                indent: 0
            },
        ]
    );
    // マーカーの後にスペースがない → 普通の段落。
    assert_eq!(
        parse_blocks("-5 degrees"),
        vec![MdBlock::Paragraph("-5 degrees".to_string())]
    );
}

#[test]
fn 閉じていないフェンスは末尾まで飲む() {
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
fn 言語指定の無いフェンスとcrlf() {
    let blocks = parse_blocks("```\r\ncode\r\n```\r\n");
    assert_eq!(
        blocks,
        vec![
            MdBlock::CodeBlock {
                lang: None,
                lines: vec!["code".to_string()]
            },
            MdBlock::Blank,
        ]
    );
}

#[test]
fn フェンスの中のmarkdownは解釈しない() {
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
fn 水平線と本文の見分け() {
    assert_eq!(parse_blocks("---"), vec![MdBlock::Rule]);
    assert_eq!(parse_blocks("***"), vec![MdBlock::Rule]);
    assert_eq!(
        parse_blocks("a - b"),
        vec![MdBlock::Paragraph("a - b".to_string())]
    );
}

// インライン

#[test]
fn snake_caseと素のアスタリスクはそのまま() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    // アンダースコアは強調にならない。
    let spans = inline_spans(
        "call set_change_summary here",
        base,
        &theme,
        MarkdownFlavor::Rich,
    );
    assert_eq!(joined(&spans), "call set_change_summary here");
    assert_eq!(spans.len(), 1, "no styled split for snake_case");
    // 2 * 3: 両側にスペースがあるアスタリスクはそのまま文字として扱われる。
    let spans = inline_spans("rate is 2 * 3", base, &theme, MarkdownFlavor::Rich);
    assert_eq!(joined(&spans), "rate is 2 * 3");
    assert_eq!(spans.len(), 1);
}

#[test]
fn 太字と斜体とコードに装飾が付く() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);

    let spans = inline_spans("a **b** c", base, &theme, MarkdownFlavor::Rich);
    assert_eq!(joined(&spans), "a b c");
    assert!(
        spans
            .iter()
            .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::BOLD))
    );

    let spans = inline_spans("a *b* c", base, &theme, MarkdownFlavor::Rich);
    assert!(
        spans
            .iter()
            .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::ITALIC))
    );

    let spans = inline_spans("use `git` now", base, &theme, MarkdownFlavor::Rich);
    // インラインコードは NBSP でパディングされ、カード上のピンクのチップになる。
    // パディング込みの文字列そのものではなく、トリム後の内容で照合する。
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
fn 閉じていないインラインの記号はそのまま() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    for input in [
        "a `b",
        "a *b",
        "a **b",
        "*",
        "`",
        "a ~~b",
        "~~",
        "~",
        "a ~ b",
        "~/foo",
        "a ~~ b ~~ c",
    ] {
        let spans = inline_spans(input, base, &theme, MarkdownFlavor::Rich);
        assert_eq!(joined(&spans), input, "input {input:?} should be literal");
    }
}

#[test]
fn 打ち消し線は装飾と減光が付く() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans("keep ~~drop~~ this", base, &theme, MarkdownFlavor::Rich);
    assert_eq!(joined(&spans), "keep drop this");
    let struck = spans.iter().find(|s| s.content == "drop").unwrap();
    assert!(struck.style.add_modifier.contains(Modifier::CROSSED_OUT));
    assert_eq!(struck.style.fg, Some(theme.muted));
}

#[test]
fn 打ち消し線の中の記法は入れ子にしない() {
    // 太字/斜体と同様、取り消し線もその内容をそのまま出力する（ネストしない）
    // — ただし取り消し線の外側にあるインラインマークアップは引き続き機能する。
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans(
        "~~old **bold**~~ and **real**",
        base,
        &theme,
        MarkdownFlavor::Rich,
    );
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
fn 素のチルダの連なりはフェンスであって打ち消し線ではない() {
    // 行頭の ~~~ はコードフェンスであり、インラインの取り消し線より先に解析される。
    assert!(matches!(
        parse_blocks("~~~\ncode\n~~~").as_slice(),
        [MdBlock::CodeBlock { .. }]
    ));
}

// タスクチェックボックス

#[test]
fn タスクのチェックボックスを読む() {
    assert_eq!(
        parse_blocks("- [ ] todo\n- [x] done\n- [X] also\n1. [ ] num"),
        vec![
            MdBlock::ListItem {
                ordered: None,
                checked: Some(false),
                text: "todo".to_string(),
                indent: 0
            },
            MdBlock::ListItem {
                ordered: None,
                checked: Some(true),
                text: "done".to_string(),
                indent: 0
            },
            MdBlock::ListItem {
                ordered: None,
                checked: Some(true),
                text: "also".to_string(),
                indent: 0
            },
            MdBlock::ListItem {
                ordered: Some("1".to_string()),
                checked: Some(false),
                text: "num".to_string(),
                indent: 0
            },
        ]
    );
}

#[test]
fn チェックボックスでないものは素の項目のまま() {
    // 不正な形式のマーカーはチェックボックスになってはいけない。テキストは
    // そのまま保持される。
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
fn 空のチェックボックスも読める() {
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
fn チェックボックスは幅の中に収まる() {
    // [x]  マーカーより十分に大きい幅では、行が範囲内に収まる。
    // （他のすべてのリスト項目と同様、マーカーより狭い幅は守れない — その
    // 極端なケースは never_panics でカバーする。）
    for width in [8usize, 20, 40] {
        for line in render("- [x] done\n- [ ] あいうえお task", width) {
            assert!(display_width(&line_text(&line)) <= width);
        }
    }
}

// テーブル

#[test]
fn 表は見出しと揃えと行を読む() {
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
fn 縦棒を含む段落は表にしない() {
    // デリミタ行がない → テーブルではない。ソース行は1行も消費されない。
    assert_eq!(
        parse_blocks("a | b\nc | d"),
        vec![
            MdBlock::Paragraph("a | b".to_string()),
            MdBlock::Paragraph("c | d".to_string()),
        ]
    );
    // デリミタなしで EOF に達する、ヘッダーらしき行。
    assert_eq!(
        parse_blocks("| h1 | h2 |"),
        vec![MdBlock::Paragraph("| h1 | h2 |".to_string())]
    );
    // ハイフンが0個のデリミタはデリミタとして扱わない。
    assert_eq!(
        parse_blocks("| a | b |\n| : | : |").len(),
        2,
        "no-dash second line means two paragraphs, not a table"
    );
}

#[test]
fn セルの分割は外側の縦棒を正規化する() {
    assert_eq!(split_table_row("| a | b |"), vec!["a", "b"]);
    assert_eq!(split_table_row("a | b"), vec!["a", "b"]);
    assert_eq!(split_table_row("| a | b"), vec!["a", "b"]);
    assert_eq!(split_table_row("a | b |"), vec!["a", "b"]);
}

#[test]
fn 表は幅に収め必要なら切り詰める() {
    // ヘッダー + 区切り線 + 本体2行 = 4行、すべて width に収まる。
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

/// 切り詰めずに折り返す狙い: 内容が一切失われないこと。幅が広すぎるセルの
/// どの単語も、その列を保持できるだけの幅さえあれば、描画されたテーブルの
/// どこかに必ず現れなければならない。
#[test]
fn 幅の広いセルは失わずに折り返す() {
    let table = "| feature | notes |\n| --- | --- |\n\
        | toggle | switches a markdown file between raw source and rendered prose |\n\
        | scroll | independent of the raw view |";
    let words = [
        "switches",
        "markdown",
        "between",
        "source",
        "rendered",
        "prose",
        "independent",
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

/// 折り返しによって行は高くなるが、列はグリッドのままでなければならない:
/// 行内のどの行も同じ表示幅でなければ、2列目がガタガタになってしまう。
#[test]
fn 折り返しても列は揃ったまま() {
    let table = "| a | b |\n| --- | --- |\n\
        | one two three four five | six |\n\
        | x | seven eight nine ten eleven |";
    for width in [24usize, 36, 50] {
        let lines = render(table, width);
        // レンダラがブロックの前に入れる先頭の空行をスキップする。
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

/// 列より幅の広い、分割不能な1つのトークンでも全文が現れなければならない
/// — 途中で切るのではなく、複数行にハード分割する。
#[test]
fn 分割不能な長いセルは切らずに割る() {
    let url = "https://example.com/a/very/long/path/that/never/breaks";
    let table = format!("| link |\n| --- |\n| {url} |");
    let text: String = render(&table, 24)
        .iter()
        .map(|l| line_text(l))
        .collect::<Vec<_>>()
        .join("");
    // 複数行を（パディングを除いて）つなぎ合わせると、URL 全体が存在する。
    let joined: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(joined.contains(url), "URL was cut: {text:?}");
}

#[test]
fn セルの切り詰めはマルチバイトを割らない() {
    // CJK / アクセント付き文字のセルを、その内容幅より狭い幅に押し込める
    // — panic せず、幅の上限を守らなければならない。
    let table = "| name |\n| ---- |\n| café |\n| 日本語テスト |\n| 🧑‍🤝‍🧑x |";
    for width in [1usize, 2, 3, 4, 5, 6, 10] {
        for line in render(table, width) {
            assert!(display_width(&line_text(&line)) <= width.max(1));
        }
    }
}

#[test]
fn 揃え方を変えてもセルの幅は変わらない() {
    // どのアライメントでも同じ内容なら同じ列幅になる。
    let mk = |delim: &str| render(&format!("| h |\n| {delim} |\n| ab |"), 20);
    let widths: Vec<usize> = ["---", ":--", "--:", ":-:"]
        .iter()
        .map(|d| line_text(&mk(d)[2]).trim_end().chars().count())
        .collect();
    // 左寄せ/右寄せ/中央寄せでパディングは異なるが、トリム後の本体内容は "ab"。
    for d in ["---", ":--", "--:", ":-:"] {
        let body = line_text(&mk(d)[2]);
        assert!(body.contains("ab"), "alignment {d} lost content");
    }
    // （トリムしていない）行全体の幅は、どのアライメントでも同じ。
    let full: Vec<usize> = ["---", ":--", "--:", ":-:"]
        .iter()
        .map(|d| display_width(&line_text(&mk(d)[2])))
        .collect();
    assert!(
        full.iter().all(|&w| w == full[0]),
        "row widths differ: {full:?}"
    );
    let _ = widths;
}

#[test]
fn 列数の揃わない行は正規化する() {
    // 短い行も長い行も panic せずに描画され、ヘッダーに合わせてパディング/
    // 切り詰めされる。
    let table = "| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |";
    let lines = render(table, 40);
    // ヘッダー + 区切り線 + 2行。
    assert_eq!(lines.len(), 4);
}

#[test]
fn リンクは本文とurlの両方を出す() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);

    // リンクテキストを表示し、URL は控えめな括弧書きで保持する。
    let spans = inline_spans(
        "see [the docs](https://example.com) now",
        base,
        &theme,
        MarkdownFlavor::Rich,
    );
    assert_eq!(joined(&spans), "see the docs (https://example.com) now");
    assert!(
        spans.iter().any(|s| s.content == "the docs"
            && s.style.add_modifier.contains(Modifier::UNDERLINED)),
        "link text is underlined"
    );

    // リンクテキスト内のインラインマークアップにも引き続きスタイルが付く。
    let spans = inline_spans(
        "[**bold** link](https://x.io)",
        base,
        &theme,
        MarkdownFlavor::Rich,
    );
    assert!(
        spans
            .iter()
            .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)),
        "emphasis inside link text is preserved"
    );
    assert!(joined(&spans).contains("(https://x.io)"));
}

#[test]
fn 本文がurlと同じか空ならurlは一度だけ() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);

    let spans = inline_spans(
        "[https://x.com](https://x.com)",
        base,
        &theme,
        MarkdownFlavor::Rich,
    );
    assert_eq!(joined(&spans), "https://x.com");

    // 末尾のスラッシュや大文字小文字の違いがあっても1つにまとまる。
    let spans = inline_spans(
        "[https://x.com/](https://x.com)",
        base,
        &theme,
        MarkdownFlavor::Rich,
    );
    assert_eq!(joined(&spans), "https://x.com");

    let spans = inline_spans("[](https://x.com)", base, &theme, MarkdownFlavor::Rich);
    assert_eq!(joined(&spans), "https://x.com");
}

#[test]
fn 壊れたリンクはそのまま() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    for input in ["[text]", "[text](", "[text(url)", "a [b] c", "["] {
        let spans = inline_spans(input, base, &theme, MarkdownFlavor::Rich);
        assert_eq!(joined(&spans), input, "{input:?} should stay literal");
    }
}

#[test]
fn リンクの後ろの本文は残る() {
    let (theme, _, _) = fixtures();
    let base = Style::default().fg(theme.fg);
    let spans = inline_spans("[a](b)c", base, &theme, MarkdownFlavor::Rich);
    assert_eq!(joined(&spans), "a (b)c");
}

fn joined(spans: &[Span]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}
