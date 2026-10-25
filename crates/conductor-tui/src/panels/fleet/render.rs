use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use conductor_core::keymap::Action;

use super::{Ask, FleetPanel, Place, Row, RowKey, State, Worktree};
use crate::workspace::{Focus, Workspace};

const UNTITLED: &str = "(まだ何も頼んでいない)";

pub const LIST_TITLE: &str = " Sessions ";

/// 一覧の上に固定する 1 行。状態ごとの本数を、行頭の記号と同じ色で言葉にする。
pub fn summary(fleet: &FleetPanel, theme: &Theme) -> Line<'static> {
    let shown = [
        State::Waiting(Ask::Question),
        State::Working,
        State::Finished,
        State::Idle,
    ];
    let mut counts = [0usize; 4];
    for row in fleet.visual_rows().iter().filter(|r| !r.pinned) {
        let slot = match row.state {
            State::Waiting(_) => 0,
            State::Working => 1,
            State::Finished => 2,
            State::Idle => 3,
            State::Other(_) | State::Unknown => continue,
        };
        counts[slot] += 1;
    }
    let mut spans = Vec::new();
    for (i, (state, count)) in shown.iter().zip(counts).enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(theme.hint)));
        }
        // 待機の記号は hint 色なので、そのままだと 0 本と見分けがつかない。
        let color = match (count, state) {
            (0, _) => theme.hint,
            (_, State::Idle) => theme.fg,
            _ => look(theme, state).1,
        };
        let word = match state {
            State::Waiting(_) => "入力待ち",
            State::Working => "作業中",
            State::Finished => "完了",
            _ => "待機",
        };
        spans.push(Span::styled(
            format!("{word} {count}"),
            Style::default().fg(color),
        ));
    }
    Line::from(spans)
}

/// 一覧の本文の 1 行が何か。描画とクリックの当たり判定が同じ並びを読む。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Other,
    Repo(Option<PathBuf>),
    Worktree(Worktree),
}

/// 一覧の区画の中身。要約行・本文・最下行の矩形と、本文の行とスクロール量。
pub struct Composed {
    pub summary: Option<Rect>,
    pub body: Rect,
    pub footer: Rect,
    pub lines: Vec<Line<'static>>,
    pub slots: Vec<Slot>,
    pub offset: usize,
}

pub fn compose(ws: &Workspace, rect: Rect) -> Option<Composed> {
    let inner = inner(rect);
    if inner.height == 0 {
        return None;
    }
    let fleet = &ws.panels.fleet;
    let theme = &ws.theme;
    let body = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let footer = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    let now = SystemTime::now();
    let focused = ws.focus == Focus::Fleet;
    let (pinned, groups) = fleet.groups();
    let mut lines: Vec<Line> = Vec::new();
    let mut slots: Vec<Slot> = Vec::new();
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
    slots.resize(lines.len(), Slot::Other);
    for group in &groups {
        let smart = group.root.clone().filter(|root| fleet.is_tree_root(root));
        lines.push(repo_header(
            &group.name,
            theme,
            inner.width,
            smart.is_some(),
        ));
        slots.push(Slot::Repo(smart));
        for node in &group.nodes {
            let indent = match &node.worktree {
                Some(worktree) => {
                    let selected = fleet.is_selected_worktree(worktree);
                    if selected {
                        selected_line = lines.len();
                    }
                    lines.push(worktree_line(
                        theme,
                        worktree,
                        inner.width,
                        selected,
                        focused,
                    ));
                    slots.push(Slot::Worktree(worktree.clone()));
                    4
                }
                None => 0,
            };
            for row in &node.rows {
                if fleet.is_selected(row) {
                    selected_line = lines.len();
                }
                lines.push(row_line(
                    theme,
                    row,
                    inner.width,
                    indent,
                    fleet.is_selected(row),
                    focused,
                    now,
                ));
                slots.push(Slot::Other);
            }
        }
    }
    Some(Composed {
        summary: (inner.height >= 2).then_some(Rect { height: 1, ..inner }),
        body,
        footer,
        offset: selected_line.saturating_sub(usize::from(body.height).saturating_sub(2)),
        lines,
        slots,
    })
}

pub fn list(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let Some(composed) = compose(ws, rect) else {
        return;
    };
    if let Some(summary_rect) = composed.summary {
        frame.render_widget(
            Paragraph::new(summary(&ws.panels.fleet, &ws.theme)),
            summary_rect,
        );
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            clip(&hint(ws), usize::from(composed.footer.width)),
            Style::default().fg(ws.theme.hint),
        )),
        composed.footer,
    );
    frame.render_widget(
        Paragraph::new(composed.lines).scroll((composed.offset as u16, 0)),
        composed.body,
    );
}

/// 一覧の最下行。今のフォーカスと選んだ行で効くキーを、keymap の割り当てどおりに出す。
pub fn hint(ws: &Workspace) -> String {
    let context = ws.key_context();
    let chord = |action| crate::render::representative_chord(&ws.keymap, context, action);
    if ws.focus == Focus::FleetSession {
        let back = chord(Action::LeaveTerminal).map(|key| format!("{key} 一覧へ戻る · "));
        return format!("{}← などは Claude に届く", back.unwrap_or_default());
    }
    let items: &[(Action, &str)] = match ws.panels.fleet.selected_row().map(|row| row.place) {
        Some(Place::Mine) => &[
            (Action::Select, "開く"),
            (Action::ExpandOrRight, "返事"),
            (Action::StopSession, "止める"),
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::ExitSubPanel, "閉じる"),
        ],
        Some(Place::OtherConductor | Place::Outside) => &[
            (Action::StopSession, "止める"),
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::ExitSubPanel, "閉じる"),
        ],
        None => &[
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::ExitSubPanel, "閉じる"),
        ],
    };
    items
        .iter()
        .filter_map(|(action, label)| chord(*action).map(|key| format!("{key} {label}")))
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(super) fn stop_question(row: &Row) -> String {
    let title = row.title.as_deref().unwrap_or(UNTITLED);
    format!("「{}」を止める?", clip(title, 30))
}

pub(super) fn stop_detail(row: &Row) -> Vec<String> {
    let place = match row.place {
        Place::Mine => "この窓",
        Place::OtherConductor => "conductor の中",
        Place::Outside => "この窓の外",
    };
    let pid = match row.key {
        RowKey::Pid(pid) => format!(" (pid {pid})"),
        RowKey::Panel(_) => String::new(),
    };
    let mut state = format!("状態: {}", state_text(&row.state));
    if row.state == State::Working {
        state.push_str("。作業の途中で止まります");
    }
    vec![
        format!("場所: {place}{pid}"),
        tail_of(&row.working_dir),
        state,
    ]
}

/// 確認の枠は画面幅で切れ、切れるのはパスの末尾。どの repo のどの worktree かは末尾の 2 つにある。
fn tail_of(dir: &std::path::Path) -> String {
    match dir.parent().and_then(|parent| parent.file_name()) {
        Some(parent) => format!("…/{}/{}", parent.to_string_lossy(), super::dir_name(dir)),
        None => dir.display().to_string(),
    }
}

pub fn detail(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    if let Some(session) = ws.fleet_session() {
        crate::panels::terminal::render::fleet(frame, rect, ws, &session);
        return;
    }
    let inner = inner(rect);
    let theme = &ws.theme;
    if let Some(worktree) = ws.panels.fleet.selected_worktree() {
        let new = crate::render::representative_chord(
            &ws.keymap,
            ws.key_context(),
            Action::NewSessionMenu,
        );
        let mut lines = vec![
            Line::styled(
                worktree.label,
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                worktree.path.display().to_string(),
                Style::default().fg(theme.dir_fg),
            ),
        ];
        if let Some(key) = new {
            lines.push(Line::from(""));
            lines.push(Line::styled(
                format!("{key} でこの worktree に新しいセッション"),
                Style::default().fg(theme.fg),
            ));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }
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

/// 本文の右端 3 桁のボタン。当たり判定は [button_hit] が同じ桁を読む。
pub const NEW_BUTTON: &str = "[+]";
pub const SMART_BUTTON: &str = "[✧]";
const BUTTON_WIDTH: u16 = 3;

pub fn button_hit(composed: &Composed, x: u16, y: u16) -> Option<&Slot> {
    let body = composed.body;
    if !(body.y..body.bottom()).contains(&y) || x + BUTTON_WIDTH < body.right() || x >= body.right()
    {
        return None;
    }
    let slot = composed
        .slots
        .get(composed.offset + usize::from(y - body.y))?;
    match slot {
        Slot::Repo(Some(_)) | Slot::Worktree(_) => Some(slot),
        Slot::Repo(None) | Slot::Other => None,
    }
}

fn repo_header(name: &str, theme: &Theme, width: u16, smart: bool) -> Line<'static> {
    if !smart {
        return header(name, theme.fg, width);
    }
    let name = clip(name, usize::from(width.saturating_sub(BUTTON_WIDTH + 2)));
    let mut line = header(&name, theme.fg, width.saturating_sub(BUTTON_WIDTH + 1));
    line.spans.push(Span::raw(" "));
    line.spans.push(Span::styled(
        SMART_BUTTON,
        Style::default().fg(theme.accent),
    ));
    line
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

fn worktree_line(
    theme: &Theme,
    worktree: &Worktree,
    width: u16,
    selected: bool,
    focused: bool,
) -> Line<'static> {
    let room = usize::from(width.saturating_sub(BUTTON_WIDTH + 3));
    let label = clip(&worktree.label, room);
    let pad = room.saturating_sub(label.width());
    crate::list::row_line(
        vec![
            Span::styled(format!("  {label}"), Style::default().fg(theme.fg)),
            Span::raw(" ".repeat(pad + 1)),
            Span::styled(NEW_BUTTON, Style::default().fg(theme.accent)),
        ],
        theme,
        selected,
        focused,
    )
}

fn row_line(
    theme: &Theme,
    row: &Row,
    width: u16,
    indent: usize,
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
    let room = (width as usize).saturating_sub(indent + 2 + left.width() + right.width() + 2);
    let title = clip(&title, room);
    let pad = room.saturating_sub(title.width());
    let title_fg = if row.title.is_some() {
        theme.fg
    } else {
        theme.hint
    };
    crate::list::row_line(
        vec![
            Span::raw(" ".repeat(indent)),
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
