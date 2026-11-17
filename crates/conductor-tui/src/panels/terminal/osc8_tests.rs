use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::link::{Link, Painter, Segment, marked_links, merge};
use crate::workspace::Workspace;

const TABLE: &[u8] = include_bytes!("../../link/fixtures/table_cell_url_width50.bin");
const PROSE: &[u8] = include_bytes!("../../link/fixtures/prose_url_width60.bin");
const SAME_URL: &[u8] = include_bytes!("../../link/fixtures/same_url_rows_width60.bin");

struct Shown {
    ws: Workspace,
    id: String,
    size: (u16, u16),
    _dir: tempfile::TempDir,
}

/// 何も書かない子 (cat) を Claude の区画に映し、写しの画面へ直に流す。
fn claude_showing(rows: u16, cols: u16, bytes: &[u8]) -> Shown {
    use conductor_svc::pty::{Launch, Spawn};
    let mut ws = Workspace::for_test();
    let dir = tempfile::tempdir().unwrap();
    let panel = &mut ws.panels.terminal;
    panel.worktree = Some(dir.path().to_path_buf());
    let index = panel
        .pty
        .spawn(Spawn {
            launch: Launch::Shell {
                program: "/bin/cat",
            },
            worktree: "t",
            label: "CC:1",
            working_dir: dir.path(),
            rows,
            cols,
        })
        .unwrap();
    let id = panel.pty.sessions()[index].id.clone();
    panel.claude.show(Some(id.clone()));
    // host の最初の画面が後から届いて上書きしないよう、端末の echo が返るまで待つ。
    panel.pty.write_to_session(index, b"x").unwrap();
    let screen = panel.pty.screen(index).unwrap();
    crate::testing::wait_for(|| render::lock(&screen).screen().contents().contains('x'));
    let shown = Shown {
        ws,
        id,
        size: (rows, cols),
        _dir: dir,
    };
    feed(&shown, &[&b"\x1b[H\x1b[2J"[..], bytes].concat());
    shown
}

fn feed(shown: &Shown, bytes: &[u8]) {
    let panel = &shown.ws.panels.terminal;
    let screen = panel
        .pty
        .screen(panel.index_of(Some(&shown.id)).unwrap())
        .unwrap();
    let mut parser = render::lock(&screen);
    parser.set_size(shown.size.0, shown.size.1);
    parser.process(bytes);
}

/// 内容領域が PTY と同じ大きさになる Claude の区画。
fn claude_rect(shown: &Shown) -> Rect {
    let (rows, cols) = shown.size;
    Rect::new(0, 0, cols + 2, rows + 3)
}

fn draw(shown: &Shown, rect: Rect) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(rect.width, rect.height)).unwrap();
    terminal
        .draw(|frame| render::pane(frame, rect, &shown.ws, Region::TerminalClaude))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn marked(shown: &Shown, rect: Rect, buffer: &Buffer) -> Vec<Link> {
    marked_links(&shown.ws, [(Region::TerminalClaude, Some(rect))], buffer)
}

fn text(buffer: &Buffer, segment: &Segment) -> String {
    segment
        .xs
        .clone()
        .map(|x| buffer[(x, segment.y)].symbol())
        .collect()
}

fn texts(buffer: &Buffer, link: &Link) -> Vec<String> {
    link.segments.iter().map(|s| text(buffer, s)).collect()
}

#[test]
fn 表のセル内で折れたurlは完全なurlの一つのリンクになる() {
    let shown = claude_showing(8, 50, TABLE);
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let links = marked(&shown, rect, &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        links[0].url,
        "https://claude.ai/artifact/Ab12Cd34Ef56Gh78Ij90Kl"
    );
    assert_eq!(
        texts(&buffer, &links[0]),
        ["https://claude.ai/artifact", "/Ab12Cd34Ef56Gh78Ij90Kl"]
    );
}

#[test]
fn claudeが地の文で折ったurlも一つのリンクになる() {
    let shown = claude_showing(6, 60, PROSE);
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let links = marked(&shown, rect, &buffer);
    let url = "https://example.com/a/very/long/path/that/keeps/going/beyond/the/width/of/this/terminal/window?query=1";
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].url, url);
    assert_eq!(texts(&buffer, &links[0]).concat(), url);
}

#[test]
fn 同じurlはどこに出ても一つのリンクにまとめる() {
    let shown = claude_showing(16, 60, SAME_URL);
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let links = marked(&shown, rect, &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        texts(&buffer, &links[0]),
        [
            "https://example.com/same/x",
            "https://example.com/same/x",
            "https://example.com/same/x",
            "https://example.com/same/x",
            "one",
            "two"
        ]
    );
}

fn osc8(url: &str, shown_text: &str) -> String {
    format!("\x1b]8;id=x;{url}\x07{shown_text}\x1b]8;;\x07")
}

#[test]
fn 違うurlが隣り合う行は別のリンクのまま() {
    let a = "https://a.example/one";
    let b = "https://b.example/two";
    let bytes = format!("{}\r\n{}", osc8(a, a), osc8(b, b));
    let shown = claude_showing(4, 40, bytes.as_bytes());
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let urls: Vec<String> = marked(&shown, rect, &buffer)
        .into_iter()
        .map(|l| l.url)
        .collect();
    assert_eq!(urls, [a, b]);
}

#[test]
fn 表のセルのどこで折れても完全なurlの一つのリンクになる() {
    let url = "https://claude.ai/artifact/Ab12Cd34Ef56Gh78Ij90Kl";
    for at in 1..url.len() {
        let (head, tail) = url.split_at(at);
        let bytes = format!(
            "\u{2502} {}\x1b[55G\u{2502}\r\n\u{2502} {})\x1b[55G\u{2502}",
            osc8(url, head),
            osc8(url, tail),
        );
        let shown = claude_showing(4, 60, bytes.as_bytes());
        let rect = claude_rect(&shown);
        let buffer = draw(&shown, rect);
        let links = marked(&shown, rect, &buffer);
        assert_eq!(links.len(), 1, "break at {at}: {links:?}");
        assert_eq!(links[0].url, url, "break at {at}");
        assert_eq!(texts(&buffer, &links[0]), [head, tail], "break at {at}");
    }
}

#[test]
fn 全角のラベルは一つの区間で包む() {
    let url = "https://a.example/label";
    let shown = claude_showing(4, 40, format!("x {}", osc8(url, "ラベル")).as_bytes());
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let links = marked(&shown, rect, &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].segments.len(), 1, "{links:?}");
    assert_eq!(links[0].segments[0].xs.len(), 6);
}

#[test]
fn 描いた後に覆われたセルは包まない() {
    let url = "https://a.example/covered";
    let shown = claude_showing(4, 40, osc8(url, url).as_bytes());
    let rect = claude_rect(&shown);
    let mut buffer = draw(&shown, rect);
    let content = render::content_area(rect);
    buffer[(content.x + 3, content.y)].set_symbol("#");
    let links = marked(&shown, rect, &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    let xs: Vec<_> = links[0].segments.iter().map(|s| s.xs.clone()).collect();
    assert_eq!(
        xs,
        [
            content.x..content.x + 3,
            content.x + 4..content.x + url.len() as u16
        ]
    );
}

#[test]
fn 描いた後にptyが書き換えたセルは包まない() {
    let url = "https://a.example/changed";
    let shown = claude_showing(4, 40, osc8(url, url).as_bytes());
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    feed(
        &shown,
        format!("\x1b[1;1H{}", osc8("https://other.example/zzzzzzz", "XXXX")).as_bytes(),
    );
    let links = marked(&shown, rect, &buffer);
    assert!(links.iter().all(|l| l.url == url), "{links:?}");
    let content = render::content_area(rect);
    assert_eq!(links[0].segments[0].xs.start, content.x + 4);
}

#[test]
fn ptyが区画より広くても見えている桁だけ包む() {
    let url = "https://a.example/a/very/long/path/that/does/not/fit/here";
    let shown = claude_showing(4, 70, osc8(url, url).as_bytes());
    let rect = Rect::new(0, 0, 32, 7);
    let buffer = draw(&shown, rect);
    let links = marked(&shown, rect, &buffer);
    let content = render::content_area(rect);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].url, url);
    assert_eq!(links[0].segments[0].xs, content.x..content.right());
}

#[test]
fn 遡って読んでいる間は帯の重なる最上行を包まない() {
    let hidden = "https://a.example/under-banner";
    let seen = "https://b.example/seen";
    let bytes = format!(
        "{}\r\n{}\r\na\r\nb\r\nc",
        osc8(hidden, hidden),
        osc8(seen, seen)
    );
    let mut shown = claude_showing(4, 40, bytes.as_bytes());
    shown.ws.panels.terminal.claude.scroll = 1;
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let urls: Vec<String> = marked(&shown, rect, &buffer)
        .into_iter()
        .map(|l| l.url)
        .collect();
    assert_eq!(urls, [seen]);
}

#[test]
fn トランスクリプトを開いている間は描いた字と違うセルを包まない() {
    let url = "https://a.example/transcript";
    let mut shown = claude_showing(4, 40, osc8(url, url).as_bytes());
    let id = shown.id.clone();
    let panel = &mut shown.ws.panels.terminal;
    panel.pending_transcript = Some(PendingTranscript {
        panel: id.clone(),
        session: format!("session-of-{id}"),
        scroll_back: 0,
    });
    let turn = conductor_core::claude_log::LogEntry {
        role: conductor_core::claude_log::Role::User,
        blocks: vec![conductor_core::claude_log::DisplayBlock::Text(
            "turn".into(),
        )],
    };
    panel.install_transcript(&format!("session-of-{id}"), Ok(vec![turn]));
    assert!(panel.transcript_of(Some(&id)).is_some());
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    assert_eq!(marked(&shown, rect, &buffer), vec![]);
}

#[test]
fn 一覧の右に映したセッションでも描いた位置で包む() {
    let url = "https://a.example/fleet";
    let mut shown = claude_showing(4, 40, osc8(url, url).as_bytes());
    let mut svc = conductor_svc::Services::new();
    crate::effect::apply(
        &mut shown.ws,
        &mut svc,
        vec![crate::effect::Effect::OpenFleet],
    );
    shown.ws.apply_fleet(
        Default::default(),
        vec![crate::panels::fleet::OwnSession {
            panel_id: shown.id.clone(),
            working_dir: shown._dir.path().to_path_buf(),
            claude_session_id: None,
        }],
    );
    assert_eq!(shown.ws.fleet_session().as_deref(), Some(shown.id.as_str()));
    let rect = Rect::new(0, 0, 42, 6);
    let mut terminal = Terminal::new(TestBackend::new(rect.width, rect.height)).unwrap();
    terminal
        .draw(|frame| render::fleet(frame, rect, &shown.ws, &shown.id))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let links = marked_links(&shown.ws, [(Region::FleetSession, Some(rect))], &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(texts(&buffer, &links[0]), [url]);
    assert_eq!(links[0].segments[0].y, render::pty_area(rect).y);
}

#[test]
fn 何も変わらなければ二度目は書き直さない() {
    let shown = claude_showing(6, 60, SAME_URL);
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let mut painter = Painter::default();
    assert!(
        !painter
            .paint(&buffer, &marked(&shown, rect, &buffer), false)
            .is_empty()
    );
    assert!(
        painter
            .paint(&buffer, &marked(&shown, rect, &buffer), false)
            .is_empty()
    );
}

#[test]
fn 書き戻せない字を含むurlは包まない() {
    let shown = claude_showing(
        4,
        40,
        "\x1b]8;;https://a.example/\u{9c}x\x07text\x1b]8;;\x07".as_bytes(),
    );
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    assert_eq!(marked(&shown, rect, &buffer), vec![]);
}

fn link(url: &str, y: u16, xs: std::ops::Range<u16>) -> Link {
    Link {
        url: url.into(),
        segments: vec![Segment { y, xs }],
    }
}

#[test]
fn 印のリンクと重なる推測は捨て重ならない推測は残す() {
    let marked = vec![link("https://real.example", 1, 5..15)];
    let guessed = vec![
        link("https://shown.example", 1, 10..20),
        link("https://other.example", 2, 5..15),
    ];
    let urls: Vec<String> = merge(marked, guessed).into_iter().map(|l| l.url).collect();
    assert_eq!(urls, ["https://real.example", "https://other.example"]);
}

#[test]
fn 画面全体を通しても印のリンクが包まれる() {
    let url = "https://a.example/whole";
    let shown = claude_showing(6, 40, osc8(url, "https://shown.example/text").as_bytes());
    let area = Rect::new(0, 0, 160, 40);
    let layout = crate::layout::layout(&shown.ws, area);
    assert!(layout.rect(Region::TerminalClaude).is_some());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| crate::render::render(frame, &shown.ws, &layout))
        .unwrap();
    let links = crate::link::on_screen(&shown.ws, &layout, terminal.backend().buffer());
    let urls: Vec<&str> = links.iter().map(|l| l.url.as_str()).collect();
    assert_eq!(urls, [url]);
}

#[test]
fn フォーカスの地を敷いた区画でも印のリンクが包まれる() {
    let url = "https://a.example/focused";
    let mut shown = claude_showing(6, 40, osc8(url, url).as_bytes());
    shown.ws.chrome.terminal_bg = Some(ratatui::style::Color::Rgb(30, 30, 40));
    shown.ws.focus = crate::workspace::Focus::TerminalClaude;
    let area = Rect::new(0, 0, 160, 40);
    let layout = crate::layout::layout(&shown.ws, area);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| crate::render::render(frame, &shown.ws, &layout))
        .unwrap();
    let links = crate::link::on_screen(&shown.ws, &layout, terminal.backend().buffer());
    let marked: Vec<&Link> = links.iter().filter(|l| l.url == url).collect();
    assert_eq!(marked.len(), 1, "{links:?}");
    assert_eq!(marked[0].segments[0].xs.len(), url.len());
}

#[test]
fn 描いた後にptyが同じ字を別の色で書き換えたセルは包まない() {
    let url = "https://a.example/changed";
    let shown = claude_showing(4, 40, osc8(url, url).as_bytes());
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    feed(
        &shown,
        format!("\x1b[1;1H\x1b[31m{}\x1b[m", osc8(url, "h")).as_bytes(),
    );
    let links = marked(&shown, rect, &buffer);
    let content = render::content_area(rect);
    assert_eq!(links[0].segments[0].xs.start, content.x + 1);
}

#[test]
fn 代替画面のリンクは遡りの量に依らず映っている行で読む() {
    let url = "https://a.example/alternate";
    let bytes = format!(
        "a\r\nb\r\nc\r\nd\r\ne\r\n\x1b[?1049h\x1b[H{}",
        osc8(url, url)
    );
    let mut shown = claude_showing(4, 40, bytes.as_bytes());
    shown.ws.panels.terminal.claude.scroll = 3;
    let rect = claude_rect(&shown);
    let buffer = draw(&shown, rect);
    let links = marked(&shown, rect, &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].segments[0].y, render::content_area(rect).y);
}

#[test]
fn シェルの区画でも子が付けたリンクを包む() {
    let url = "https://a.example/shell";
    let mut shown = claude_showing(4, 40, format!("$ {}", osc8(url, "x")).as_bytes());
    let id = shown.id.clone();
    shown.ws.panels.terminal.shell.show(Some(id));
    let (rows, cols) = shown.size;
    let rect = Rect::new(0, 0, cols + 2, rows + 3);
    let mut terminal = Terminal::new(TestBackend::new(rect.width, rect.height)).unwrap();
    terminal
        .draw(|frame| render::pane(frame, rect, &shown.ws, Region::TerminalShell))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let links = marked_links(&shown.ws, [(Region::TerminalShell, Some(rect))], &buffer);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].url, url);
    assert_eq!(texts(&buffer, &links[0]), ["x"]);
}
