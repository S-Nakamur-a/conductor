use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use conductor_core::cc_hook::statusline::{Limit, Usage};
use conductor_core::config::ClaudeAccount;
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use conductor_core::icons::{self, IconSet};
use conductor_core::keymap::Action;
use conductor_core::revidere_records::{Mark, Review};

use super::{Ask, FleetPanel, Inspection, Lower, Place, Row, RowKey, State, Worktree};
use crate::effect::Effect;
use crate::modal::Confirm;
use crate::panels::explorer::git_changes;
use crate::review::ReviewState;
use crate::workspace::{Focus, Workspace};

const UNTITLED: &str = "(まだ何も頼んでいない)";

pub const LIST_TITLE: &str = " Sessions ";

/// 一覧の上に固定する要約。右端に 5h / 7d を置き、幅が足りなければ残り時間、バー、0 本の状態項目の
/// 順に落とす。それでも 1 行に入らなければ使用量を 2 行目に回し (1 行目に全状態が入らなければ 0 本の
/// 項目を落とす)、2 行目にも入らなければ、アカウントの設定があるときだけ名前と 5h の率を 0 本でない
/// 状態の横に出す。それも入らなければ使用量を落とす。全部 0 本なら状態項目は省かない — 省くと何の数か
/// 分からなくなる。
pub fn summary_lines(
    fleet: &FleetPanel,
    theme: &Theme,
    accounts: &[ClaudeAccount],
    now: SystemTime,
    width: usize,
) -> Vec<Line<'static>> {
    let all = state_spans(fleet, theme, false);
    let nonzero = state_spans(fleet, theme, true);
    let usage = usage_options(fleet.usage(), accounts, theme, now, &WITH_WEEK);
    let mut candidates: Vec<(&[Span], &[Span])> = usage
        .iter()
        .map(|u| (all.as_slice(), u.as_slice()))
        .collect();
    if let Some(narrowest) = usage.last()
        && !nonzero.is_empty()
    {
        candidates.push((nonzero.as_slice(), narrowest.as_slice()));
    }
    if let Some((states, right)) = candidates
        .into_iter()
        .find(|(states, right)| spans_width(states) + 1 + spans_width(right) <= width)
    {
        return vec![spread(states, right, width)];
    }
    if let Some(right) = usage.iter().find(|u| spans_width(u) <= width) {
        let states = if spans_width(&all) <= width || nonzero.is_empty() {
            &all
        } else {
            &nonzero
        };
        return vec![spread(states, &[], width), spread(&[], right, width)];
    }
    if !accounts.is_empty() && !nonzero.is_empty() {
        let first = usage_options(fleet.usage(), accounts, theme, now, &[Form::FirstLimit]);
        if let Some(right) = first.first()
            && spans_width(&nonzero) + 1 + spans_width(right) <= width
        {
            return vec![spread(&nonzero, right, width)];
        }
    }
    vec![spread(&all, &[], width)]
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn spread(left: &[Span<'static>], right: &[Span<'static>], width: usize) -> Line<'static> {
    let pad = width.saturating_sub(spans_width(left) + spans_width(right));
    let mut spans = left.to_vec();
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend_from_slice(right);
    Line::from(spans)
}

/// 状態ごとの本数を、行頭の記号と同じ色で言葉にする。
fn state_spans(fleet: &FleetPanel, theme: &Theme, hide_zero: bool) -> Vec<Span<'static>> {
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
    for (state, count) in shown.iter().zip(counts) {
        if hide_zero && count == 0 {
            continue;
        }
        if !spans.is_empty() {
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
    spans
}

/// 一覧の本文の 1 行が何か。描画とクリックの当たり判定が同じ並びを読む。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Other,
    Repo(Option<PathBuf>),
    Worktree(Worktree),
    Session(Row),
}

/// 一覧の区画の中身。要約行・本文・最下行の矩形と、本文の行とスクロール量。
pub struct Composed {
    pub summary: Option<(Rect, Vec<Line<'static>>)>,
    pub body: Rect,
    pub footer: Rect,
    pub lines: Vec<Line<'static>>,
    pub slots: Vec<Slot>,
    pub offset: usize,
    /// 入力待ちの欄の後ろ、木が始まる行。
    pub tree_start: usize,
}

impl Composed {
    pub fn max_offset(&self) -> usize {
        max_offset(self.lines.len(), self.body)
    }
}

fn max_offset(lines: usize, body: Rect) -> usize {
    lines.saturating_sub(usize::from(body.height))
}

pub fn compose(ws: &Workspace, rect: Rect) -> Option<Composed> {
    let inner = inner(rect);
    if inner.height == 0 {
        return None;
    }
    let fleet = &ws.panels.fleet;
    let theme = &ws.theme;
    let now = SystemTime::now();
    let mut summary = if inner.height >= 2 {
        summary_lines(
            fleet,
            theme,
            &ws.config.claude.accounts,
            now,
            usize::from(inner.width),
        )
    } else {
        Vec::new()
    };
    summary.truncate(usize::from(inner.height - 1));
    let summary_rows = summary.len() as u16;
    let body = Rect {
        y: inner.y + summary_rows,
        height: inner.height.saturating_sub(summary_rows + 1),
        ..inner
    };
    let footer = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
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
        slots.push(Slot::Other);
        for row in &pinned {
            if fleet.is_selected(row) {
                selected_line = lines.len() + 1;
            }
            lines.extend(pinned_box(
                theme,
                row,
                account(ws, row),
                inner.width,
                fleet.is_selected(row),
                focused,
                now,
            ));
            slots.resize(lines.len(), Slot::Session(row.clone()));
        }
    }
    slots.resize(lines.len(), Slot::Other);
    let tree_start = lines.len();
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
                        ws.config.ui.icon_set(),
                        worktree,
                        fleet.review_of(&worktree.path).map(|r: &Review| r.mark),
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
                    ws,
                    row,
                    inner.width,
                    indent,
                    fleet.is_selected(row),
                    focused,
                    now,
                ));
                slots.push(Slot::Session(row.clone()));
            }
        }
    }
    let max = max_offset(lines.len(), body);
    let offset = match fleet.wheel_top() {
        Some(top) => tree_start.saturating_add_signed(top).min(max),
        None => selected_line
            .saturating_sub(usize::from(body.height).saturating_sub(2))
            .min(max),
    };
    Some(Composed {
        summary: (!summary.is_empty()).then_some((
            Rect {
                height: summary_rows,
                ..inner
            },
            summary,
        )),
        body,
        footer,
        offset,
        tree_start,
        lines,
        slots,
    })
}

pub fn list(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let Some(composed) = compose(ws, rect) else {
        return;
    };
    if let Some((rect, lines)) = composed.summary {
        frame.render_widget(Paragraph::new(lines), rect);
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            clip(&hint(ws), usize::from(composed.footer.width)),
            Style::default().fg(ws.theme.hint),
        )),
        composed.footer,
    );
    crate::render::border_scrollbar(
        frame,
        rect,
        composed.body,
        composed.lines.len(),
        composed.offset,
    );
    frame.render_widget(
        Paragraph::new(composed.lines).scroll((composed.offset as u16, 0)),
        composed.body,
    );
}

/// 右の区画の枠の題。右に入っていればその旨と戻るキー、この窓のセッションを映していれば入るキー、ほかは空。
pub fn session_title(ws: &Workspace) -> String {
    let context = ws.key_context();
    let chord = |action| crate::render::representative_chord(&ws.keymap, context, action);
    let account = ws
        .panels
        .fleet
        .selected_row()
        .and_then(|row| account(ws, &row))
        .map(|(name, _)| format!(" {name} ·"))
        .unwrap_or_default();
    if ws.focus == Focus::FleetSession {
        return match chord(Action::LeaveTerminal) {
            Some(key) => format!("{account} 入力中 · {key} で一覧へ "),
            None => format!("{account} 入力中 "),
        };
    }
    match (ws.fleet_session(), chord(Action::ExpandOrRight)) {
        (Some(_), Some(key)) => format!("{account} {key} で入る "),
        _ => String::new(),
    }
}

/// 一覧の下の区画に何かを出すキー。区画が無い窓では押しても何も出ないので案内しない。
pub fn needs_lower(action: Action) -> bool {
    matches!(
        action,
        Action::NewShell | Action::ShowFleetChanges | Action::ShowFleetArtifacts
    )
}

/// 一覧の最下行。今のフォーカスと選んだ行で効くキーを、keymap の割り当てどおりに出す。
pub fn hint(ws: &Workspace) -> String {
    let context = ws.key_context();
    let chord = |action| crate::render::representative_chord(&ws.keymap, context, action);
    if ws.focus == Focus::FleetSession {
        let back = chord(Action::LeaveTerminal).map(|key| format!("{key} 一覧へ戻る · "));
        return format!("{}← などは Claude に届く", back.unwrap_or_default());
    }
    if ws.focus == Focus::FleetShell {
        let back = chord(Action::LeaveTerminal).map(|key| format!("{key} 一覧へ戻る · "));
        let changes =
            chord(Action::OpenFileFromTerminal).map(|key| format!("{key} Git Changes · "));
        return format!(
            "{}{}ほかのキーはシェルに届く",
            back.unwrap_or_default(),
            changes.unwrap_or_default()
        );
    }
    let fleet = &ws.panels.fleet;
    let items: &[(Action, &str)] = match fleet.selected_row().map(|row| row.place) {
        Some(Place::Mine) => &[
            (Action::Select, "開く"),
            (Action::ExpandOrRight, "返事"),
            (Action::StopSession, "止める"),
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::NewShell, "シェル"),
            (Action::ExitSubPanel, "閉じる"),
            (Action::OpenBranchPage, "PR を開く"),
        ],
        Some(Place::OtherConductor | Place::Outside) => &[
            (Action::StopSession, "止める"),
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::ExitSubPanel, "閉じる"),
            (Action::OpenBranchPage, "PR を開く"),
        ],
        None if fleet.selected_worktree().is_some() => &[
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::NewShell, "シェル"),
            (Action::StopSession, "worktree を消す"),
            (Action::ExitSubPanel, "閉じる"),
            (Action::OpenBranchPage, "PR を開く"),
        ],
        None => &[
            (Action::NewSessionMenu, "新しいセッション"),
            (Action::ExitSubPanel, "閉じる"),
        ],
    };
    items
        .iter()
        .filter(|(action, _)| !needs_lower(*action) || fleet.lower_shown())
        .filter_map(|(action, label)| chord(*action).map(|key| format!("{key} {label}")))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 失うもの (未コミット、入っていないコミット、止まるセッション) を数えて見せる。数えられなかったものは
/// 0 と名乗らせない。
pub(super) fn delete_confirm(inspection: Inspection) -> Confirm {
    let check = &inspection.check;
    let label = clip(&inspection.label, 30);
    let question = match (&check.branch, check.keeps_branch) {
        (Some(_), false) => format!("worktree '{label}' とブランチを消す?"),
        (Some(_), true) => format!("worktree '{label}' を消す? (ブランチは残す)"),
        (None, _) => format!("worktree '{label}' を消す?"),
    };
    let mut detail = Vec::new();
    match check.dirty {
        Some(0) => {}
        Some(n) => detail.push(format!("未コミットの変更 {n} 件は失われる")),
        None => detail.push("未コミットの変更を確かめられなかった (あれば失われる)".into()),
    }
    match (check.unmerged, &check.base) {
        (Some(0), _) => {}
        (Some(n), Some(base)) => detail.push(format!(
            "{base} に入っていないコミット {n} 個 (squash で merge した PR もここに出る)"
        )),
        _ => detail.push("どこにも入っていないコミットがあるか確かめられなかった".into()),
    }
    let outside = inspection.claudes.iter().filter(|c| !c.own).count();
    let claudes = inspection.own_claudes + outside;
    detail.push(match (claudes, inspection.shells) {
        (0, 0) => "止まるセッションは無い".to_string(),
        (c, 0) => format!("止まる Claude {c} 本 (うちこの窓の外 {outside} 本)"),
        (0, s) => format!("止まるシェル {s} 本 (ここで開いたもの)"),
        (c, s) => format!("止まる Claude {c} 本 (うちこの窓の外 {outside} 本)、シェル {s} 本"),
    });
    detail.into_iter().fold(
        Confirm::yes_only(
            question,
            vec![Effect::DeleteFleetWorktree(Box::new(inspection))],
        )
        .title("Delete worktree"),
        Confirm::detail,
    )
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
    let mut title = vec![Span::styled(
        row.title.clone().unwrap_or_else(|| UNTITLED.to_string()),
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
    )];
    if let Some((name, color)) = account(ws, &row) {
        title.insert(
            0,
            Span::styled(format!("{name} "), Style::default().fg(color)),
        );
    }
    let mut lines = vec![
        Line::from(title),
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

pub fn lower_title(ws: &Workspace) -> String {
    let fleet = &ws.panels.fleet;
    match fleet.lower() {
        Lower::Changes => {
            return match fleet.changes.for_target(fleet.selected_dir().as_deref()) {
                Some((changes, _)) => git_changes::render::title(changes),
                None => " Git Changes ".to_string(),
            };
        }
        Lower::Artifacts => {
            let target = fleet.selected_conversation().map(|c| c.id);
            let count = fleet
                .artifacts
                .for_target(target.as_deref())
                .map_or_else(String::new, |list| format!(" ({})", list.len()));
            return format!(
                " Artifacts{count}{} ",
                keys(
                    ws,
                    &[
                        (Action::ShowFleetChanges, "で Git Changes"),
                        (Action::NewShell, "でシェル"),
                    ]
                )
            );
        }
        Lower::Shell => {}
    }
    match (ws.focus, ws.fleet_shell()) {
        (Focus::FleetShell, _) => format!(
            " Shell · 入力中{} ",
            keys(
                ws,
                &[
                    (Action::LeaveTerminal, "で一覧へ"),
                    (Action::OpenFileFromTerminal, "で Git Changes"),
                ]
            )
        ),
        (_, Some(_)) => format!(
            " Shell{} ",
            keys(
                ws,
                &[
                    (Action::NewShell, "で入る"),
                    (Action::ShowFleetChanges, "で Git Changes"),
                ]
            )
        ),
        (_, None) => " Shell ".to_string(),
    }
}

/// 割り当ての無いキーは案内に出さない。
fn keys(ws: &Workspace, items: &[(Action, &str)]) -> String {
    items
        .iter()
        .filter_map(|(action, label)| {
            crate::render::representative_chord(&ws.keymap, ws.key_context(), *action)
                .map(|key| format!(" · {key} {label}"))
        })
        .collect()
}

/// 一覧の左下。Git Changes か、選んだ行の worktree のシェルか、選んだセッションの Artifact。
pub fn lower(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    match ws.panels.fleet.lower() {
        Lower::Changes => return changes(frame, rect, ws),
        Lower::Artifacts => return artifacts(frame, rect, ws),
        Lower::Shell => {}
    }
    if let Some(session) = ws.fleet_shell() {
        crate::panels::terminal::render::fleet_shell(frame, rect, ws, &session);
        return;
    }
    let text = format!(
        "  シェルなし{}",
        keys(
            ws,
            &[
                (Action::NewShell, "で起動"),
                (Action::ShowFleetChanges, "で Git Changes"),
            ]
        )
    );
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(ws.theme.hint))),
        inner(rect),
    );
}

fn changes(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = inner(rect);
    let fleet = &ws.panels.fleet;
    let target = fleet.selected_dir();
    let lines = match (fleet.changes.for_target(target.as_deref()), &target) {
        (Some((changes, status)), _) => git_changes::render::lines(
            changes,
            status,
            // コメントと既読はこの窓の repo のものしか持っていない。
            &ReviewState::default(),
            &ws.theme,
            &ws.config,
            git_changes::render::Area {
                width: usize::from(inner.width),
                height: usize::from(inner.height),
            },
            false,
        )
        .into_iter()
        // 操作しない区画なので、先頭行に付く選択の地は消す。
        .map(|mut line| {
            line.style.bg = None;
            line
        })
        .collect(),
        (None, Some(_)) => vec![Line::styled(
            "  loading\u{2026}",
            Style::default().fg(ws.theme.muted),
        )],
        (None, None) => Vec::new(),
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

/// 1 行目が先頭の Artifact。押した行はこの並びで引く (`SessionArtifacts::click`)。
fn artifacts(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let fleet = &ws.panels.fleet;
    let hint = |text: &str| {
        vec![Line::styled(
            text.to_string(),
            Style::default().fg(ws.theme.hint),
        )]
    };
    let target = fleet.selected_conversation();
    let lines = match (
        &target,
        fleet
            .artifacts
            .for_target(target.as_ref().map(|c| c.id.as_str())),
    ) {
        (None, _) => hint("  セッションを選ぶと出ます"),
        (Some(_), None) => vec![Line::styled(
            "  loading\u{2026}",
            Style::default().fg(ws.theme.muted),
        )],
        (Some(_), Some([])) => hint("  Artifact はまだありません"),
        (Some(_), Some(list)) => list
            .iter()
            .map(|a| {
                Line::from(vec![
                    Span::styled(a.title.clone(), Style::default().fg(ws.theme.fg)),
                    Span::styled(format!("  {}", a.url), Style::default().fg(ws.theme.hint)),
                ])
            })
            .collect(),
    };
    frame.render_widget(Paragraph::new(lines), inner(rect));
}

pub fn inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    )
}

type LimitsSpans<'a> = dyn Fn(&[(&str, Limit)]) -> Vec<Span<'static>> + 'a;

const WITH_WEEK: [Form; 3] = [Form::BarsWithCountdown, Form::Bars, Form::Percent];

/// 使用量を forms の形ごとに。アカウントがあれば、アカウントごとに名前をその色で前に置く。
fn usage_options(
    usage: &Usage,
    accounts: &[ClaudeAccount],
    theme: &Theme,
    now: SystemTime,
    forms: &[Form],
) -> Vec<Vec<Span<'static>>> {
    let groups = (usage.limits.iter())
        .enumerate()
        .map(|(i, limits)| {
            let shown = [("5h", limits.five_hour), ("7d", limits.seven_day)]
                .into_iter()
                .filter_map(|(label, limit)| Some((label, limit?)))
                .collect::<Vec<_>>();
            (accounts.get(i), shown)
        })
        .filter(|(_, shown)| !shown.is_empty())
        .collect::<Vec<_>>();
    let compose = |each: &LimitsSpans<'_>| {
        let mut spans = Vec::new();
        for (g, (account, limits)) in groups.iter().enumerate() {
            if g > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(theme.hint)));
            }
            if let Some(account) = account {
                spans.push(Span::styled(
                    format!("{} ", account.name),
                    Style::default().fg(account.color().unwrap_or(theme.info)),
                ));
            }
            spans.extend(each(limits));
        }
        spans
    };
    if groups.is_empty() {
        return Vec::new();
    }
    forms
        .iter()
        .map(|&form| compose(&|limits| limit_spans(limits, form, theme, now)))
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Form {
    BarsWithCountdown,
    Bars,
    Percent,
    /// 最初の枠の率だけ。5h ならラベルも省く。
    FirstLimit,
}

fn limit_spans(
    limits: &[(&str, Limit)],
    form: Form,
    theme: &Theme,
    now: SystemTime,
) -> Vec<Span<'static>> {
    let hint = Style::default().fg(theme.hint);
    if form == Form::FirstLimit {
        let (label, limit) = limits[0];
        let pct = limit.used_percentage.round().clamp(0.0, 100.0) as u8;
        let label = if label == "5h" {
            String::new()
        } else {
            format!("{label} ")
        };
        return vec![Span::styled(
            format!("{label}{pct}%"),
            Style::default().fg(level(theme, pct)),
        )];
    }
    let mut spans = Vec::new();
    for (i, (label, limit)) in limits.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                if form == Form::Percent { " " } else { " · " },
                hint,
            ));
        }
        let pct = limit.used_percentage.round().clamp(0.0, 100.0) as u8;
        spans.push(Span::styled(format!("{label} "), hint));
        if form == Form::Percent {
            spans.push(Span::styled(
                format!("{pct}%"),
                Style::default().fg(level(theme, pct)),
            ));
            continue;
        }
        spans.extend(bar(pct, 8, theme));
        let mut text = format!(" {pct}%");
        if form == Form::BarsWithCountdown {
            text.push_str(&format!(" {}", countdown_text(limit.secs_left(now))));
        }
        spans.push(Span::styled(text, hint));
    }
    spans
}

/// 空きは網かけでなく地を敷いたマスで描く。網かけは暗いテーマで地に沈み、
/// 端数のブロックはマスの左しか塗らないので、地を敷かないと塗りと空きの間が抜ける。
fn bar(pct: u8, cells: usize, theme: &Theme) -> Vec<Span<'static>> {
    const PARTIAL: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = usize::from(pct.min(100)) * cells * 8 / 100;
    let (full, rest) = (eighths / 8, eighths % 8);
    let empty = cells - full - usize::from(rest > 0);
    let fill = Style::default().fg(level(theme, pct));
    let track = Style::default().bg(theme.hint);
    vec![
        Span::styled("█".repeat(full), fill),
        Span::styled(PARTIAL[rest], fill.bg(theme.hint)),
        Span::styled(" ".repeat(empty), track),
    ]
}

/// statusline と同じしきい値。
fn level(theme: &Theme, pct: u8) -> Color {
    match pct {
        90.. => theme.error,
        70.. => theme.warning,
        _ => theme.accent,
    }
}

fn countdown_text(secs: i64) -> String {
    match secs {
        86_400.. => format!("{}d{}h", secs / 86_400, secs % 86_400 / 3600),
        3600.. => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}m", secs.max(0) / 60),
    }
}

/// 本文の右端に右から並ぶボタン。当たり判定は [buttons] を読み、描画 (worktree_line /
/// repo_header) は同じ並びで直書きしている。
pub const NEW_BUTTON: &str = "[+]";
pub const SMART_BUTTON: &str = "[✧]";
const BUTTON_WIDTH: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    New,
    Smart,
    Revidere,
    /// revidere の印。印がある行でだけ押せる。
    Mark,
}

/// 行のボタンを右端から順に。
pub fn buttons(slot: &Slot) -> &'static [Button] {
    match slot {
        Slot::Repo(Some(_)) => &[Button::Smart],
        Slot::Worktree(w) if w.in_tree => &[Button::New, Button::Revidere, Button::Mark],
        Slot::Worktree(_) => &[Button::New],
        Slot::Repo(None) | Slot::Session(_) | Slot::Other => &[],
    }
}

/// x が右端から何番目のボタンの桁か。行の中身は見ない。
pub fn button_column(composed: &Composed, x: u16) -> Option<usize> {
    let from_right = usize::from(composed.body.right().checked_sub(x + 1)? / BUTTON_WIDTH);
    (from_right < MOST_BUTTONS).then_some(from_right)
}

const MOST_BUTTONS: usize = 3;

pub fn button_hit(composed: &Composed, x: u16, y: u16) -> Option<(Button, &Slot)> {
    let slot = slot_at(composed, x, y)?;
    let button = *buttons(slot).get(button_column(composed, x)?)?;
    Some((button, slot))
}

pub fn slot_at(composed: &Composed, x: u16, y: u16) -> Option<&Slot> {
    let body = composed.body;
    if !body.contains(Position::new(x, y)) {
        return None;
    }
    composed
        .slots
        .get(composed.offset + usize::from(y - body.y))
}

fn repo_header(name: &str, theme: &Theme, width: u16, smart: bool) -> Line<'static> {
    if !smart {
        return header(name, theme.info, width);
    }
    let name = clip(name, usize::from(width.saturating_sub(BUTTON_WIDTH + 2)));
    let mut line = header(&name, theme.info, width.saturating_sub(BUTTON_WIDTH + 1));
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
    icons: IconSet,
    worktree: &Worktree,
    review: Option<Mark>,
    width: u16,
    selected: bool,
    focused: bool,
) -> Line<'static> {
    let (badge, badge_fg) = match review {
        Some(Mark::Current) => (icons::REVIDERE_CURRENT.get(icons), theme.success),
        Some(Mark::Stale) => (icons::REVIDERE_STALE.get(icons), theme.warning),
        None => (" ", theme.hint),
    };
    let revidere = if worktree.in_tree {
        format!("[{}]", icons::REVIDERE.get(icons))
    } else {
        "   ".to_string()
    };
    let room = usize::from(width.saturating_sub(2 * BUTTON_WIDTH + 5));
    let label = clip(&worktree.label, room);
    let pad = room.saturating_sub(label.width());
    let lead = if selected { " ❯" } else { "  " };
    let spans = vec![
        Span::styled(lead, Style::default().fg(theme.accent)),
        // repo の見出しと同じ色で、下に並ぶセッションの題と分ける。
        Span::styled(label, Style::default().fg(theme.info)),
        Span::raw(" ".repeat(pad + 1)),
        Span::styled(badge, Style::default().fg(badge_fg)),
        Span::raw(" "),
        Span::styled(revidere, Style::default().fg(theme.fg)),
        Span::styled(NEW_BUTTON, Style::default().fg(theme.accent)),
    ];
    crate::list::row_line(spans, theme, selected, focused)
}

/// 行の Claude のアカウントの名前と色。accounts に無い dir の行は None。
fn account(ws: &Workspace, row: &Row) -> Option<(String, Color)> {
    let account = ws.config.account_of(row.sessions_dir.as_deref()?)?;
    Some((
        account.name.clone(),
        account.color().unwrap_or(ws.theme.info),
    ))
}

fn row_line(
    ws: &Workspace,
    row: &Row,
    width: u16,
    indent: usize,
    selected: bool,
    focused: bool,
    now: SystemTime,
) -> Line<'static> {
    let theme = &ws.theme;
    let account = account(ws, row);
    let (glyph, color) = look(theme, &row.state);
    // 選択は地の色だけだと薄く、地を明るくすると hint の文字とバーの空きが沈むので印で示す。
    let (edge, edge_fg) = if selected {
        ("❯", theme.accent)
    } else if row.place == Place::Outside {
        ("╎", color)
    } else {
        ("▌", color)
    };
    let age = row.since.map(|s| elapsed(s, now)).unwrap_or_default();
    let left = format!(
        " {age}{} {}",
        " ".repeat(6usize.saturating_sub(age.width())),
        tag(row.place)
    );
    let right: Vec<Span<'static>> = match (row.context_percent, row.context_tokens) {
        (Some(pct), _) => {
            let mut spans = bar(pct, 4, theme);
            spans.push(Span::styled(
                format!(" {pct:>3}%"),
                Style::default().fg(level(theme, pct)),
            ));
            spans
        }
        (None, Some(tokens)) => vec![Span::styled(
            tokens_text(tokens),
            Style::default().fg(theme.hint),
        )],
        (None, None) => Vec::new(),
    };
    let right_width: usize = right.iter().map(|s| s.content.width()).sum();
    let account = account.map(|(name, color)| {
        Span::styled(
            format!("{name} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    });
    let account_width = account.as_ref().map_or(0, |s| s.content.width());
    let title = row.title.clone().unwrap_or_else(|| UNTITLED.to_string());
    let room = (width as usize)
        .saturating_sub(indent + 2 + left.width() + account_width + right_width + 2);
    let title = clip(&title, room);
    let pad = room.saturating_sub(title.width());
    let title_fg = if row.title.is_some() {
        theme.fg
    } else {
        theme.hint
    };
    let mut spans = vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(
            edge,
            Style::default().fg(edge_fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            glyph,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(left, Style::default().fg(theme.hint)),
    ];
    spans.extend(account);
    spans.extend([
        Span::styled(title, Style::default().fg(title_fg)),
        Span::raw(" ".repeat(pad + 2)),
    ]);
    spans.extend(right);
    crate::list::row_line(spans, theme, selected, focused)
}

fn pinned_box(
    theme: &Theme,
    row: &Row,
    account: Option<(String, Color)>,
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
    let lead = format!(
        "! {} {}",
        row.since.map(|s| elapsed(s, now)).unwrap_or_default(),
        tag(row.place),
    );
    let title = row.title.clone().unwrap_or_else(|| UNTITLED.to_string());
    let base = if selected {
        crate::list::selection(theme, focused)
    } else {
        Style::default()
    };
    let body = |parts: Vec<(String, Style)>| {
        let mut room = inner_w;
        let mut spans = vec![Span::styled("│", border)];
        for (text, style) in parts {
            let text = clip(&text, room);
            room = room.saturating_sub(text.width());
            spans.push(Span::styled(text, base.patch(style)));
        }
        spans.push(Span::styled(" ".repeat(room), base));
        spans.push(Span::styled("│", border));
        Line::from(spans)
    };
    let fg = Style::default().fg(theme.fg);
    let mut first = vec![(lead, fg)];
    first.extend(account.map(|(name, color)| (format!("{name} "), Style::default().fg(color))));
    first.push((title, fg));
    vec![
        Line::styled(format!("┌{}┐", "─".repeat(inner_w)), border),
        body(first),
        body(vec![(format!("  {ask}"), Style::default().fg(color))]),
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
