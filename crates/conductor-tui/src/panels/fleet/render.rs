use std::time::{Duration, SystemTime};

use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{Ask, FleetPanel, Place, Row, State};
use crate::workspace::{Focus, Workspace};

const UNTITLED: &str = "(まだ何も頼んでいない)";

pub fn list_title(fleet: &FleetPanel) -> String {
    let rows = fleet.groups().1.into_iter().flat_map(|g| g.rows);
    let (mut waiting, mut working, mut finished, mut idle) = (0, 0, 0, 0);
    for row in rows {
        match row.state {
            State::Waiting(_) => waiting += 1,
            State::Working => working += 1,
            State::Finished => finished += 1,
            State::Idle => idle += 1,
            State::Other(_) | State::Unknown => {}
        }
    }
    format!(" Sessions  ! {waiting} · › {working} · ✓ {finished} · z {idle} ")
}

pub fn list(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = inner(rect);
    let fleet = &ws.panels.fleet;
    let theme = &ws.theme;
    let now = SystemTime::now();
    let focused = ws.focus == Focus::Fleet;
    let (pinned, groups) = fleet.groups();
    let mut lines: Vec<Line> = Vec::new();
    let mut selected_line = 0;
    if pinned.is_empty() && groups.is_empty() {
        lines.push(Line::styled(
            "Claude はまだ動いていません。ctrl+n で起動",
            Style::default().fg(theme.fg),
        ));
    }
    if !pinned.is_empty() {
        lines.push(header(
            &format!("入力待ち {}", pinned.len()),
            theme.waiting_primary,
            inner.width,
        ));
        for row in &pinned {
            if fleet.is_selected(row) {
                selected_line = lines.len() + 1;
            }
            lines.extend(pinned_box(
                theme,
                row,
                inner.width,
                fleet.is_selected(row),
                focused,
                now,
            ));
        }
    }
    for group in &groups {
        lines.push(header(
            &format!("{} {}", group.name, group.rows.len()),
            theme.fg,
            inner.width,
        ));
        for row in &group.rows {
            if fleet.is_selected(row) {
                selected_line = lines.len();
            }
            lines.push(row_line(
                theme,
                row,
                inner.width,
                fleet.is_selected(row),
                focused,
                now,
            ));
        }
    }
    let offset = selected_line.saturating_sub(usize::from(inner.height).saturating_sub(2));
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

pub fn detail(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = inner(rect);
    let theme = &ws.theme;
    let Some(row) = ws.panels.fleet.selected_row() else {
        return;
    };
    let (_, color) = look(theme, &row.state);
    let mut lines = vec![
        Line::styled(
            row.title.clone().unwrap_or_else(|| UNTITLED.to_string()),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            row.working_dir.display().to_string(),
            Style::default().fg(theme.dir_fg),
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                state_text(&row.state),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                row.since
                    .map(|since| format!("  {}前から", elapsed(since, SystemTime::now())))
                    .unwrap_or_default(),
                Style::default().fg(theme.hint),
            ),
        ]),
    ];
    if let Some(tokens) = row.context_tokens {
        lines.push(Line::styled(
            format!("文脈 {} トークン", tokens_text(tokens)),
            Style::default().fg(theme.hint),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        match row.place {
            Place::Mine => "Enter でこの worktree の画面に入る".to_string(),
            Place::OtherConductor => format!(
                "この窓の外で動いています ({})。ここからは打てません",
                ws.panels.fleet.group_of(&row)
            ),
            Place::Outside => "外の端末で動いています".to_string(),
        },
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    )
}

fn header(text: &str, color: Color, width: u16) -> Line<'static> {
    let rule = (width as usize).saturating_sub(text.width() + 1);
    Line::from(vec![
        Span::styled(
            text.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("─".repeat(rule), Style::default().fg(color)),
    ])
}

fn look(theme: &Theme, state: &State) -> (&'static str, Color) {
    match state {
        State::Waiting(_) => ("!", theme.waiting_primary),
        State::Working => ("›", theme.accent),
        State::Finished => ("✓", theme.success),
        State::Idle => ("z", theme.hint),
        State::Other(_) | State::Unknown => ("·", theme.hint),
    }
}

fn state_text(state: &State) -> String {
    match state {
        State::Waiting(Ask::Question) => "入力待ち (質問)".into(),
        State::Waiting(Ask::Permission) => "入力待ち (許可)".into(),
        State::Waiting(Ask::Other(raw)) => format!("入力待ち ({raw})"),
        State::Working => "作業中".into(),
        State::Finished => "完了・未確認".into(),
        State::Idle => "待機".into(),
        State::Other(raw) => raw.clone(),
        State::Unknown => "状態不明".into(),
    }
}

fn tag(place: Place) -> &'static str {
    match place {
        Place::Outside => "[外] ",
        Place::Mine | Place::OtherConductor => "",
    }
}

fn row_line(
    theme: &Theme,
    row: &Row,
    width: u16,
    selected: bool,
    focused: bool,
    now: SystemTime,
) -> Line<'static> {
    let (glyph, color) = look(theme, &row.state);
    let edge = if row.place == Place::Outside {
        "╎"
    } else {
        "▌"
    };
    let age = row.since.map(|s| elapsed(s, now)).unwrap_or_default();
    let left = format!(
        " {age}{} {}",
        " ".repeat(6usize.saturating_sub(age.width())),
        tag(row.place)
    );
    let right = row.context_tokens.map(tokens_text).unwrap_or_default();
    let title = row.title.clone().unwrap_or_else(|| UNTITLED.to_string());
    let room = (width as usize).saturating_sub(2 + left.width() + right.width() + 2);
    let title = clip(&title, room);
    let pad = room.saturating_sub(title.width());
    let title_fg = if row.title.is_some() {
        theme.fg
    } else {
        theme.hint
    };
    crate::list::row_line(
        vec![
            Span::styled(
                format!("{edge}{glyph}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(left, Style::default().fg(theme.hint)),
            Span::styled(title, Style::default().fg(title_fg)),
            Span::raw(" ".repeat(pad + 2)),
            Span::styled(right, Style::default().fg(theme.hint)),
        ],
        theme,
        selected,
        focused,
    )
}

fn pinned_box(
    theme: &Theme,
    row: &Row,
    width: u16,
    selected: bool,
    focused: bool,
    now: SystemTime,
) -> Vec<Line<'static>> {
    let color = theme.waiting_primary;
    let border = Style::default().fg(color);
    let inner_w = (width as usize).saturating_sub(2);
    let ask = match &row.state {
        State::Waiting(Ask::Question) => "質問に答えを待っています".to_string(),
        State::Waiting(Ask::Permission) => "許可を待っています".to_string(),
        other => state_text(other),
    };
    let first = format!(
        "! {} {}{}",
        row.since.map(|s| elapsed(s, now)).unwrap_or_default(),
        tag(row.place),
        row.title.clone().unwrap_or_else(|| UNTITLED.to_string()),
    );
    let base = if selected {
        crate::list::selection(theme, focused)
    } else {
        Style::default()
    };
    let body = |text: String, style: Style| {
        let text = clip(&text, inner_w);
        let pad = inner_w.saturating_sub(text.width());
        Line::from(vec![
            Span::styled("│", border),
            Span::styled(format!("{text}{}", " ".repeat(pad)), base.patch(style)),
            Span::styled("│", border),
        ])
    };
    vec![
        Line::styled(format!("┌{}┐", "─".repeat(inner_w)), border),
        body(first, Style::default().fg(theme.fg)),
        body(format!("  {ask}"), Style::default().fg(color)),
        Line::styled(format!("└{}┘", "─".repeat(inner_w)), border),
    ]
}

fn clip(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) + 1 > room {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

fn elapsed(since: SystemTime, now: SystemTime) -> String {
    let secs = now
        .duration_since(since)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    match secs {
        0..60 => format!("{secs}秒"),
        60..3600 => format!("{}分", secs / 60),
        3600..86400 => format!("{}時間", secs / 3600),
        _ => format!("{}日", secs / 86400),
    }
}

fn tokens_text(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else {
        format!("{}k", tokens / 1000)
    }
}
