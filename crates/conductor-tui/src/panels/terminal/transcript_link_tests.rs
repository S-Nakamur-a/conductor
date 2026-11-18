use conductor_core::claude_log::{DisplayBlock, LogEntry, Role};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::link::{Link, Segment, on_screen};
use crate::workspace::Workspace;

const PANEL: &str = "panel-a";
const AREA: Rect = Rect::new(0, 0, 160, 40);

fn assistant(text: &str) -> LogEntry {
    LogEntry {
        role: Role::Assistant,
        blocks: vec![DisplayBlock::Text(text.into())],
    }
}

fn reading(entries: Vec<LogEntry>, scroll_back: usize) -> Workspace {
    let mut ws = Workspace::for_test();
    let panel = &mut ws.panels.terminal;
    panel.claude.show(Some(PANEL.into()));
    panel.pending_transcript = Some(PendingTranscript {
        panel: PANEL.into(),
        session: "s".into(),
        scroll_back,
    });
    panel.install_transcript("s", Ok(entries));
    assert!(panel.transcript_of(Some(PANEL)).is_some());
    ws
}

/// Claude の区画を幅 width にして窓全体を描き、押せるリンクを読む。
fn links_on(ws: &mut Workspace, width: u16) -> (Vec<Link>, Buffer) {
    let area = Rect::new(0, 0, AREA.width, AREA.height);
    let layout = crate::layout::layout(ws, area);
    let rect = layout.rect(Region::TerminalClaude).unwrap();
    let content = render::content_area(rect);
    ws.panels.terminal.claude.size = (content.height, content.width.min(width));
    let _ = ws.prepare();
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| crate::render::render(frame, ws, &layout))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (on_screen(ws, &layout, &buffer), buffer)
}

fn text(buffer: &Buffer, segment: &Segment) -> String {
    segment
        .xs
        .clone()
        .map(|x| buffer[(x, segment.y)].symbol())
        .collect()
}

fn with_url<'a>(links: &'a [Link], url: &str) -> Vec<&'a Link> {
    links.iter().filter(|l| l.url == url).collect()
}

const URL: &str = "https://example.com/a/very/long/path/to/the/doc";

#[test]
fn トランスクリプトの表のセルで折れたurlは完全なurlの一つのリンクになる() {
    let table = format!("| 名前 | 場所 |\n|---|---|\n| doc | {URL} |");
    let mut ws = reading(vec![assistant(&table)], 0);
    let (links, buffer) = links_on(&mut ws, 40);
    let found = with_url(&links, URL);
    assert_eq!(found.len(), 1, "{links:?}");
    assert!(found[0].segments.len() >= 2, "{found:?}");
    let joined: String = found[0].segments.iter().map(|s| text(&buffer, s)).collect();
    assert_eq!(joined, URL);
    assert!(
        links.iter().all(|l| l.url == URL),
        "途中までの URL が残っている: {links:?}"
    );
}

#[test]
fn トランスクリプトのスキームのないlocalhostは開く先にhttpを付ける() {
    let mut ws = reading(vec![assistant("起動した: localhost:4111/app")], 0);
    let (links, _) = links_on(&mut ws, 60);
    assert_eq!(
        with_url(&links, "http://localhost:4111/app").len(),
        1,
        "{links:?}"
    );
}

#[test]
fn 押して開けないurlと端末への命令を含むurlは包まない() {
    let text = [
        "[相対](./docs/a.md)",
        "[節](#setup)",
        "[メール](mailto:a@b.example)",
        "[題付き](https://x.example \"title\")",
        "[命令](https://x.example/\u{1b}]8;;evil)",
        "[c1](https://x.example/\u{9c}c1)",
    ]
    .join("\n\n");
    let mut ws = reading(vec![assistant(&text)], 0);
    let (links, _) = links_on(&mut ws, 80);
    assert!(
        links.iter().all(|l| l.url.starts_with("https://x.example")
            && !l.url.contains(['\u{1b}', '\u{9c}', ' '])),
        "{links:?}"
    );
}

#[test]
fn 括弧の数が合わないurlは印にせず字の推測に任せる() {
    let url = "https://en.wikipedia.org/wiki/Rust_(lang)";
    let mut ws = reading(vec![assistant(&format!("[Rust]({url})"))], 0);
    ws.panels.terminal.log_urls = Default::default();
    let (links, _) = links_on(&mut ws, 100);
    assert!(
        links
            .iter()
            .all(|l| l.url != "https://en.wikipedia.org/wiki/Rust_(lang"),
        "{links:?}"
    );
}

#[test]
fn ユーザのターンには印を付けない() {
    let mut ws = reading(
        vec![LogEntry {
            role: Role::User,
            blocks: vec![DisplayBlock::Text(format!("これ見て [ラベル]({URL})"))],
        }],
        0,
    );
    let (links, _) = links_on(&mut ws, 120);
    assert!(with_url(&links, URL).is_empty(), "{links:?}");
}

#[test]
fn 別のセッションを映している区画にはトランスクリプトの印を出さない() {
    let table = format!("| 名前 | 場所 |\n|---|---|\n| doc | {URL} |");
    let mut ws = reading(vec![assistant(&table)], 0);
    ws.panels.terminal.claude.show(Some("panel-b".into()));
    let (links, _) = links_on(&mut ws, 40);
    assert!(with_url(&links, URL).is_empty(), "{links:?}");
}

#[test]
fn 遡って一行目が上端の外に出たurlは見えている断片だけを包む() {
    let table = format!("| 名前 | 場所 |\n|---|---|\n| doc | {URL} |");
    let mut entries = vec![assistant(&table)];
    entries.extend((0..60).map(|i| assistant(&format!("行 {i}"))));
    let mut partial = false;
    for back in 0..120 {
        let mut ws = reading(entries.clone(), back);
        let (links, buffer) = links_on(&mut ws, 40);
        for link in with_url(&links, URL) {
            let shown: Vec<String> = link.segments.iter().map(|s| text(&buffer, s)).collect();
            assert!(URL.ends_with(&shown.concat()), "遡り {back}: {shown:?}");
            partial |= shown.concat() != URL;
        }
        assert!(links.iter().all(|l| l.url == URL), "遡り {back}: {links:?}");
    }
    assert!(partial, "上端で切れた形を一度も通っていない");
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn 最新へ戻る札に掛かったurlは札の左だけを包む() {
    let long = format!("{URL}/and/more/and/more/and/more/and/more/and/more");
    let filler: Vec<LogEntry> = (0..30).map(|i| assistant(&format!("行 {i}"))).collect();
    let mut entries = filler.clone();
    entries.push(assistant(&long));
    entries.extend(filler);
    let mut under_badge = false;
    for back in 1..120 {
        let mut ws = reading(entries.clone(), back);
        let (links, buffer) = links_on(&mut ws, 60);
        for link in with_url(&links, &long) {
            for segment in &link.segments {
                let shown = text(&buffer, segment);
                assert!(long.contains(&shown), "遡り {back}: {shown:?}");
                under_badge |= row_text(&buffer, segment.y).contains("(G)");
            }
        }
    }
    assert!(under_badge, "札と同じ行に URL が来る形を一度も通っていない");
}

#[test]
fn モーダルを開いている間はトランスクリプトの印も返さない() {
    let mut ws = reading(vec![assistant(URL)], 0);
    ws.modals
        .push(crate::modal::Modal::Help(crate::modal::help::Help::open(
            ws.focus,
        )));
    let (links, _) = links_on(&mut ws, 80);
    assert_eq!(links, vec![]);
}
