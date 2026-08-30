//! エクスプローラ上半分のファイルツリーの描画。

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState};

/// 深さレベルごとのインデント文字列のキャッシュ。確保の繰り返しを避ける。
const INDENT_CACHE: &[&str] = &[
    "",
    "  ",
    "    ",
    "      ",
    "        ",
    "          ",
    "            ",
    "              ",
    "                ",
    "                  ",
];

/// 指定した深さのインデント文字列を返す。よくある深さはキャッシュを使う。
fn indent_for_depth(depth: usize) -> std::borrow::Cow<'static, str> {
    if depth < INDENT_CACHE.len() {
        std::borrow::Cow::Borrowed(INDENT_CACHE[depth])
    } else {
        std::borrow::Cow::Owned("  ".repeat(depth))
    }
}

/// ファイルツリー（上半分）を描画する。
pub(super) fn render_file_tree(frame: &mut Frame, area: Rect, app: &mut App, panel_focused: bool) {
    let on_diff = app.explorer.focus_on_diff_list;
    let tree_focused = panel_focused && !on_diff;
    // カラム単位のフォーカス色をアニメーションさせる。ツリーは diff list に
    // フォーカスがないときの「アクティブ」要素なので、カラムがフォーカスを
    // 得るときも失うときも滑らかに遷移する。非アクティブなサブパネルは
    // 静的な secondary の色調のままにする。
    let border_color = if tree_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        app.theme.border_secondary
    } else if !on_diff {
        app.animated_border_color(Focus::Explorer)
    } else {
        app.theme.border_unfocused
    };

    let visible = app.explorer.visible_indices();
    let inner_height = area.height.saturating_sub(2) as usize;

    let tree_selected = app.explorer.tree.tree_selected;
    let selected_vis_idx = visible
        .iter()
        .position(|&i| i == tree_selected)
        .unwrap_or(0);

    let icon_set = app.config.ui.icon_set();
    let panel_icon = crate::icons::PANEL_EXPLORER.labeled(icon_set);
    let mut title = if visible.len() > inner_height {
        format!(
            " {panel_icon}Explorer ({}/{}) ",
            selected_vis_idx + 1,
            visible.len()
        )
    } else {
        format!(" {panel_icon}Explorer ")
    };
    // レビューの成果物があるサイン。コメントバッジの「まだ開いていないものが
    // ある」というパターンを踏襲する。2 列ビューを開いている間はここが
    // 描かれないので、冗長になる心配は無い。
    if app.revidere.has_review() {
        title.push_str(&crate::icons::PANEL_REVIEW.labeled(icon_set));
    }

    let theme = &app.theme;
    // ボーダーの太さは Explorer カラム全体のフォーカスで、タイトルの強調は
    // その中でツリー側にフォーカスがあるかで決まる (下半分と共有のカラムなので)。
    let title_style = if tree_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = crate::ui::common::PanelChrome::new(theme, title, panel_focused, border_color)
        .with_expand_button(app.expanded_panel == Some(Focus::Explorer))
        .with_title_style(title_style)
        .into_block();

    let scroll = app.explorer.tree.tree_scroll;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(vis_idx, &tree_idx)| {
            let entry = app.explorer.tree.file_tree.get(tree_idx)?;
            let indent = indent_for_depth(entry.depth);

            // 名前部分と切り離すことで、hover 時の下線を名前自体に限定できる
            // (list_row::decoration_style を参照)。アイコンをさらに分けているのは
            // 種別ごとの色を乗せるため。
            let prefix = if entry.is_dir {
                let arrow = crate::icons::expand_arrow(entry.is_expanded, icon_set);
                format!("{indent}{arrow} ")
            } else {
                format!("{indent}  ")
            };
            let icon = if entry.is_dir {
                crate::icons::dir_icon(entry.is_expanded)
            } else {
                entry.icon
            };
            let glyph = format!("{} ", icon.glyph(icon_set));

            // 未追跡/無視エントリはファイルかディレクトリかに関わらず暗く表示し、
            // 下のディレクトリ/ファイルの色分けより優先する。ここで
            // theme.muted を意図的に避けているのは、solarized-dark では
            // 背景と同じ RGB で事実上見えなくなり、github-light では
            // ボーダー色に見えてしまうため。
            let base_fg = match entry.git_state {
                crate::git_engine::status_map::TreeGitState::Untracked
                | crate::git_engine::status_map::TreeGitState::Ignored => theme.hint,
                crate::git_engine::status_map::TreeGitState::Tracked => {
                    if entry.is_dir {
                        theme.info
                    } else {
                        theme.fg
                    }
                }
            };
            let hover = app.list_hover.explorer_tree.phase(vis_idx);
            let style = crate::ui::common::list_row::row_style(
                theme,
                base_fg,
                vis_idx == selected_vis_idx,
                tree_focused,
                hover,
            );

            let decoration = crate::ui::common::list_row::decoration_style(style);
            // アイコンの色はファイル種別を表すが、選択行と減光対象の行では行の色に
            // 譲る。選択の背景色の上で種別色が読める保証は11テーマぶんには無く、
            // untracked/ignored の減光はアイコンにも及ぶべきだからである。
            let icon_style = if vis_idx == selected_vis_idx
                || !matches!(
                    entry.git_state,
                    crate::git_engine::status_map::TreeGitState::Tracked
                ) {
                decoration
            } else {
                decoration.fg(icon.role.color(theme))
            };

            Some(ListItem::new(Line::from(vec![
                Span::styled(prefix, decoration),
                Span::styled(glyph, icon_style),
                Span::styled(entry.name.clone(), style),
            ])))
        })
        .collect();

    // 最後の項目より下の行（またはスクロールや高さ変更後の古い行）に
    // 前フレームの文字が残らないよう、先にクリアする。viewer と同じ
    // スクロール残像対策。
    frame.render_widget(ratatui::widgets::Clear, area);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);

    // パネルに収まりきらない数の項目があるときだけスクロールバーを描画する。
    if visible.len() > inner_height {
        let inner_area = area.inner(ratatui::layout::Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state =
            ScrollbarState::new(visible.len().saturating_sub(inner_height)).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, inner_area, &mut scrollbar_state);
    }
}
