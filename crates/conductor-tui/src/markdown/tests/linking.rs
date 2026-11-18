//! 描いた行に添えるリンクの桁。桁が描いた字とずれると、押した字と違う URL が開く。

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_segmentation::UnicodeSegmentation;

use super::super::wrap::drawn_width;
use super::*;

fn linked(text: &str, width: usize) -> Vec<MdLine> {
    let (theme, ss, st) = fixtures();
    render_linked(text, width, &theme, &ss, &st, Flavor::Transcript)
}

/// url のリンクが付いた桁に描いた字を、行ごとに上から。
fn pieces(lines: &[MdLine], url: &str) -> Vec<String> {
    let mut out = Vec::new();
    for md in lines {
        for run in md.links.iter().filter(|run| &*run.url == url) {
            let mut col = 0;
            let mut piece = String::new();
            for g in md.line.spans.iter().flat_map(|s| s.content.graphemes(true)) {
                if run.cols.contains(&col) {
                    piece.push_str(g);
                }
                col += drawn_width(g);
            }
            out.push(piece);
        }
    }
    out
}

fn urls(lines: &[MdLine]) -> Vec<String> {
    let mut urls: Vec<String> = lines
        .iter()
        .flat_map(|md| md.links.iter().map(|run| run.url.to_string()))
        .collect();
    urls.dedup();
    urls
}

const URL: &str = "https://example.com/a/very/long/path/to/doc";

#[test]
fn 表のセルで折れたurlは折れ目に依らず全区間が同じurlの字になる() {
    let table = format!("| 名前 | 場所 |\n|---|---|\n| doc | {URL} |");
    for width in 16..70 {
        let lines = linked(&table, width);
        let pieces = pieces(&lines, URL);
        assert_eq!(pieces.concat(), URL, "幅 {width}: {pieces:?}");
        assert_eq!(urls(&lines), [URL], "幅 {width}");
    }
}

#[test]
fn 地の文で折れたurlは折れ目に依らず全区間が同じurlの字になる() {
    let text = format!("詳しくは {URL} を見て");
    for width in 8..70 {
        let lines = linked(&text, width);
        assert_eq!(pieces(&lines, URL).concat(), URL, "幅 {width}");
    }
}

#[test]
fn スキームのないlocalhostも表のセルで折れ目に依らず一つのurlになる() {
    let raw = "localhost:4111/app/settings";
    let table = format!("| 名前 | 場所 |\n|---|---|\n| app | {raw} |");
    for width in 14..50 {
        let lines = linked(&table, width);
        assert_eq!(pieces(&lines, raw).concat(), raw, "幅 {width}");
    }
}

#[test]
fn 日本語へそのまま続く素のurlは印にしない() {
    let lines = linked("https://ja.wikipedia.org/wiki/日本 を参照", 80);
    assert_eq!(urls(&lines), Vec::<String>::new());
}

#[test]
fn ラベル付きのリンクはラベルと括弧の中のurlに印を付け括弧には付けない() {
    let url = "https://example.com/doc";
    let lines = linked(&format!("[ドキュメント]({url})"), 80);
    assert_eq!(pieces(&lines, url), ["ドキュメント", url]);
}

#[test]
fn ラベルが別のurlのときは外側のurlだけを付ける() {
    let lines = linked("[https://a.example](https://b.example)", 80);
    assert_eq!(urls(&lines), ["https://b.example"]);
}

#[test]
fn インラインコードとコードブロックの中のurlは印にしない() {
    let text = "`https://a.example/x` を\n\n```\nhttps://b.example/y\n```";
    assert_eq!(urls(&linked(text, 80)), Vec::<String>::new());
}

#[test]
fn 一覧と引用と見出しでは前置きの幅だけずれた桁に付ける() {
    for text in [
        "- https://a.example/list",
        "3. https://a.example/list",
        "> https://a.example/list",
        "# https://a.example/list",
        "  - https://a.example/list",
    ] {
        let lines = linked(text, 80);
        assert_eq!(
            pieces(&lines, "https://a.example/list"),
            ["https://a.example/list"],
            "{text}"
        );
    }
}

#[test]
fn 見出しと区切り線の下でも印は自分の行の字に付く() {
    let lines = linked("# 見出し\n\n---\n\n本文\n\nhttps://a.example/after", 40);
    assert_eq!(
        pieces(&lines, "https://a.example/after"),
        ["https://a.example/after"]
    );
}

#[test]
fn 太字の中の素のurlにも印を付ける() {
    let lines = linked("**https://a.example/bold** を見て", 80);
    assert_eq!(
        pieces(&lines, "https://a.example/bold"),
        ["https://a.example/bold"]
    );
}

#[test]
fn 記号をまたぐ素のurlは描いた字の並びで一つにする() {
    let lines = linked("https://example.com/*/foo* を見て", 80);
    assert_eq!(urls(&lines), ["https://example.com//foo"]);
}

#[test]
fn タブの後ろのurlの桁は描いたセルと一致する() {
    let url = "https://a.example/tab";
    let lines = linked(&format!("名前\t{url}"), 80);
    let md = lines.iter().find(|md| !md.links.is_empty()).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
    terminal
        .draw(|frame| {
            frame.buffer_mut().set_line(0, 0, &md.line, 80);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let drawn: String = md.links[0]
        .cols
        .clone()
        .map(|x| buffer[(x as u16, 0)].symbol().to_string())
        .collect();
    assert_eq!(drawn, url);
}
