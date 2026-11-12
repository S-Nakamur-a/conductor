use super::*;
use conductor_core::claude_log::urls::extract;
use ratatui::style::{Color, Style};

fn screen(width: u16, lines: &[&str]) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, lines.len() as u16));
    for (y, line) in lines.iter().enumerate() {
        buffer.set_string(0, y as u16, line, Style::default());
    }
    buffer
}

fn logged(text: &str) -> HashSet<String> {
    extract(text).map(str::to_string).collect()
}

fn urls_of(links: &[Link]) -> Vec<&str> {
    links.iter().map(|l| l.url.as_str()).collect()
}

/// 本物の Claude Code 2.1.317 を幅 44 で動かし、vt100 で再生した画面 (ユーザ入力とその答え)。
const CAP7: [&str; 26] = [
    "  A: [the                                   ",
    "  docs](https://example.com/docs/a/very/lon ",
    "  g/path/to/the/reference/page.html)        ",
    "  B: https://example.com/bold/a/very/long/p ",
    "  ath/that/wraps/across/rows                ",
    "  C: https://example.com/code/a/very/long/p ",
    "  ath/that/wraps/across/rows                ",
    "  D: (see https://example.com/paren/a/very/ ",
    "  long/path/that/wraps/rows).               ",
    "  E: https://en.wikipedia.org/wiki/Rust_(pr ",
    "  ogramming_language)_and_some_more_words   ",
    "  F: https://ja.wikipedia.org/wiki/日本語の ",
    "  記事の名前がとても長い場合のページ        ",
    "⏺ A: the docs                               ",
    "  B: https://example.com/bold/a/very/long/pa",
    "  th/that/wraps/across/rows                 ",
    "  C: https://example.com/code/a/very/long/pa",
    "  th/that/wraps/across/rows                 ",
    "  D: (see https://example.com/paren/a/very/l",
    "  ong/path/that/wraps/rows).                ",
    "  E: https://en.wikipedia.org/wiki/Rust_(pro",
    "  gramming_language)_and_some_more_words    ",
    "  F: https://ja.wikipedia.org/wiki/日本語の ",
    "  記事の名前がとても長い場合のページ        ",
    "  G: https://example.com/angle/a/very/long/p",
    "  ath/that/wraps/across/rows                ",
];

const CAP7_LOG: &str =
    "A: [the docs](https://example.com/docs/a/very/long/path/to/the/reference/page.html)
B: **https://example.com/bold/a/very/long/path/that/wraps/across/rows**
C: `https://example.com/code/a/very/long/path/that/wraps/across/rows`
D: (see https://example.com/paren/a/very/long/path/that/wraps/rows).
E: https://en.wikipedia.org/wiki/Rust_(programming_language)_and_some_more_words
F: https://ja.wikipedia.org/wiki/日本語の記事の名前がとても長い場合のページ
G: <https://example.com/angle/a/very/long/path/that/wraps/across/rows>";

#[test]
fn 本物の画面で折り返したurlをユーザ入力も答えも包む() {
    let buffer = screen(44, &CAP7);
    let urls = logged(CAP7_LOG);
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    let a = "https://example.com/docs/a/very/long/path/to/the/reference/page.html";
    let b = "https://example.com/bold/a/very/long/path/that/wraps/across/rows";
    let c = "https://example.com/code/a/very/long/path/that/wraps/across/rows";
    let d = "https://example.com/paren/a/very/long/path/that/wraps/rows";
    let e = "https://en.wikipedia.org/wiki/Rust_(programming_language)_and_some_more_words";
    let g = "https://example.com/angle/a/very/long/path/that/wraps/across/rows";
    assert_eq!(urls_of(&links), vec![a, b, c, d, e, b, c, d, e, g]);
    assert_eq!(
        links[5].segments,
        vec![Segment { y: 14, xs: 5..44 }, Segment { y: 15, xs: 2..27 },]
    );
}

#[test]
fn 番号付きの一覧の行末のurlを次の行の番号と繋がない() {
    let lines = [
        "  1. https://github.com/o/r/issues/1                                  ",
        "  2. https://github.com/o/r/issues/2                                  ",
        "⏺ 1. https://github.com/o/r/issues/1                                  ",
        "  2. https://github.com/o/r/issues/2                                  ",
    ];
    let buffer = screen(70, &lines);
    let numbered: String = (1..=12)
        .map(|n| format!("{n}. https://github.com/o/r/issues/{n}\n"))
        .collect();
    let urls = logged(&numbered);
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    assert_eq!(
        urls_of(&links),
        vec![
            "https://github.com/o/r/issues/1",
            "https://github.com/o/r/issues/2",
            "https://github.com/o/r/issues/1",
            "https://github.com/o/r/issues/2",
        ]
    );
    assert!(links.iter().all(|l| l.segments.len() == 1));
}

#[test]
fn 三行にまたがるurlを包む() {
    let lines = [
        "  horizon. https://example.com/a/very/long  ",
        "  /path/that/should/wrap/across/several/row ",
        "  s/of/the/terminal/index.html              ",
        "⏺ horizon. https://example.com/a/very/long/p",
        "  ath/that/should/wrap/across/several/rows/o",
        "  f/the/terminal/index.html                 ",
    ];
    let buffer = screen(44, &lines);
    let urls = logged(
        "horizon. https://example.com/a/very/long/path/that/should/wrap/across/several/rows/of/the/terminal/index.html 日本語",
    );
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].segments.len(), 3);
    assert_eq!(links[1].segments[2], Segment { y: 5, xs: 2..27 });
}

#[test]
fn ログに無い繋ぎは作らず各行のurlをその行だけで包む() {
    let lines = ["see https://a.example/xx", "yy and https://b.example"];
    let buffer = screen(24, &lines);
    let empty = HashSet::new();
    let one_line = vec!["https://a.example/xx", "https://b.example"];
    assert_eq!(
        urls_of(&find(&buffer, buffer.area, &Joining::Logged(&empty))),
        one_line
    );
    let urls = logged("https://b.example and https://a.example/zz");
    assert_eq!(
        urls_of(&find(&buffer, buffer.area, &Joining::Logged(&urls))),
        one_line
    );
}

#[test]
fn 一行のurlの後ろの括弧と全角の字は包まない() {
    let lines = [
        "(see https://a.ex/x) and https://b.ex/y）です",
        "Rust: https://en.wikipedia.org/wiki/Rust_(a).",
    ];
    let buffer = screen(50, &lines);
    let links = find(&buffer, buffer.area, &Joining::Wrapped(&[]));
    assert_eq!(
        urls_of(&links),
        vec![
            "https://a.ex/x",
            "https://b.ex/y",
            "https://en.wikipedia.org/wiki/Rust_(a)"
        ]
    );
    assert_eq!(links[0].segments, vec![Segment { y: 0, xs: 5..19 }]);
}

#[test]
fn 一行のurlの後ろの角括弧と全角の句読点は包まない() {
    let lines = [
        "[https://a.ex/z] 見て https://b.ex/x。次",
        "公式(https://c.ex/x)を参照 [https://d.ex/y]です",
    ];
    let buffer = screen(50, &lines);
    let want = vec![
        "https://a.ex/z",
        "https://b.ex/x",
        "https://c.ex/x",
        "https://d.ex/y",
    ];
    let links = find(&buffer, buffer.area, &Joining::Wrapped(&[]));
    assert_eq!(urls_of(&links), want);
    let empty = HashSet::new();
    let links = find(&buffer, buffer.area, &Joining::Logged(&empty));
    assert_eq!(urls_of(&links), want);
}

#[test]
fn 日本語へ続く一行のurlは途中まで包まず外の端末に任せる() {
    let lines = [
        "参考 https://ja.wikipedia.org/wiki/日本語 です",
        "https://ja.wikipedia.org/wiki/Category:日本の企業",
        "https://ja.wikipedia.org/wiki/Wikipedia:井戸端",
        "https://ex.com/search?q=x,日本 https://ex.com/v1.日本",
    ];
    let buffer = screen(60, &lines);
    assert!(find(&buffer, buffer.area, &Joining::Wrapped(&[])).is_empty());
}

#[test]
fn 最後の行が右端で終わり次の行が語ならその行までを包む() {
    let lines = [
        "see https://ex.com/aaa",
        "bbbbbbbbbbbbbbbbbbbbbb",
        "next words",
    ];
    let buffer = screen(22, &lines);
    let urls = logged("see https://ex.com/aaabbbbbbbbbbbbbbbbbbbbbb next words");
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    assert_eq!(
        urls_of(&links),
        vec!["https://ex.com/aaabbbbbbbbbbbbbbbbbbbbbb"]
    );
    assert_eq!(links[0].segments.len(), 2);
}

#[test]
fn 一行の二つ目のurlが次の行へ続くときはそれだけを包む() {
    let lines = ["https://a.ex/1 https://b.ex/", "22 tail"];
    let buffer = screen(28, &lines);
    let urls = logged("https://a.ex/1 https://b.ex/22 tail");
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    assert_eq!(urls_of(&links), vec!["https://a.ex/1", "https://b.ex/22"]);
    assert_eq!(
        links[1].segments,
        vec![Segment { y: 0, xs: 15..28 }, Segment { y: 1, xs: 0..2 }]
    );
}

#[test]
fn 閉じ括弧で終わるurlは括弧まで包み文末の句点は外す() {
    let lines = [
        "x https://en.wikipedia.org/wiki/Rust_(pro",
        "gramming_language). Next",
    ];
    let buffer = screen(41, &lines);
    let urls = logged("x https://en.wikipedia.org/wiki/Rust_(programming_language). Next");
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    assert_eq!(
        urls_of(&links),
        vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
    );
    assert_eq!(links[0].segments[1], Segment { y: 1, xs: 0..18 });
}

#[test]
fn 全角の後ろのurlは桁で区間を取る() {
    let lines = ["日本 https://ex.com/aaa", "bb"];
    let buffer = screen(23, &lines);
    let urls = logged("日本 https://ex.com/aaabb");
    let links = find(&buffer, buffer.area, &Joining::Logged(&urls));
    assert_eq!(links[0].segments[0], Segment { y: 0, xs: 5..23 });
}

#[test]
fn シェルは端末が折り返した行だけを繋ぐ() {
    let lines = ["$ curl https://ex.com/a", "aa/b c"];
    let buffer = screen(23, &lines);
    let links = find(&buffer, buffer.area, &Joining::Wrapped(&[0]));
    assert_eq!(urls_of(&links), vec!["https://ex.com/aaa/b"]);
    let unwrapped = find(&buffer, buffer.area, &Joining::Wrapped(&[]));
    assert_eq!(urls_of(&unwrapped), vec!["https://ex.com/a"]);

    let short = screen(24, &lines);
    assert_eq!(
        urls_of(&find(&short, short.area, &Joining::Wrapped(&[0]))),
        vec!["https://ex.com/a"],
        "右端まで届いていない URL は続かない"
    );
}

fn link(url: &str, segments: Vec<Segment>) -> Link {
    Link {
        url: url.into(),
        segments,
    }
}

#[test]
fn リンクはカーソルと折り返しを退避してから包み最後に戻す() {
    let buffer = screen(8, &["https://", "ex.com  "]);
    let links = [link(
        "https://ex.com",
        vec![Segment { y: 0, xs: 0..8 }, Segment { y: 1, xs: 0..6 }],
    )];
    let bytes = String::from_utf8(Painter::default().paint(&buffer, &links, false)).unwrap();
    let open = bytes
        .find("\x1b]8;id=conductor0;https://ex.com\x1b\\")
        .unwrap();
    let close = bytes.find("\x1b]8;;\x1b\\").unwrap();
    assert!(bytes.starts_with("\x1b7\x1b[?7h"));
    assert!(bytes.ends_with("\x1b[?7l\x1b8"));
    assert!(open < close);
    let drawn = &bytes[open + "\x1b]8;id=conductor0;https://ex.com\x1b\\".len()..close];
    assert!(
        drawn.contains("https://") && drawn.contains("ex.com"),
        "{drawn:?}"
    );
}

#[test]
fn 包まなくなった行はリンクの外で描き直す() {
    let buffer = screen(8, &["https://", "ex.com  "]);
    let mut painter = Painter::default();
    let links = [link(
        "https://ex.com",
        vec![Segment { y: 0, xs: 0..8 }, Segment { y: 1, xs: 0..6 }],
    )];
    painter.paint(&buffer, &links, false);

    let next = screen(8, &["next    ", "line    "]);
    let bytes = String::from_utf8(painter.paint(&next, &[], false)).unwrap();
    assert!(!bytes.contains("\x1b]8;id"), "{bytes:?}");
    assert!(
        bytes.contains("next") && bytes.contains("line"),
        "{bytes:?}"
    );
    assert!(
        painter.paint(&next, &[], false).is_empty(),
        "一度直したら次は書かない"
    );
}

#[test]
fn 同じリンクと字なら書き直さず_字が変わるか端末を消したら書き直す() {
    let buffer = screen(8, &["https://", "ex.com  "]);
    let links = [link(
        "https://ex.com",
        vec![Segment { y: 0, xs: 0..8 }, Segment { y: 1, xs: 0..6 }],
    )];
    let mut painter = Painter::default();
    assert!(!painter.paint(&buffer, &links, false).is_empty());
    assert!(painter.paint(&buffer, &links, false).is_empty());

    let mut outside = buffer.clone();
    outside[(7, 1)].set_char('x');
    assert!(
        painter.paint(&outside, &links, false).is_empty(),
        "リンクの外の字は ratatui が書くだけでリンクを壊さない"
    );

    assert!(!painter.paint(&outside, &links, true).is_empty());

    let mut inside = outside.clone();
    inside[(2, 1)].set_char('X');
    assert!(!painter.paint(&inside, &links, false).is_empty());

    let wider = screen(9, &["https://", "ex.com  "]);
    assert!(!painter.paint(&wider, &links, false).is_empty());
}

#[test]
fn 画面が縮んでも前のリンクの行を読みに行かない() {
    let mut painter = Painter::default();
    let tall = screen(8, &["", "", "", "https://", "ex.com"]);
    painter.paint(
        &tall,
        &[link(
            "https://ex.com",
            vec![Segment { y: 3, xs: 0..8 }, Segment { y: 4, xs: 0..6 }],
        )],
        false,
    );
    let small = screen(4, &["ab"]);
    let bytes = painter.paint(&small, &[], false);
    assert!(bytes.starts_with(b"\x1b7"));
}

#[test]
fn 全角の続きのセルは書き直さない() {
    let mut painter = Painter::default();
    let buffer = screen(6, &["日本ab"]);
    painter.paint(
        &buffer,
        &[link("https://x", vec![Segment { y: 0, xs: 4..6 }])],
        false,
    );
    let bytes = String::from_utf8(painter.paint(&buffer, &[], false)).unwrap();
    assert!(!bytes.contains("日 "), "{bytes:?}");
    assert!(!bytes.contains("本 "), "{bytes:?}");
    assert!(bytes.contains("ab"), "{bytes:?}");
}

#[test]
fn 描かないセルは書き直さない() {
    let mut buffer = screen(6, &["abcdef"]);
    buffer[(2, 0)].set_skip(true);
    let mut painter = Painter::default();
    painter.paint(
        &buffer,
        &[link("https://x", vec![Segment { y: 0, xs: 0..2 }])],
        false,
    );
    let bytes = String::from_utf8(painter.paint(&buffer, &[], false)).unwrap();
    assert!(!bytes.contains('c'), "{bytes:?}");
    assert!(bytes.contains("ab") && bytes.contains("def"), "{bytes:?}");
}

#[test]
fn 選択のハイライトは色ごと包む() {
    let mut buffer = screen(8, &["https://", "ex.com  "]);
    buffer.set_style(Rect::new(0, 1, 3, 1), Style::default().bg(Color::Red));
    let bytes = String::from_utf8(Painter::default().paint(
        &buffer,
        &[link(
            "https://ex.com",
            vec![Segment { y: 0, xs: 0..8 }, Segment { y: 1, xs: 0..6 }],
        )],
        false,
    ))
    .unwrap();
    assert!(bytes.contains("48;5;1mex."), "{bytes:?}");
}

fn scan(urls: &[&str]) -> Scan {
    Scan {
        offset: 10,
        urls: urls.iter().map(|u| u.to_string()).collect(),
    }
}

fn target(id: &str) -> Option<(PathBuf, String)> {
    Some((PathBuf::from("/r"), id.into()))
}

#[test]
fn 読み込み中は次を投げない() {
    let mut log = LogUrls::default();
    log.note_fetching();
    assert_eq!(log.wanted(target("a"), Instant::now()), None);
    log.forget_in_flight();
    assert!(log.wanted(target("a"), Instant::now()).is_some());
}

#[test]
fn 読んだ直後は読み直さず間をおいたら続きから読む() {
    let now = Instant::now();
    let mut log = LogUrls::default();
    log.install("a".into(), Some(scan(&["https://x"])), Some("a"), now);
    assert_eq!(log.wanted(target("a"), now + Duration::from_secs(1)), None);
    let (_, _, from) = log.wanted(target("a"), now + REFRESH).unwrap();
    assert_eq!(from.offset, 10);
    let (_, _, other) = log.wanted(target("b"), now).unwrap();
    assert_eq!(other, Scan::default());
}

#[test]
fn 映していない会話の結果は入れない() {
    let now = Instant::now();
    let mut log = LogUrls::default();
    log.install("a".into(), Some(scan(&["https://x"])), Some("a"), now);
    log.install("old".into(), Some(scan(&["https://y"])), Some("a"), now);
    assert!(log.of("a").unwrap().contains("https://x"));
    assert_eq!(log.of("old"), None);
}

#[test]
fn 読めなかったときは同じ会話の前の集合を残す() {
    let now = Instant::now();
    let mut log = LogUrls::default();
    log.install("a".into(), Some(scan(&["https://x"])), Some("a"), now);
    log.install("a".into(), None, Some("a"), now);
    assert!(log.of("a").unwrap().contains("https://x"));
}
