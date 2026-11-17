use crate::Parser;

const URL: &str = "https://a.example/p";

fn parse(rows: u16, cols: u16, input: &[u8]) -> Parser {
    let mut parser = Parser::new(rows, cols, 20);
    parser.process(input);
    parser
}

fn open(url: &str) -> Vec<u8> {
    format!("\x1b]8;id=x;{url}\x1b\\").into_bytes()
}

const CLOSE: &[u8] = b"\x1b]8;;\x1b\\";

fn links(parser: &Parser, row: u16) -> Vec<Option<String>> {
    let (_, cols) = parser.screen().size();
    (0..cols)
        .map(|col| parser.screen().cell_link(row, col).map(str::to_owned))
        .collect()
}

fn linked_cols(parser: &Parser, row: u16) -> Vec<u16> {
    let (_, cols) = parser.screen().size();
    (0..cols)
        .filter(|&col| parser.screen().cell_link(row, col).is_some())
        .collect()
}

#[test]
fn 開いてから閉じるまでの字にだけurlが付く() {
    let parser =
        parse(3, 10, &[b"a", &open(URL)[..], b"bc", CLOSE, b"d"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![1, 2]);
    assert_eq!(parser.screen().cell_link(0, 1), Some(URL));
}

#[test]
fn 付帯の無い形とbel終端も同じに読む() {
    let parser = parse(
        3,
        10,
        format!("\x1b]8;;{URL}\x07ab\x1b]8;;\x07c").as_bytes(),
    );
    assert_eq!(linked_cols(&parser, 0), vec![0, 1]);
    assert_eq!(parser.screen().cell_link(0, 0), Some(URL));
}

#[test]
fn urlの中のセミコロンで割れた分を繋ぎ直す() {
    let url = "https://a.example/?a=1;b=2;c=3";
    let parser = parse(3, 10, &[&open(url)[..], b"x"].concat());
    assert_eq!(parser.screen().cell_link(0, 0), Some(url));
}

#[test]
fn セミコロンでvteの上限に届いたurlはリンクにしない() {
    let url_with = |n: usize| {
        let parts: Vec<String> = (0..=n).map(|i| format!("a{i}")).collect();
        format!("https://a.example/?{}", parts.join(";"))
    };
    let fits = url_with(12);
    let parser = parse(3, 10, &[&open(&fits)[..], b"x"].concat());
    assert_eq!(parser.screen().cell_link(0, 0), Some(fits.as_str()));

    let cut = url_with(13);
    let parser = parse(3, 10, &[&open(&cut)[..], b"x"].concat());
    assert_eq!(parser.screen().cell_link(0, 0), None);
}

#[test]
fn 引数の欠けたosc8はリンクを閉じる() {
    for short in [&b"\x1b]8\x1b\\"[..], b"\x1b]8;\x1b\\"] {
        let parser =
            parse(3, 10, &[&open(URL)[..], b"a", short, b"b"].concat());
        assert_eq!(linked_cols(&parser, 0), vec![0], "{short:?}");
    }
}

#[test]
fn sgrの初期化をまたいでもリンクは続く() {
    let parser =
        parse(3, 10, &[&open(URL)[..], b"\x1b[1ma\x1b[mb", CLOSE].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0, 1]);
}

#[test]
fn 閉じずに改行した次の行の字にも付く() {
    let parser = parse(3, 10, &[&open(URL)[..], b"ab\r\ncd"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0, 1]);
    assert_eq!(linked_cols(&parser, 1), vec![0, 1]);
}

#[test]
fn 消したセルからはリンクが消える() {
    let linked = [&open(URL)[..], b"abcdef", CLOSE].concat();
    let parser = parse(3, 10, &[&linked[..], b"\x1b[1;5H\x1b[K"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0, 1, 2, 3], "EL");
    let parser = parse(3, 10, &[&linked[..], b"\x1b[1;2H\x1b[2X"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0, 3, 4, 5], "ECH");
    let parser = parse(3, 10, &[&linked[..], b"\x1b[2J"].concat());
    assert_eq!(linked_cols(&parser, 0), Vec::<u16>::new(), "ED");
}

#[test]
fn リンク無しの字で上書きするとリンクが消える() {
    let parser =
        parse(3, 10, &[&open(URL)[..], b"abc", CLOSE, b"\rx"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![1, 2]);
}

#[test]
fn 全角字は先頭のセルだけにリンクが付く() {
    let parser =
        parse(3, 10, &[&open(URL)[..], "ラベ".as_bytes(), CLOSE].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0, 2]);
}

#[test]
fn 全角字の左半分を上書きして残る空白にはリンクが付かない() {
    let parser = parse(
        3,
        10,
        &["ラ".as_bytes(), b"\r", &open(URL)[..], b"x", CLOSE].concat(),
    );
    assert_eq!(linked_cols(&parser, 0), vec![0]);
    assert_eq!(parser.screen().cell(0, 1).unwrap().contents(), " ");
}

#[test]
fn 端末の初期化の後の字にはリンクが付かない() {
    let parser = parse(3, 10, &[&open(URL)[..], b"\x1bcab"].concat());
    assert_eq!(linked_cols(&parser, 0), Vec::<u16>::new());
}

#[test]
fn 代替画面を出入りしても開いたリンクは続く() {
    let parser = parse(3, 10, &[&open(URL)[..], b"a\x1b[?1049hb"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0]);
    let parser = parse(
        3,
        10,
        &[&open(URL)[..], b"\x1b[?1049h\x1b[?1049l\r\nc"].concat(),
    );
    assert_eq!(linked_cols(&parser, 1), vec![0]);
}

#[test]
fn scrollbackへ送った行のリンクも遡って引ける() {
    let mut parser = parse(
        2,
        10,
        &[&open(URL)[..], b"a", CLOSE, b"\r\nb\r\nc\r\nd"].concat(),
    );
    assert_eq!(linked_cols(&parser, 0), Vec::<u16>::new());
    parser.set_scrollback(2);
    assert_eq!(linked_cols(&parser, 0), vec![0]);
}

#[test]
fn 字の挿入と削除でリンクは字と一緒に動く() {
    let linked = [b"a", &open(URL)[..], b"bc", CLOSE].concat();
    let parser = parse(3, 10, &[&linked[..], b"\x1b[1;1H\x1b[2@"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![3, 4], "ICH");
    let parser = parse(3, 10, &[&linked[..], b"\x1b[1;1H\x1b[P"].concat());
    assert_eq!(linked_cols(&parser, 0), vec![0, 1], "DCH");
}

#[test]
fn 日本語を含むurlはリンクにする() {
    let url = "https://ja.wikipedia.org/wiki/東京";
    let parser = parse(3, 10, &[&open(url)[..], b"x"].concat());
    assert_eq!(parser.screen().cell_link(0, 0), Some(url));
}

#[test]
fn 外の端末へ書き戻せない字を含むurlはリンクにしない() {
    let bad: [&[u8]; 4] = [
        "https://a.example/\u{9c}x".as_bytes(),
        b"https://a.example/\x7fx",
        b"https://a.example/\xffx",
        b"",
    ];
    for url in bad {
        let input =
            [&open(URL)[..], b"a", b"\x1b]8;;", url, b"\x1b\\b"].concat();
        let parser = parse(3, 10, &input);
        assert_eq!(linked_cols(&parser, 0), vec![0], "{url:?}");
    }
}

#[test]
fn 閉じずに次を開くと後のurlに替わる() {
    let other = "https://b.example/q";
    let parser = parse(
        3,
        10,
        &[&open(URL)[..], b"a", &open(other)[..], b"b"].concat(),
    );
    assert_eq!(
        links(&parser, 0)[..2],
        [Some(URL.into()), Some(other.into())]
    );
}

#[test]
fn 知らないoscは今までどおり読み捨てる() {
    let parser = parse(3, 10, b"\x1b]52;c;aGk=\x1b\\ab");
    assert_eq!(parser.screen().contents(), "ab");
    assert_eq!(linked_cols(&parser, 0), Vec::<u16>::new());
}

#[test]
fn 番号が尽きた後の新しいurlはリンクにしない() {
    let mut parser = Parser::new(3, 10, 0);
    for i in 0..u16::MAX {
        parser.process(&open(&format!("https://a.example/{i}")));
    }
    parser.process(
        &[
            &open("https://a.example/one-more")[..],
            b"a",
            &open("https://a.example/7")[..],
            b"b",
        ]
        .concat(),
    );
    assert_eq!(parser.screen().cell_link(0, 0), None);
    assert_eq!(parser.screen().cell_link(0, 1), Some("https://a.example/7"));
}

#[test]
fn 読みの境目でosc8が割れても同じセルに同じurlが付く() {
    let input = [b"a", &open(URL)[..], b"bc", CLOSE, b"d"].concat();
    let whole = parse(3, 10, &input);
    for at in 0..=input.len() {
        let mut parser = Parser::new(3, 10, 20);
        parser.process(&input[..at]);
        parser.process(&input[at..]);
        assert_eq!(links(&parser, 0), links(&whole, 0), "split at {at}");
    }
}

fn reparse(parser: &Parser) -> Parser {
    let (rows, cols) = parser.screen().size();
    parse(rows, cols, &parser.screen().state_formatted())
}

fn every_cell(parser: &Parser) -> Vec<(String, Option<String>)> {
    let (rows, cols) = parser.screen().size();
    (0..rows)
        .flat_map(|row| (0..cols).map(move |col| (row, col)))
        .map(|(row, col)| {
            let cell = parser.screen().cell(row, col).unwrap();
            (
                cell.contents(),
                parser.screen().cell_link(row, col).map(str::to_owned),
            )
        })
        .collect()
}

#[test]
fn 画面を書き出して流し直すと同じセルに同じurlが付く() {
    let input = [
        &open(URL)[..],
        b"abcdefghij",
        CLOSE,
        b"\x1b[41m   \x1b[m",
        b"\r\n",
        &open("https://b.example/q")[..],
        "ラベル".as_bytes(),
        b"\x1b[44m\x1b[3X\x1b[m",
        CLOSE,
        b"\x1b[10Gz",
    ]
    .concat();
    let parser = parse(3, 10, &input);
    assert_eq!(every_cell(&reparse(&parser)), every_cell(&parser));
}

#[test]
fn 書き出した時点で開いていたリンクは流し直した後の字にも付く() {
    let parser = parse(3, 10, &[&open(URL)[..], b"ab"].concat());
    let mut again = reparse(&parser);
    again.process(b"c");
    assert_eq!(linked_cols(&again, 0), vec![0, 1, 2]);
}

#[test]
fn リンクの無い画面の書き出しにosc8は入らない() {
    let parser = parse(3, 10, b"\x1b[31mab\x1b[m\r\n\x1b[42m  \x1b[K");
    let formatted = parser.screen().state_formatted();
    assert!(!formatted.windows(3).any(|w| w == b"]8;"), "{formatted:?}");
}

#[test]
fn 大きさを変えても残るセルのリンクは残る() {
    let mut parser = parse(3, 10, &[&open(URL)[..], b"ab", CLOSE].concat());
    parser.set_size(3, 6);
    assert_eq!(linked_cols(&parser, 0), vec![0, 1]);
    parser.set_size(5, 10);
    assert_eq!(linked_cols(&parser, 0), vec![0, 1]);
}

#[test]
fn vteの1024バイトに届いて切れたかもしれないurlはリンクにしない() {
    // params は "8" と "id=x" と URL で、vte は `;` を数えない。
    let url_of = |total: usize| {
        let head = "https://a.example/";
        format!("{head}{}", "a".repeat(total - 1 - 4 - head.len()))
    };
    let fits = url_of(1023);
    let parser = parse(3, 10, &[&open(&fits)[..], b"x"].concat());
    assert_eq!(parser.screen().cell_link(0, 0), Some(fits.as_str()));

    for total in [1024, 1138, 2048] {
        let cut = url_of(total);
        let parser = parse(3, 10, &[&open(&cut)[..], b"x"].concat());
        assert_eq!(parser.screen().cell_link(0, 0), None, "{total}");
    }
}
