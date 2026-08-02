//! エクスプローラ上半分のファイルツリーの描画。

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

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
    let on_diff = app.viewer_state.explorer.explorer_focus_on_diff_list;
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

    let visible = app.viewer_state.visible_indices();
    let inner_height = area.height.saturating_sub(2) as usize;

    let tree_selected = app.viewer_state.tree.tree_selected;
    let selected_vis_idx = visible
        .iter()
        .position(|&i| i == tree_selected)
        .unwrap_or(0);

    let mut title = if visible.len() > inner_height {
        format!(" Explorer ({}/{}) ", selected_vis_idx + 1, visible.len())
    } else {
        " Explorer ".to_string()
    };
    // walkthrough 準備完了のサイン。コメントバッジの「まだ開いていないものが
    // ある」というパターンを踏襲する。walkthrough ビューを既に表示している
    // ときはバッジが冗長になるので隠す。
    let walkthrough_ready = matches!(
        app.walkthrough.current.as_ref().map(|wt| wt.header.status),
        Some(crate::walkthrough::WalkthroughStatus::Ready)
    );
    if walkthrough_ready
        && app.viewer_state.explorer.explorer_bottom_view
            != crate::viewer::ExplorerBottomView::Walkthrough
    {
        title.push_str("\u{1f9ed} ");
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

    let scroll = app.viewer_state.tree.tree_scroll;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(vis_idx, &tree_idx)| {
            let entry = app.viewer_state.tree.file_tree.get(tree_idx)?;
            let indent = indent_for_depth(entry.depth);

            // 名前部分と切り離すことで、hover 時の下線を名前自体に限定できる
            // (list_row::decoration_style を参照)。
            let prefix = if entry.is_dir {
                let arrow = if entry.is_expanded {
                    "\u{25bc}" // ▼
                } else {
                    "\u{25b6}" // ▶
                };
                format!("{indent}{arrow} {} ", entry.icon)
            } else {
                format!("{indent}  {} ", entry.icon)
            };

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

            Some(ListItem::new(Line::from(vec![
                Span::styled(
                    prefix,
                    crate::ui::common::list_row::decoration_style(style),
                ),
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
