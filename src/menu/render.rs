//! メニューバーの描画: タイトルバーの下に常時表示されるタイトルのストリップと、
//! 開いているメニューのドロップダウン。
//!
//! どちらのパスもヒット領域を app.menu に記録するので、マウスハンドラは
//! 実際に描画されたものに対してクリックを解決できる — worktree ストリップで
//! [crate::worktree::bar] が使っているのと同じ契約。
//!
//! スタイリングは既存のポップアップに合わせている（Clear + Borders::ALL +
//! アクセントボーダー、ハイライトには selected_bg/selected_fg）。これにより
//! ドロップダウンが後付けのウィジェットではなく同じアプリの一部として見える。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::hit_map::ColumnSpans;
use crate::menu::model::{MENUS, MenuItem};
use crate::menu::state::ItemHit;

/// トップレベルのタイトルの両側に置く空白カラム。ハイライトに少し余裕を
/// 持たせ、クリック対象も広げる。
const TITLE_PAD: u16 = 1;

/// 行のラベルの終わりからショートカットの始まりまでのカラム数。
const LABEL_CHORD_GAP: u16 = 4;

fn width(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// メニューバーの行を描画し、各タイトルのクリック領域を記録する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 || area.width == 0 {
        app.menu.bar_hits.clear();
        return;
    }

    let theme = &app.theme;
    let active = app.menu.focus.active_index();
    let hover = app.menu.hover;

    // Color::Reset にして、上のタイトルバーに合わせてターミナル自身の背景
    // （背景画像も含む）が透けて見えるようにする。
    let bar_bg = ratatui::style::Color::Reset;

    let mut spans: Vec<Span> = Vec::new();
    let mut hits: ColumnSpans<usize> = ColumnSpans::default();
    let mut x = area.x;

    for (i, menu) in MENUS.iter().enumerate() {
        let text = format!(
            "{pad}{icon}{title}{pad}",
            pad = " ".repeat(TITLE_PAD as usize),
            icon = menu.icon.labeled(app.config.ui.icon_set()),
            title = menu.title
        );
        let w = width(&text);

        // 行からはみ出す前に止める。途中で切れたタイトルを描くと、画面上の
        // 見た目と一致しないヒット領域を記録してしまう。
        if x + w > area.x + area.width {
            break;
        }

        let style = if active == Some(i) {
            // 開いている/フォーカス中のメニューは「選択状態」なので、アプリ内の
            // 他の選択行と同じ背景の扱いにする。
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if hover == Some(i) {
            // hover は前景色だけで表現する: いくつかのテーマは背景画像に配慮した
            // 透明なバーになっており、そこに hover の背景を塗るとタイトルバーの
            // Color::Reset とぶつかってしまう。
            Style::default()
                .fg(theme.accent)
                .bg(bar_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg).bg(bar_bg)
        };

        spans.push(Span::styled(text, style));
        hits.push(x, x + w, i);
        x += w;
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bar_bg)),
        area,
    );
    app.menu.bar_hits = hits;
}

/// 開いているメニューがあればそのドロップダウンを描画し、行のヒット領域を
/// 記録する。
///
/// パネルの後に呼ばれるので、ポップアップはそれらの上に乗る。frame_area は
/// 画面全体: ドロップダウンはメインエリアの上にあるメニューバーから垂れ
/// 下がるものなので、メインエリアではなく画面全体にクランプする。
pub fn render_dropdown(frame: &mut Frame, frame_area: Rect, app: &mut App) {
    let (menu_idx, selected, scroll) = match app.menu.focus {
        crate::menu::MenuFocus::Open {
            index,
            selected,
            scroll,
        } => (index, selected, scroll),
        _ => {
            app.menu.clear_dropdown_regions();
            return;
        }
    };
    let Some(menu) = MENUS.get(menu_idx) else {
        app.menu.clear_dropdown_regions();
        return;
    };

    // ショートカットのヒントはフォーカス中パネルのレイヤーに対して解決するので、
    // 行には今まさに発火するチョードが表示される — コマンドパレットと同じ規則。
    let context = app.focus.current().key_context();
    let rows: Vec<Row> = menu
        .items
        .iter()
        .map(|item| match item {
            MenuItem::Separator => Row::Separator,
            MenuItem::Command { id, label } => {
                let chord = crate::command_palette::COMMANDS
                    .iter()
                    .find(|c| c.id == *id)
                    .and_then(|c| c.action)
                    .and_then(|a| crate::ui::common::representative_chord(&app.keymap, context, a))
                    .unwrap_or_default();
                Row::Command {
                    label,
                    chord,
                    enabled: app.command_enabled(*id),
                }
            }
        })
        .collect();

    // ジオメトリの計算。
    let label_w = rows
        .iter()
        .map(|r| match r {
            Row::Command { label, .. } => width(label),
            Row::Separator => 0,
        })
        .max()
        .unwrap_or(0);
    let chord_w = rows
        .iter()
        .map(|r| match r {
            Row::Command { chord, .. } => width(chord),
            Row::Separator => 0,
        })
        .max()
        .unwrap_or(0);

    // ボーダー2カラム + 前後のパディング1カラムずつ。
    let content_w = label_w + LABEL_CHORD_GAP + chord_w;
    let popup_w = (content_w + 4).min(frame_area.width);

    let anchor_x = app
        .menu
        .bar_hits
        .spans()
        .find(|(_, _, m)| **m == menu_idx)
        .map(|(x0, _, _)| x0)
        .unwrap_or(frame_area.x);
    // 右側のメニューがはみ出す場合でもポップアップが画面内に収まるようにする。
    let max_x = (frame_area.x + frame_area.width).saturating_sub(popup_w);
    let popup_x = anchor_x.min(max_x);

    let popup_y = app.layout.cache.menubar_area.y + app.layout.cache.menubar_area.height;
    let avail_h = (frame_area.y + frame_area.height).saturating_sub(popup_y);
    // ボーダー2行分; 最低1行のコンテンツ行がなければ表示するものがない。
    let popup_h = ((rows.len() as u16) + 2).min(avail_h);
    if popup_h < 3 || popup_w < 4 {
        app.menu.clear_dropdown_regions();
        return;
    }
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(format!(" {} ", menu.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // 各行の描画。
    let visible = inner.height as usize;
    let start = scroll.min(rows.len().saturating_sub(visible.max(1)));
    let theme = &app.theme;
    let mut lines: Vec<Line> = Vec::new();
    let mut hits: Vec<ItemHit> = Vec::new();

    for (offset, row) in rows.iter().skip(start).take(visible).enumerate() {
        let y = inner.y + offset as u16;
        let idx = start + offset;
        match row {
            Row::Separator => {
                lines.push(Line::from(Span::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(theme.border_unfocused),
                )));
            }
            Row::Command {
                label,
                chord,
                enabled,
            } => {
                let is_selected = idx == selected;
                let pad = (inner.width as usize)
                    .saturating_sub(width(label) as usize + width(chord) as usize + 2);
                // 無効化された行は位置とラベルは保つが、ショートカットのヒントは
                // 失う: 今何もしないチョードを表示するのは、そのキーが何をする
                // かについて嘘をつくことになる。
                let shown_chord = if *enabled { chord.as_str() } else { "" };
                let pad = if *enabled {
                    pad
                } else {
                    pad + width(chord) as usize
                };

                let (label_style, chord_style) = match (is_selected, *enabled) {
                    (true, true) => (
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(theme.selected_fg).bg(theme.selected_bg),
                    ),
                    // 選択中だが無効化されている行でもカーソル位置は表示するので、
                    // メニューを矢印キーでたどっていて止まったように見えることはない。
                    (true, false) => (
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                            .add_modifier(Modifier::DIM),
                        Style::default().fg(theme.selected_fg).bg(theme.selected_bg),
                    ),
                    (false, true) => (
                        Style::default().fg(theme.fg),
                        Style::default().fg(theme.hint),
                    ),
                    // theme.muted ではなく通常の前景色に DIM をかける: muted は
                    // 同梱テーマのいくつかで背景色と同じかそれに近く、それでは
                    // 「利用不可」に見えるのではなく行そのものが消えてしまう。
                    (false, false) => (
                        Style::default().fg(theme.fg).add_modifier(Modifier::DIM),
                        Style::default().fg(theme.fg).add_modifier(Modifier::DIM),
                    ),
                };

                lines.push(Line::from(vec![
                    Span::styled(format!(" {label}"), label_style),
                    Span::styled(" ".repeat(pad), label_style),
                    Span::styled(format!("{shown_chord} "), chord_style),
                ]));
                hits.push(ItemHit {
                    y,
                    item: idx,
                    enabled: *enabled,
                });
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
    app.menu.item_hits = hits;
    app.menu.dropdown_area = popup_area;
}

/// 描画用に解決したドロップダウンの1行: ラベル、有効なショートカット、利用可否。
enum Row {
    Command {
        label: &'static str,
        chord: String,
        enabled: bool,
    },
    Separator,
}

/// frame_height のときにドロップダウンが表示できるコンテンツ行数。キー
/// ハンドラが選択項目をスクロールして見える位置に保つために使う。
/// [render_dropdown] のクランプと同じ計算をしている。
pub fn visible_rows(app: &App, frame_height: u16) -> usize {
    let popup_y = app.layout.cache.menubar_area.y + app.layout.cache.menubar_area.height;
    let avail_h = frame_height.saturating_sub(popup_y);
    avail_h.saturating_sub(2) as usize
}
