//! worktree パネルのゾーン1、すなわち worktree + インラインセッション一覧の描画。
//! 選択状態・待機/実行中インジケータ・ステータスマーカーを表示する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState};
use unicode_width::UnicodeWidthChar;

use crate::app::{App, PendingWorktreeOp, WorktreeListRow};
use crate::git_engine::WorktreeInfo;
use crate::theme::Theme;
use crate::ui::common::PanelChrome;

/// 未コミット変更が無いことを示すチェック。
const CLEAN_MARK: &str = " \u{2713}";
/// pending-create 行で説明文を切り詰める幅。
const PENDING_DESC_WIDTH: usize = 30;

/// 文字列を max_width の表示幅に収まるよう切り詰める。
/// 切り詰めが発生した場合は末尾に "..." を付ける。
pub(super) fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut end = s.len();
    for (i, ch) in s.char_indices() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > max_width {
            end = i;
            break;
        }
        width += cw;
    }
    if end < s.len() {
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

/// インラインセッション行 1 つ分の、描画に要る情報。
struct SessionRow {
    label: String,
    waiting: bool,
    active: bool,
}

/// 1 回の描画のあいだ、全行で共有される値。
///
/// 行ごとに引数を並べ直さなくて済むよう束ねている。中身はどれも
/// 「このフレームでの見た目」を決めるもので、行をまたいで変わらない。
struct RowCtx<'a> {
    theme: &'a Theme,
    /// 待機パルスの位相 (約 1 秒周期)。
    pulse_on: bool,
    /// 非同期処理中に回すブライユのスピナー。
    spinner: &'a str,
    /// フォーカス中の Claude パネルが映している worktree。
    /// その worktree だけは点滅させない (見ている本人には煩いだけなので)。
    focused_cc_wt: Option<PathBuf>,
    /// pty 添字 → その行の表示情報。
    sessions: HashMap<usize, SessionRow>,
    /// 選択中の worktree の添字。
    selected: usize,
}

impl RowCtx<'_> {
    /// この worktree の点滅を抑えるべきか (待機中で、かつフォーカス中の CC パネルが
    /// まさにそれを映している)。
    fn suppress_blink(&self, waiting: bool, path: &Path) -> bool {
        waiting && self.focused_cc_wt.as_deref() == Some(path)
    }
}

/// worktree + インラインセッション一覧（ゾーン1）を描画する。
pub(super) fn render_worktree_list(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    focused: bool,
    border_color: Color,
) {
    let ctx = RowCtx {
        theme: &app.theme,
        // 約 1 秒周期 (60fps で 30 フレーム点灯 / 30 フレーム消灯)。
        pulse_on: (app.ui_tick / 30).is_multiple_of(2),
        spinner: super::super::common::spinner_frame(app.ui_tick),
        focused_cc_wt: (app.focus == crate::app::Focus::TerminalClaude)
            .then(|| app.selected_worktree_path()),
        sessions: collect_session_rows(app),
        selected: app.worktrees.selected_index(),
    };

    let mut items: Vec<ListItem> = app
        .worktrees
        .rows
        .iter()
        .enumerate()
        .map(|(row_idx, row)| match *row {
            WorktreeListRow::Session { pty_idx, .. } => session_item(&ctx, pty_idx, row_idx),
            WorktreeListRow::Worktree(i) => worktree_item(&ctx, app, i),
        })
        .collect();
    items.extend(pending_create_items(&ctx, app));

    let list = List::new(items)
        .block(list_block(app, focused, border_color))
        .highlight_style(
            Style::default()
                .bg(ctx.theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    state.select(Some(app.worktrees.row_selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// パネルの枠。grab 中だけはタイトルを警告色にして、通常の配色を上書きする。
fn list_block<'a>(app: &'a App, focused: bool, border_color: Color) -> Block<'a> {
    let grabbed = app.worktree_mgr.grabbed_branch.is_some();
    let title = if grabbed {
        " Worktrees [GRABBED] "
    } else {
        " Worktrees "
    };

    let mut chrome = PanelChrome::new(&app.theme, title, focused, border_color)
        .with_expand_button(app.expanded_panel == Some(crate::app::Focus::Worktree));
    if grabbed {
        chrome = chrome.with_title_style(
            Style::default()
                .fg(app.theme.waiting_primary)
                .add_modifier(Modifier::BOLD),
        );
    }
    chrome.into_block()
}

/// pty 添字を引くだけで済むよう、セッションの表示情報を先に集める。
/// 待機 / 実行中は親 worktree の状態を引き継ぐ。
fn collect_session_rows(app: &App) -> HashMap<usize, SessionRow> {
    let mut rows = HashMap::new();
    for (wt_idx, _, sessions) in app.all_cc_sessions_by_worktree() {
        let Some(wt_path) = app.worktrees.get(wt_idx).map(|wt| &wt.path) else {
            continue;
        };
        let waiting = app.terminal.cc_waiting_worktrees.contains(wt_path);
        let active = app.terminal.cc_active_worktrees.contains(wt_path);
        for (pty_idx, label) in sessions {
            rows.insert(
                pty_idx,
                SessionRow {
                    label,
                    waiting,
                    active,
                },
            );
        }
    }
    rows
}

/// インラインのセッション行 (親 worktree の下にインデントして並ぶ)。
fn session_item<'a>(ctx: &RowCtx<'_>, pty_idx: usize, row_idx: usize) -> ListItem<'a> {
    let theme = ctx.theme;
    let session = ctx.sessions.get(&pty_idx);
    let label = match session.map(|s| s.label.as_str()) {
        Some(l) if !l.is_empty() => l.to_string(),
        _ => format!("CC:{}", pty_idx + 1),
    };

    let label_style = if row_idx == ctx.selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    };

    // 待機 > 実行中 > 通常 の優先順で 1 つだけマーカーを出す。
    let marker = match session {
        Some(s) if s.waiting => Span::styled(
            "   \u{23f3} ", // ⏳
            Style::default().fg(theme.waiting_primary),
        ),
        Some(s) if s.active => Span::styled(
            format!("   {} ", ctx.spinner),
            Style::default().fg(theme.accent),
        ),
        _ => Span::raw("   \u{25b8} "), // ▸
    };

    ListItem::new(Line::from(vec![marker, Span::styled(label, label_style)]))
}

/// worktree 1 行分。
fn worktree_item<'a>(ctx: &RowCtx<'_>, app: &App, i: usize) -> ListItem<'a> {
    let Some(wt) = app.worktrees.get(i) else {
        return ListItem::new(Line::from(""));
    };
    let theme = ctx.theme;

    // 削除待ちは他のどの装飾よりも優先する — その行はもう普通の worktree ではない。
    if app.is_worktree_pending_delete(&wt.path) {
        return pending_delete_item(ctx, &wt.branch);
    }

    let waiting = app.terminal.cc_waiting_worktrees.contains(&wt.path);
    let active = app.terminal.cc_active_worktrees.contains(&wt.path);
    // __grab ブランチ = 本来のブランチを main に奪われて一時的なチェックアウトを
    // 抱えている状態。目立たせる必要がないので全体を muted に落とす。
    let grabbed = wt.branch.ends_with("__grab");
    let selected = i == ctx.selected;
    let suppress_blink = ctx.suppress_blink(waiting, &wt.path);

    let mut spans = vec![
        Span::styled(
            format!(" {} ", worktree_marker(wt, grabbed, selected)),
            marker_style(ctx, waiting, suppress_blink, grabbed, selected),
        ),
        Span::styled(
            wt.branch.clone(),
            branch_style(ctx, waiting, grabbed, selected),
        ),
    ];

    if app.new_worktree_paths.contains(&wt.path) {
        spans.push(Span::styled(
            " \u{1F331}", // 🌱
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if grabbed {
        spans.push(Span::styled(" (grabbed)", Style::default().fg(theme.muted)));
    }
    if wt.is_main && app.worktree_mgr.grabbed_branch.is_some() {
        spans.push(Span::styled(
            " \u{1f4e5}grabbed", // 📥grabbed
            Style::default()
                .fg(theme.waiting_primary)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(span) = activity_span(ctx, waiting, active, grabbed, suppress_blink) {
        spans.push(span);
    }
    spans.extend(dirty_count_spans(ctx, wt, grabbed));
    if !grabbed && let Some(span) = ahead_behind_span(ctx, wt) {
        spans.push(span);
    }

    let item = ListItem::new(Line::from(spans));
    match waiting_background(ctx, waiting, grabbed, suppress_blink) {
        Some(bg) => item.style(Style::default().bg(bg)),
        None => item,
    }
}

/// 削除待ちの行。ゴミ箱アイコンとスピナーだけで、通常の装飾は一切載せない。
fn pending_delete_item<'a>(ctx: &RowCtx<'_>, branch: &str) -> ListItem<'a> {
    ListItem::new(Line::from(vec![
        Span::styled(
            format!(" {}\u{1f5d1} ", ctx.spinner), // 🗑
            Style::default().fg(ctx.theme.error),
        ),
        Span::styled(
            branch.to_string(),
            Style::default()
                .fg(ctx.theme.muted)
                .add_modifier(Modifier::DIM),
        ),
    ]))
}

/// 行頭のマーカー字形。main > grab 中 > 選択中 > その他 の順で決まる。
fn worktree_marker(wt: &WorktreeInfo, grabbed: bool, selected: bool) -> &'static str {
    if wt.is_main {
        "\u{25cf}" // ●
    } else if grabbed {
        "\u{1f512}" // 🔒
    } else if selected {
        "\u{25c9}" // ◉
    } else {
        "\u{25cb}" // ○
    }
}

fn marker_style(
    ctx: &RowCtx<'_>,
    waiting: bool,
    suppress_blink: bool,
    grabbed: bool,
    selected: bool,
) -> Style {
    let theme = ctx.theme;
    if grabbed {
        Style::default().fg(theme.muted)
    } else if waiting && !suppress_blink {
        Style::default()
            .fg(if ctx.pulse_on {
                theme.waiting_primary
            } else {
                theme.waiting_secondary
            })
            .add_modifier(Modifier::BOLD)
    } else if waiting {
        Style::default().fg(theme.waiting_primary)
    } else if selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

fn branch_style(ctx: &RowCtx<'_>, waiting: bool, grabbed: bool, selected: bool) -> Style {
    let theme = ctx.theme;
    if grabbed {
        Style::default().fg(theme.muted)
    } else if waiting {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.success)
    }
}

/// 待機中の菱形 (点滅) か、実行中のスピナー。grab 中の worktree には出さない。
fn activity_span<'a>(
    ctx: &RowCtx<'_>,
    waiting: bool,
    active: bool,
    grabbed: bool,
    suppress_blink: bool,
) -> Option<Span<'a>> {
    if grabbed {
        return None;
    }
    if waiting {
        // 点滅を抑えているあいだは、位相によらず塗りつぶしの菱形で固定する。
        let lit = suppress_blink || ctx.pulse_on;
        let glyph = if lit { " \u{25c6}" } else { " \u{25c7}" };
        let fg = if lit {
            ctx.theme.waiting_primary
        } else {
            ctx.theme.waiting_secondary
        };
        return Some(Span::styled(
            glyph,
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
        ));
    }
    if active {
        return Some(Span::styled(
            format!(" {}", ctx.spinner),
            Style::default()
                .fg(ctx.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    }
    None
}

/// 未コミット変更の内訳 (+追加 ~変更 -削除)。clean ならチェックひとつ。
fn dirty_count_spans<'a>(ctx: &RowCtx<'_>, wt: &WorktreeInfo, grabbed: bool) -> Vec<Span<'a>> {
    let theme = ctx.theme;
    if wt.is_clean {
        return vec![Span::styled(CLEAN_MARK, Style::default().fg(theme.muted))];
    }
    // grab 中は数字の意味が薄いので、色を落として存在だけ示す。
    let color = |normal: Color| if grabbed { theme.muted } else { normal };
    [
        ('+', wt.added, color(theme.success)),
        ('~', wt.modified, color(theme.warning)),
        ('-', wt.deleted, color(theme.error)),
    ]
    .into_iter()
    .filter(|(_, count, _)| *count > 0)
    .map(|(sign, count, fg)| Span::styled(format!(" {sign}{count}"), Style::default().fg(fg)))
    .collect()
}

/// upstream との進み / 遅れ。同期済みなら ≡、未知 (upstream 無し) なら何も出さない。
fn ahead_behind_span<'a>(ctx: &RowCtx<'_>, wt: &WorktreeInfo) -> Option<Span<'a>> {
    let (ahead, behind) = (wt.ahead?, wt.behind?);
    if ahead == 0 && behind == 0 {
        return Some(Span::styled(" ≡", Style::default().fg(ctx.theme.muted)));
    }
    let mut text = String::from(" ");
    if ahead > 0 {
        text.push_str(&format!("↑{ahead}"));
    }
    if behind > 0 {
        text.push_str(&format!("↓{behind}"));
    }
    Some(Span::styled(text, Style::default().fg(ctx.theme.info)))
}

/// 待機中の行に敷く背景。パルスの位相で濃さを変え、呼吸しているように見せる。
fn waiting_background(
    ctx: &RowCtx<'_>,
    waiting: bool,
    grabbed: bool,
    suppress_blink: bool,
) -> Option<Color> {
    if !waiting || grabbed {
        return None;
    }
    let factor = if suppress_blink {
        0.20
    } else if ctx.pulse_on {
        0.24
    } else {
        0.16
    };
    Some(Theme::darken(ctx.theme.waiting_primary, factor))
}

/// 作成中の worktree を一覧の末尾に並べる。ブランチ名が決まるまでは
/// (Smart Worktree の LLM 生成待ちなど) 入力された説明文を代わりに出す。
fn pending_create_items<'a>(ctx: &RowCtx<'_>, app: &App) -> Vec<ListItem<'a>> {
    app.worktree_mgr
        .pending_worktrees
        .iter()
        .filter(|p| {
            matches!(
                p.op,
                PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
            )
        })
        .map(|pending| {
            let icon = if pending.op == PendingWorktreeOp::SmartCreating {
                "\u{1F9E0}" // 🧠
            } else {
                "\u{2728}" // ✨
            };
            let name = if pending.branch.is_empty() {
                truncate_to_width(&pending.description, PENDING_DESC_WIDTH)
            } else {
                pending.branch.clone()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {}{icon} ", ctx.spinner),
                    Style::default().fg(ctx.theme.success),
                ),
                Span::styled(
                    name,
                    Style::default()
                        .fg(ctx.theme.muted)
                        .add_modifier(Modifier::DIM),
                ),
            ]))
        })
        .collect()
}
