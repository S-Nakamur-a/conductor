//! 折り返して何行にもまたがる URL を、端末の OSC 8 リンクで包む。外の端末は見た目の
//! 1 行の中でしか URL を探さないので、包まないと 2 行目から先を押しても開かない。
//!
//! ratatui のセルに OSC 8 を入れると幅を誤って数えるので、描き終えた後に、包む区間の
//! セルだけをもう一度書く。

use std::collections::HashSet;
use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use conductor_core::claude_log::urls::{SCHEMES, Scan, is_url_char, trim_url};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::layout::{Layout, Region};
use crate::panels::terminal::render::{claude_shown, link_wrapped_rows};
use crate::workspace::{Focus, Workspace};

/// 会話ログを読み直すまでの間。読むのは伸びた分だけなので短くてよい。
pub const REFRESH: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub y: u16,
    pub xs: Range<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub segments: Vec<Segment>,
}

/// どの行が次の行へ続くかの決め方。
pub enum Joining<'a> {
    /// 端末が自分で折り返した行 (シェル)。画面の y で持つ。
    Wrapped(&'a [u16]),
    /// Claude Code は自分で折り返すので、会話ログにある URL と照らして繋ぐ。
    Logged(&'a HashSet<String>),
}

/// 描き終えたバッファから拾うので、モーダルや帯が覆った字は URL にならない。
pub fn on_screen(ws: &Workspace, layout: &Layout, buffer: &Buffer) -> Vec<Link> {
    if ws.chrome.menu.is_active() || !ws.modals.is_empty() {
        return Vec::new();
    }
    let mut links = Vec::new();
    for region in [Region::TerminalClaude, Region::FleetSession] {
        let Some(rect) = layout.rect(region) else {
            continue;
        };
        let (session, area) = claude_shown(ws, region, rect);
        let panel = &ws.panels.terminal;
        let Some((_, id)) = panel.claude_log_of(session.as_deref()) else {
            continue;
        };
        if let Some(urls) = panel.log_urls.of(&id) {
            links.extend(find(buffer, area, &Joining::Logged(urls)));
        }
    }
    for region in [Region::TerminalShell, Region::FleetChanges] {
        let Some(rect) = layout.rect(region) else {
            continue;
        };
        if let Some((wrapped, area)) = link_wrapped_rows(ws, region, rect) {
            links.extend(find(buffer, area, &Joining::Wrapped(&wrapped)));
        }
    }
    links
}

/// 1 行に収まる URL は端末が自分で拾うので返さない。
pub fn find(buffer: &Buffer, area: Rect, joining: &Joining) -> Vec<Link> {
    let rows: Vec<Vec<(u16, char)>> = (area.top()..area.bottom())
        .map(|y| row(buffer, area, y))
        .collect();
    let mut from = vec![0; rows.len()];
    let mut links = Vec::new();
    for r in 0..rows.len() {
        let mut i = from[r];
        while let Some(start) = url_start(&rows[r], i) {
            let end = run_end(&rows[r], start);
            i = end;
            let Some((link, pieces)) = follow(&rows, (r, start, end), area.top(), joining) else {
                continue;
            };
            links.push(link);
            let &(last, _, last_end) = pieces.last().expect("a link has pieces");
            for &(k, _, _) in &pieces[1..pieces.len() - 1] {
                from[k] = rows[k].len();
            }
            from[last] = last_end;
            break;
        }
    }
    links
}

type Piece = (usize, usize, usize);

fn follow(
    rows: &[Vec<(u16, char)>],
    first: Piece,
    top: u16,
    joining: &Joining,
) -> Option<(Link, Vec<Piece>)> {
    let mut pieces = vec![first];
    let mut text = chars(&rows[first.0][first.1..first.2]);
    let mut best = None;
    loop {
        let (r, _, end) = *pieces.last().expect("never empty");
        let next = r + 1;
        if next >= rows.len() || !reaches_end(&rows[r], end, joining) {
            break;
        }
        let start = match joining {
            Joining::Wrapped(wrapped) if wrapped.contains(&(top + r as u16)) => 0,
            Joining::Wrapped(_) => break,
            Joining::Logged(_) => rows[next]
                .iter()
                .position(|&(_, c)| c != ' ')
                .unwrap_or(rows[next].len()),
        };
        let stop = run_end(&rows[next], start);
        if stop == start || fits_before(&rows[r], end, stop - start, joining) {
            break;
        }
        text += &chars(&rows[next][start..stop]);
        pieces.push((next, start, stop));
        let url = trim_url(&text);
        if accepts(joining, url) {
            best = Some((pieces.len(), url.len()));
        }
        if let Joining::Logged(urls) = joining
            && !urls.iter().any(|u| u.starts_with(&text))
        {
            break;
        }
    }
    let (count, len) = best?;
    pieces.truncate(count);
    let mut left = len;
    for piece in &mut pieces {
        let take = (piece.2 - piece.1).min(left);
        piece.2 = piece.1 + take;
        left -= take;
    }
    let segments = pieces
        .iter()
        .filter(|p| p.2 > p.1)
        .map(|&(r, s, e)| Segment {
            y: top + r as u16,
            xs: rows[r][s].0..rows[r][e - 1].0 + 1,
        })
        .collect();
    let url = pieces
        .iter()
        .map(|&(r, s, e)| chars(&rows[r][s..e]))
        .collect();
    Some((Link { url, segments }, pieces))
}

fn accepts(joining: &Joining, url: &str) -> bool {
    match joining {
        Joining::Wrapped(_) => SCHEMES
            .iter()
            .any(|s| url.starts_with(s) && url.len() > s.len()),
        Joining::Logged(urls) => urls.contains(url),
    }
}

/// 端末の折り返しは右端まで字が詰まっている。Claude Code は右端の 1〜2 桁を空けて
/// 折るので、行の残りが空白なら続きとみなす (誤りはログとの照合が止める)。
fn reaches_end(row: &[(u16, char)], end: usize, joining: &Joining) -> bool {
    match joining {
        Joining::Wrapped(_) => end == row.len(),
        Joining::Logged(_) => row[end..].iter().all(|&(_, c)| c == ' '),
    }
}

/// 次の行の並びが前の行の余りに入るなら、Claude Code はそこへ置いたはずなので続きではない
/// (番号付きの一覧の次の番号など)。2 は区切りの空白と、ユーザ入力で右端に残す 1 桁。
fn fits_before(row: &[(u16, char)], end: usize, len: usize, joining: &Joining) -> bool {
    matches!(joining, Joining::Logged(_)) && row.len() - end >= len + 2
}

fn chars(cells: &[(u16, char)]) -> String {
    cells.iter().map(|&(_, c)| c).collect()
}

fn url_start(row: &[(u16, char)], from: usize) -> Option<usize> {
    (from..row.len()).find(|&i| {
        SCHEMES.iter().any(|s| {
            s.chars()
                .enumerate()
                .all(|(k, c)| row.get(i + k).is_some_and(|&(_, d)| d == c))
        })
    })
}

fn run_end(row: &[(u16, char)], start: usize) -> usize {
    (start..row.len())
        .find(|&i| !is_url_char(row[i].1))
        .unwrap_or(row.len())
}

/// 画面の 1 行の字と、その字が始まる桁。全角の続きのセルは飛ばす。
fn row(buffer: &Buffer, area: Rect, y: u16) -> Vec<(u16, char)> {
    cells(buffer, area.left()..area.right(), y)
        .map(|(x, cell)| {
            let mut chars = cell.symbol().chars();
            let c = match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => char::REPLACEMENT_CHARACTER,
            };
            (x, c)
        })
        .collect()
}

/// 書き直してよいセル。全角の続きを渡すと、端末ではその後ろが 1 桁ずつずれる。
fn cells(buffer: &Buffer, xs: Range<u16>, y: u16) -> impl Iterator<Item = (u16, &Cell)> {
    let mut x = xs.start;
    std::iter::from_fn(move || {
        while x < xs.end {
            let cell = buffer.cell((x, y))?;
            let at = x;
            x += cell.symbol().width().max(1) as u16;
            if !cell.skip {
                return Some((at, cell));
            }
        }
        None
    })
}

/// 前のフレームで包んだ行を覚えておく。ratatui は変わらないセルを書かないので、
/// 包まなくなったセルは端末に古いリンクのまま残る。
#[derive(Debug, Default)]
pub struct Painter {
    linked_rows: Vec<u16>,
}

impl Painter {
    /// 前に包んだ行を素で描き直してから、今のリンクを包んで描くバイト列。
    /// カーソルと筆は ESC 7 / ESC 8 で元へ戻す。書く間だけ自動折り返しを戻すのは、
    /// tmux が折り返しを切った画面では同じ字の上書きを飛ばし、リンクを付け替えないため。
    pub fn paint(&mut self, buffer: &Buffer, links: &[Link]) -> Vec<u8> {
        if self.linked_rows.is_empty() && links.is_empty() {
            return Vec::new();
        }
        let area = buffer.area;
        let mut out = b"\x1b7\x1b[?7h".to_vec();
        for &y in &self.linked_rows {
            draw(&mut out, buffer, area.left()..area.right(), y);
        }
        for (n, link) in links.iter().enumerate() {
            let _ = write!(out, "\x1b]8;id=conductor{n};{}\x1b\\", link.url);
            for seg in &link.segments {
                draw(&mut out, buffer, seg.xs.clone(), seg.y);
            }
            out.extend_from_slice(b"\x1b]8;;\x1b\\");
        }
        out.extend_from_slice(b"\x1b[?7l\x1b8");
        self.linked_rows = links
            .iter()
            .flat_map(|l| l.segments.iter().map(|s| s.y))
            .collect();
        self.linked_rows.sort_unstable();
        self.linked_rows.dedup();
        out
    }
}

fn draw(out: &mut Vec<u8>, buffer: &Buffer, xs: Range<u16>, y: u16) {
    let _ = CrosstermBackend::new(out).draw(cells(buffer, xs, y).map(|(x, c)| (x, y, c)));
}

/// 今映っている Claude の区画の会話 (作業ディレクトリと id)。一覧を開いている間は詳細画面は描かない。
pub fn shown_claude_log(ws: &Workspace) -> Option<(PathBuf, String)> {
    let session = if ws.chrome.fleet_open {
        ws.fleet_session()
    } else {
        ws.panels.terminal.session_in(Focus::TerminalClaude)
    };
    ws.panels.terminal.claude_log_of(session.as_deref())
}

/// 映している Claude の会話ログに出た URL。映すセッションは 1 つなので 1 本だけ持つ。
#[derive(Debug, Default)]
pub struct LogUrls {
    shown: Option<(String, Scan)>,
    fetching: bool,
    landed: Option<Instant>,
}

impl LogUrls {
    /// 投げ中は何も返さない。
    pub fn wanted(
        &self,
        target: Option<(PathBuf, String)>,
        now: Instant,
    ) -> Option<(PathBuf, String, Scan)> {
        let (dir, id) = target?;
        if self.fetching {
            return None;
        }
        let from = match &self.shown {
            Some((shown, scan)) if *shown == id => {
                if self.landed.is_some_and(|at| now < at + REFRESH) {
                    return None;
                }
                scan.clone()
            }
            _ => Scan::default(),
        };
        Some((dir, id, from))
    }

    pub fn note_fetching(&mut self) {
        self.fetching = true;
    }

    /// 結果は repo の切り替えで世代ごと捨てられるので、ガードもそこで下ろす。
    pub fn forget_in_flight(&mut self) {
        self.fetching = false;
    }

    /// 読めなかった (scan が None) ときは、同じ会話の前の集合を残す。
    pub fn install(&mut self, id: String, scan: Option<Scan>, target: Option<&str>, now: Instant) {
        self.fetching = false;
        if target != Some(id.as_str()) {
            return;
        }
        let scan = match (scan, self.shown.take()) {
            (Some(scan), _) => scan,
            (None, Some((shown, kept))) if shown == id => kept,
            (None, _) => Scan::default(),
        };
        self.shown = Some((id, scan));
        self.landed = Some(now);
    }

    pub fn of(&self, id: &str) -> Option<&HashSet<String>> {
        let (shown, scan) = self.shown.as_ref()?;
        (shown == id).then_some(&scan.urls)
    }
}

#[cfg(test)]
mod tests;
