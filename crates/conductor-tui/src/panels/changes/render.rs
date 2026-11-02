//! Files changed タブの左の一覧と中央の縦並び。描くのは見えている行だけ。

use conductor_core::diff_state::DiffSource;
use conductor_core::keymap::{Action, KeyContext};
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use conductor_core::review_store::ReviewComment;

use super::{ChangesPanel, Row, THREAD_INDENT, Tab, Top};
use crate::panels::explorer::git_changes::render::status_color;
use crate::panels::viewer::Scroll;
use crate::panels::viewer::diff::{SideRow, side_by_side};
use crate::panels::viewer::render::{MARK, side_line, summary_lines, syntax_for, unified_line};
use crate::panels::viewer::thread;
use crate::review::ReviewState;
use crate::workspace::{Focus, Workspace};

pub const LIST_TITLE: &str = " Files ";

pub fn list(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = crate::list::inner(rect);
    if inner.height > 0 {
        frame.render_widget(Paragraph::new(list_lines(ws)), inner);
    }
}

pub fn body(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = crate::list::inner(rect);
    if inner.height > 0 {
        let lines = body_lines(ws, inner.width as usize, inner.height as usize);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

pub fn list_lines(ws: &Workspace) -> Vec<Line<'static>> {
    let panel = &ws.panels.changes;
    let theme = &ws.theme;
    let Some(diff) = panel.diff() else {
        return vec![hint(theme, "loading\u{2026}")];
    };
    let cursor = panel.cursor();
    let focused = ws.focus == Focus::Explorer;
    cursor
        .visible(diff.files.len(), panel.list_viewport())
        .map(|i| {
            let file = &diff.files[i];
            let spans = vec![
                Span::styled(
                    format!(" {} ", file.status.letter()),
                    Style::default().fg(status_color(theme, file.status)),
                ),
                Span::styled(file.path.clone(), Style::default().fg(theme.fg)),
            ];
            crate::list::row_line(spans, theme, i == cursor.selected(), focused)
        })
        .collect()
}

/// 差分全体の失敗と base を解決できなかった理由は、どこまで送っても先頭に残す。
pub fn body_lines(ws: &Workspace, width: usize, height: usize) -> Vec<Line<'static>> {
    let panel = &ws.panels.changes;
    let theme = &ws.theme;
    let Some(diff) = panel.diff() else {
        return vec![hint(theme, "loading\u{2026}")];
    };
    let mut lines: Vec<Line<'static>> = diff
        .error
        .iter()
        .map(|error| Line::styled(format!(" {error}"), Style::default().fg(theme.warning)))
        .collect();
    if diff.files.is_empty() {
        if diff.error.is_none() {
            let base = match &diff.source {
                DiffSource::WorkingTree { base } => base.as_str(),
                DiffSource::Commit { oid } => oid.as_str(),
            };
            lines.push(hint(theme, &format!("No changes against {base}")));
        }
        return lines;
    }
    let top = panel.top();
    if top == Top::default()
        && let Some(summary) = ws.review.summary()
    {
        lines.extend(summary_lines(summary, theme, width));
    }
    for file in top.file..panel.sections().len() {
        let mut drawer = FileDrawer::new(ws, file, width);
        let start = if file == top.file { top.row } else { 0 };
        for row in start..panel.height(file) {
            if lines.len() >= height {
                return lines;
            }
            lines.push(drawer.line(row));
        }
    }
    lines
}

/// 1 ファイル分の行を描く。左右表示の行とスレッドは、描くファイルの分だけ組む。
struct FileDrawer<'a> {
    ws: &'a Workspace,
    file: usize,
    width: usize,
    comments: Vec<&'a ReviewComment>,
    side: Option<Vec<SideRow<'a>>>,
    thread: Option<(usize, Vec<Line<'static>>)>,
}

impl<'a> FileDrawer<'a> {
    fn new(ws: &'a Workspace, file: usize, width: usize) -> Self {
        let panel = &ws.panels.changes;
        let path = panel.diff().map(|d| d.files[file].path.as_str());
        Self {
            ws,
            file,
            width,
            comments: path.map_or_else(Vec::new, |path| ws.review.for_file(path)),
            side: panel
                .side_by_side()
                .then(|| side_by_side(&panel.sections()[file].entries)),
            thread: None,
        }
    }

    fn line(&mut self, row: usize) -> Line<'static> {
        let ws = self.ws;
        let panel = &ws.panels.changes;
        let theme = &ws.theme;
        let section = &panel.sections()[self.file];
        match section.rows[row] {
            Row::Header => header(panel, self.file, &ws.review, theme, self.width),
            Row::Blank => Line::default(),
            Row::Note => hint(theme, "Binary or empty file"),
            Row::Thread { anchor, line } => self.thread_line(anchor, line),
            Row::Body(i) => {
                let mark = if self.comments.is_empty() { 0 } else { MARK };
                let width = self.width.saturating_sub(mark);
                let body = match &self.side {
                    Some(rows) => side_line(
                        &rows[i],
                        theme,
                        section.digits,
                        width as u16,
                        Scroll::default(),
                        &section.highlighted,
                    ),
                    None => {
                        let entry = &section.entries[i];
                        let syntax = syntax_for(&section.highlighted, entry.new_line_no());
                        unified_line(entry, theme, section.digits, width, 0, syntax)
                    }
                };
                if mark == 0 {
                    return body;
                }
                let icons = ws.config.ui.icon_set();
                let line_1 = section.line_of(row).unwrap_or(0);
                let mut spans = vec![thread::marker(&self.comments, line_1, theme, icons)];
                let style = body.style;
                spans.extend(body.spans);
                Line::from(spans).style(style)
            }
        }
    }

    fn thread_line(&mut self, anchor: usize, line: usize) -> Line<'static> {
        if self.thread.as_ref().is_none_or(|(at, _)| *at != anchor) {
            let ws = self.ws;
            let lines = thread::lines(
                &ws.review,
                &self.comments,
                anchor,
                &ws.theme,
                ws.config.ui.icon_set(),
                self.width,
                THREAD_INDENT,
            );
            self.thread = Some((anchor, lines));
        }
        let lines = self.thread.as_ref().map_or(&[][..], |(_, lines)| lines);
        lines.get(line).cloned().unwrap_or_default()
    }
}

fn header(
    panel: &ChangesPanel,
    file: usize,
    review: &ReviewState,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let Some(diff) = panel.diff().and_then(|d| d.files.get(file)) else {
        return Line::default();
    };
    let bold = Modifier::BOLD;
    let viewed = if review.is_viewed(&diff.path) {
        "[x]"
    } else {
        "[ ]"
    };
    let spans = vec![
        Span::styled(format!(" {viewed} "), Style::default().fg(theme.hint)),
        Span::styled(
            format!("{} ", diff.status.letter()),
            Style::default()
                .fg(status_color(theme, diff.status))
                .add_modifier(bold),
        ),
        Span::styled(
            diff.path.clone(),
            Style::default().fg(theme.fg).add_modifier(bold),
        ),
        Span::styled(
            format!("  +{}", diff.added_lines),
            Style::default().fg(theme.diff_add),
        ),
        Span::styled(
            format!(" \u{2212}{}", diff.deleted_lines),
            Style::default().fg(theme.diff_del),
        ),
    ];
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let mut spans = spans;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans).style(Style::default().add_modifier(Modifier::UNDERLINED))
}

fn hint(theme: &Theme, text: &str) -> Line<'static> {
    Line::styled(format!(" {text}"), Style::default().fg(theme.hint))
}

/// 中央の枠の題。Code と Files changed のどちらを出しているかと、切り替えるキー。
pub fn tab_title(ws: &Workspace) -> String {
    let current = ws.center_tab();
    let labels: Vec<String> = tab_labels(ws)
        .into_iter()
        .map(|(tab, label)| match tab == current {
            true => format!("[{label}]"),
            false => format!(" {label} "),
        })
        .collect();
    format!(" {} ", labels.join(" "))
}

/// 題の先頭 (枠の角の次の桁) から数えた位置にあるタブ。
pub fn tab_at(ws: &Workspace, offset: usize) -> Option<Tab> {
    let mut at = 1;
    for (tab, label) in tab_labels(ws) {
        let width = usize::from(crate::strip::width_of(&label)) + 2;
        if (at..at + width).contains(&offset) {
            return Some(tab);
        }
        at += width + 1;
    }
    None
}

fn tab_labels(ws: &Workspace) -> [(Tab, String); 2] {
    let key = |action| {
        crate::render::representative_chord(&ws.keymap, KeyContext::Viewer, action)
            .map_or_else(String::new, |chord| format!("{chord} "))
    };
    let files = match ws.panels.changes.diff() {
        Some(diff) => format!("Files changed {}", diff.files.len()),
        None => "Files changed".to_string(),
    };
    [
        (Tab::Code, format!("{}Code", key(Action::ShowCodeTab))),
        (Tab::Files, format!("{}{files}", key(Action::ShowFilesTab))),
    ]
}
