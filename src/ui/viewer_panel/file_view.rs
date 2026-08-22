//! ビューアパネルのデフォルトの描画モード（プレーン/注釈付きファイルの内容）。
//!
//! 現在開いているファイルについて、行番号、diff ガターマーカー、コメント
//! マーカー、シンタックスハイライト、インラインのレビューコメントスレッドを
//! 描画する。unified diff モードは [super::diff_view] に、それ以外の疑似モードは
//! [super::media_view] / [super::summary_view] に委譲する。

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::code_line::{FileLineRenderCtx, render_code_line_rows};
use super::comment_thread::new_comment_anchor_end;
use super::diff_view::render_diff_view;
use super::markdown_view::{TOGGLE_W, render_markdown_view, toggle_segments, toggle_spans};
use super::media_view::render_media_view;
use super::search_box::render_search_box;
use super::span_utils::digit_count;
use super::summary_view::render_summary_view;
use super::tab_row;
use super::syntax::ensure_diff_annotations_cached;

/// 与えられた area に Viewer（ファイル内容）パネルを描画する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // 画面行マップをクリアし、diff/media モードで古いデータが使われないようにする。
    app.viewer_state.content.screen_row_map.clear();
    app.viewer_state.tab_row_hits.clear();

    // Summary 疑似ファイル: ブランチの変更サマリーがパネル全体を占める。
    // 描画関数が &mut App を取れるよう、共有借用の前にチェックする。
    if app.viewer_state.is_summary() {
        let focused = app.focus == Focus::Viewer;
        render_summary_view(frame, area, app, focused);
        return;
    }

    // 共有借用を取る前に diff 注釈キャッシュを埋める。
    ensure_diff_annotations_cached(app);

    // 描画の直前に、カーソル行が畳みの中に隠れていないかだけを正す。file_scroll を
    // 書く経路（検索・定義ジャンプ・grep・履歴復元）はどれもここに合流するので、
    // 「飛んだ先が畳まれていたら開く」の判断はこの1か所で足りる。
    //
    // diff 表示は除く。そこでの file_scroll は diff カーソルの写しでしかなく、
    // 画面に出ない畳みを開いてしまうと、素の表示へ戻ったときに理由の分からない
    // 開き方をして見える。
    if !app.viewer_state.diff_view.diff_mode {
        app.viewer_state.reveal_cursor_line();
    }

    let theme = &app.theme;
    let vs = &app.viewer_state;
    let tab_width = app.config.viewer.tab_width;
    let focused = app.focus == Focus::Viewer;
    let border_color = app.animated_border_color(Focus::Viewer);

    let is_expanded = app.expanded_panel == Some(Focus::Viewer);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    // Raw/Rendered トグル — プレーンファイルビューでの markdown ファイルのみ、
    // かつ列の幅がそれを収められるだけある場合のみ（toggle_segments はクリック
    // 判定が参照するのと同じ関数）。
    let show_md_toggle =
        vs.markdown_toggle_available() && toggle_segments(area.x, area.width).is_some();
    let md_toggle_w = if show_md_toggle { TOGGLE_W as usize } else { 0 };

    // 右側の [<=>] ボタン（と表示されていればトグル）と重ならないようタイトルを
    // 切り詰める。確保するのは: 2（枠線）+ トグル + expand_label の幅 + 1（隙間）。
    let max_title_len =
        (area.width as usize).saturating_sub(2 + md_toggle_w + expand_label.len() + 1);
    let title = match &vs.content.current_file {
        Some(path) => {
            let raw = if !vs.search.search_matches.is_empty() {
                format!(
                    " {} [{}/{}] ",
                    path,
                    vs.search.search_match_idx + 1,
                    vs.search.search_matches.len()
                )
            } else if !vs.search.search_query.is_empty() {
                format!(" {path} [no matches] ")
            } else {
                format!(" {path} ")
            };
            fit_title(&raw, max_title_len)
        }
        None => " (no file selected) ".to_string(),
    };

    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    // 両方のコントロールを1つの右寄せ行にまとめているので、トグルの描画有無に
    // 関わらず [<=>] は常に同じ列に位置し、既存のクリック判定もそのまま使える。
    let mut right_spans: Vec<Span> = Vec::new();
    if show_md_toggle {
        right_spans.extend(toggle_spans(vs.is_showing_rendered_markdown(), theme));
    }
    right_spans.push(Span::styled(
        expand_label,
        Style::default().fg(expand_color),
    ));
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(Line::from(right_spans).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    // unified diff モード: 専用レンダラーに委譲する。
    if vs.diff_view.diff_mode && !vs.diff_view.diff_view_lines.is_empty() {
        render_diff_view(frame, area, app, block);
        return;
    }

    // メディアファイルモード: 画像/動画を ASCII アートとして描画する。
    if vs.is_current_file_media() {
        render_media_view(frame, area, app, block);
        return;
    }

    // Markdown 描画モード。以下の行単位の描画より前で return するので、
    // screen_row_map は空のまま（関数の入口でクリア済み）になり、ガター・
    // コメントマーカー・スレッド行は一切描画されない — 詳細は markdown_view を参照。
    if vs.is_showing_rendered_markdown() {
        render_markdown_view(frame, area, app, block);
        return;
    }

    if vs.content.file_content.is_empty() {
        // 本文が無い理由は 3 通りあり、どれなのかを言い当てる。読めなかった場合を
        // 「ファイルを選んでください」と出すと、選んだのに反応しなかったように
        // 見えてしまう (タイトルには選択中のファイル名が出ているのに、本文は
        // 未選択の案内、という食い違いになる)。
        let (text, style) = match (&vs.content.load_error, &vs.content.current_file) {
            (Some(err), Some(path)) => (
                format!("Could not read {path}\n{err}"),
                Style::default().fg(theme.error),
            ),
            (_, Some(_)) => (
                "This file is empty.".to_string(),
                Style::default().fg(theme.muted),
            ),
            (_, None) => (
                "Select a file to view its contents.".to_string(),
                Style::default().fg(theme.muted),
            ),
        };
        let mut body = String::new();
        if tab_row::is_visible(vs) {
            // タブ行のぶんだけ本文を下げる。
            body.push('\n');
        }
        body.push_str(&text);
        let placeholder = Paragraph::new(body).style(style).block(block);
        frame.render_widget(placeholder, area);
        render_tab_row(frame, area, app);
        return;
    }

    // ジャンプ履歴からパンくずリストを組み立てる。
    let breadcrumb_visible = build_breadcrumb_line(app);

    // パンくずバーとタブ行の高さぶんを見込む（表示されているときは各1行）。
    let breadcrumb_height: u16 = if breadcrumb_visible.is_some() { 1 } else { 0 };
    let tab_row_height: u16 = if tab_row::is_visible(vs) { 1 } else { 0 };
    let inner_height =
        (area.height.saturating_sub(2 + breadcrumb_height + tab_row_height)) as usize;
    let gutter_width = digit_count(vs.content.file_content.len());

    // diff 注釈は ViewerState にキャッシュされている（関数の入口で埋めた）。
    let diff_annotations = app
        .viewer_state
        .content
        .cached_diff_annotations
        .as_ref()
        .unwrap();

    // レビューコメントが付いている行番号を集める（メモリ上のキャッシュから）。
    let comment_lines: std::collections::HashSet<usize> =
        app.review_state.file_comments.keys().copied().collect();

    // コメントの「最終」行（各範囲の最後の行 — 💬 が表示される場所）を集める。
    let comment_end_lines: std::collections::HashSet<usize> = app
        .review_state
        .comments
        .iter()
        .filter(|c| app.viewer_state.content.current_file.as_deref() == Some(&*c.file_path))
        .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
        .collect();

    // 表示行を組み立て、コメント行の後にインラインスレッドの行を挿入する。
    let expanded_threads = &app.viewer_state.explorer.expanded_inline_threads;
    let inline_reply_line = app.viewer_state.explorer.inline_reply_line;
    let compose_anchor_end = new_comment_anchor_end(app);
    let mut lines: Vec<Line> = Vec::with_capacity(inner_height);
    let mut screen_row_map: Vec<crate::viewer::ScreenRow> = Vec::with_capacity(inner_height);
    let mut remaining = inner_height;

    let line_ctx = FileLineRenderCtx {
        vs,
        theme,
        tab_width,
        area_width: area.width,
        gutter_width,
        diff_annotations,
        comment_lines: &comment_lines,
        comment_end_lines: &comment_end_lines,
    };

    // 畳まれた行を飛ばして可視行だけを描く。どの行が画面に出るかを決めるのは
    // FoldState ひとつで、ここは受け取った並びをそのまま流すだけ。
    let total_lines = vs.content.file_content.len();
    for line_1 in vs
        .content
        .folds
        .visible_from(vs.content.file_scroll + 1, total_lines)
    {
        if remaining == 0 {
            break;
        }
        let line_no = line_1 - 1;
        let content = &vs.content.file_content[line_no];

        let rows = render_code_line_rows(
            app,
            &line_ctx,
            line_no,
            content,
            expanded_threads,
            inline_reply_line,
            compose_anchor_end,
        );
        for (line, row_type) in rows {
            if remaining == 0 {
                break;
            }
            lines.push(line);
            screen_row_map.push(row_type);
            remaining -= 1;
        }
    }

    // screen_row_map を app に格納するのは、vs の借用がすべて終わった後（下記参照）。

    // パンくずバーをブロック内の先頭行として先頭に追加する。
    let mut all_lines = Vec::new();
    if tab_row_height > 0 {
        // タブ行は別ウィジェットとして重ねるので、ここでは場所だけ空ける。
        all_lines.push(Line::default());
    }
    if let Some(crumb_line) = breadcrumb_visible {
        all_lines.push(crumb_line);
    }
    all_lines.extend(lines);

    // スクロール時に古いコンテンツが残らないよう、先に area をクリアする。
    frame.render_widget(ratatui::widgets::Clear, area);

    let paragraph = Paragraph::new(all_lines).block(block);
    frame.render_widget(paragraph, area);

    // ファイルの行数がパネルに収まりきらない場合にスクロールバーを描画する —
    // トリガーも見た目も Explorer のファイルツリーと同じ。
    // 尺もつまみも可視行で数える。ファイルの総行数のままだと、畳んだ直後に
    // つまみだけが縮まずに残り、どれだけ隠れているのか読めなくなる。
    let visible_total = vs.visible_line_count();
    if visible_total > inner_height {
        let mut scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        // トラックをパンくず行より下に置き、コード領域だけをカバーするようにする。
        let head_rows = breadcrumb_height + tab_row_height;
        scrollbar_area.y += head_rows;
        scrollbar_area.height = scrollbar_area.height.saturating_sub(head_rows);
        let mut scrollbar_state = ScrollbarState::new(visible_total.saturating_sub(inner_height))
            .position(vs.cursor_visible_index());
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    // 選択ヒントのオーバーレイを表示する。
    if let Some((start, end)) = vs.selected_range() {
        let hint = if start == end {
            format!(" L{start} selected \u{2502} c: comment  Esc: clear ")
        } else {
            format!(" L{start}-L{end} selected \u{2502} c: comment  Esc: clear ")
        };
        let hint_width = hint.len().min(area.width.saturating_sub(2) as usize) as u16;
        let y = area.y + area.height.saturating_sub(2);
        let hint_area = Rect::new(area.x + 1, y, hint_width, 1);
        frame.render_widget(ratatui::widgets::Clear, hint_area);
        let hint_widget = Paragraph::new(Span::styled(
            hint,
            Style::default()
                .fg(theme.gutter_selected_fg)
                .bg(theme.gutter_selected_bg),
        ));
        frame.render_widget(hint_widget, hint_area);
    }

    // 検索入力のオーバーレイを表示する（全体オーバーレイに覆われている間はカーソル配置をしない）。
    if vs.search.search_active {
        render_search_box(
            frame,
            area,
            &vs.search.search_query,
            theme,
            app.is_any_overlay_active(),
        );
    }

    // マウスイベント処理用に画面行マッピングを保存する。
    // vs（&app.viewer_state）の借用がすべて終わった後でなければならない。
    //
    // パンくずバーは内部の先頭行を占めるがコード行ではなく、screen_row_map には
    // 含まれていなかったので、その下のすべての行がマップ上で1行ずつ上にずれて
    // いた（クリック/ホバーが1行ずれて着地する）。描画内容とマップが1対1で
    // 揃うよう、選択不可のプレースホルダーを挿入する。
    if breadcrumb_height > 0 {
        screen_row_map.insert(0, crate::viewer::ScreenRow::ThreadContent);
    }
    if tab_row_height > 0 {
        screen_row_map.insert(0, crate::viewer::ScreenRow::ThreadContent);
    }
    app.viewer_state.content.screen_row_map = screen_row_map;

    render_tab_row(frame, area, app);
}

/// ブロック内側の先頭行にタブ行を重ね、クリック領域を記録する。
pub(super) fn render_tab_row(frame: &mut Frame, area: Rect, app: &mut App) {
    if !tab_row::is_visible(&app.viewer_state) || area.width < 3 || area.height < 3 {
        return;
    }
    let row = Rect::new(area.x + 1, area.y + 1, area.width - 2, 1);
    app.viewer_state.tab_row_hits =
        tab_row::render(frame, row, &app.theme, &app.viewer_state);
}

/// Viewer のタイトルを max_w **表示カラム数**に収める。左側から省略し
/// （" …tail "）、パスの中で情報量の多い末尾（ファイル名）を残す。
///
/// バイト数や文字数ではなく表示カラム数で測る: CJK のファイル名は1文字あたり
/// 2カラムを消費するが、バイト数では3バイトになる。そのため、バイトや文字数
/// を基準に切り詰めると、想定より長いタイトルになり、右側のヘッダーコントロール
/// （[Raw|Rendered] トグルと [<=>]）に重なってしまう。" … " の幅を下回る
/// 場合は意味のある内容が収まらないので、重なるくらいならタイトルの行そのものを
/// 明け渡す。
pub(super) fn fit_title(raw: &str, max_w: usize) -> String {
    if display_width(raw) <= max_w {
        return raw.to_string();
    }
    if max_w < 4 {
        return " ".repeat(max_w);
    }
    // 先頭のスペース＋省略記号＋末尾のスペースで3カラムを消費する。
    let budget = max_w - 3;
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0usize;
    for ch in raw.trim_end().chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > budget {
            break;
        }
        tail.push(ch);
        used += cw;
    }
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    format!(" \u{2026}{tail} ")
}

fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// ジャンプ履歴＋現在位置からパンくずの Line を組み立てる。
/// エントリが2件未満（ナビゲーションが起きていない）場合は None を返す。
fn build_breadcrumb_line(app: &App) -> Option<Line<'static>> {
    let current_file = app.viewer_state.content.current_file.as_ref()?;
    let current = crate::jump_history::Location {
        file_path: current_file.clone(),
        line: app.viewer_state.content.file_scroll,
        h_scroll: app.viewer_state.content.h_scroll,
    };

    let (entries, cur_idx) = app.code_nav.history.breadcrumb_trail(&current, 7);

    // 現在のエントリしかない（ナビゲーションが起きていない）場合はパンくずを表示しない。
    let real_count = entries.iter().filter(|e| e.is_some()).count();
    if real_count <= 1 {
        return None;
    }

    let theme = &app.theme;
    let separator = Span::styled(" \u{203a} ", Style::default().fg(theme.muted)); // " › "
    let mut spans: Vec<Span<'static>> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(separator.clone());
        }
        match entry {
            None => {
                // 切り詰められた古いエントリを表す省略記号。
                spans.push(Span::styled("\u{2026}", Style::default().fg(theme.muted)));
            }
            Some(loc) => {
                let label = breadcrumb_label(loc);
                let style = if i == cur_idx {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.muted)
                };
                spans.push(Span::styled(label, style));
            }
        }
    }

    // 左側に小さな余白を追加する。
    spans.insert(0, Span::raw(" "));
    Some(Line::from(spans))
}

/// 位置情報を短いパンくずラベル「ファイル名:行番号」として整形する。
fn breadcrumb_label(loc: &crate::jump_history::Location) -> String {
    let filename = loc.file_path.rsplit('/').next().unwrap_or(&loc.file_path);
    format!("{}:{}", filename, loc.line + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ヘッダーコントロールが依存している不変条件: 返り値がその列の予算を
    /// 超えることは決してない。それより幅が広ければ、同じ行を共有するトグルや
    /// [<=>] ボタンに重なってしまう。
    #[test]
    fn fitted_title_never_exceeds_its_budget() {
        let titles = [
            " src/main.rs ",
            " 設計メモ.md ",
            " docs/日本語/とても長い名前のファイル.markdown ",
            " a.rs ",
            " 🦀🦀🦀/emoji.md ",
            " ",
        ];
        for t in titles {
            for max_w in 0..40usize {
                let fitted = fit_title(t, max_w);
                assert!(
                    display_width(&fitted) <= max_w,
                    "title={t:?} max_w={max_w} -> {fitted:?} ({} cols)",
                    display_width(&fitted)
                );
            }
        }
    }

    #[test]
    fn short_titles_pass_through_untouched() {
        assert_eq!(fit_title(" a.rs ", 20), " a.rs ");
        // ちょうど予算に収まる場合もそのまま通る。
        assert_eq!(fit_title(" a.rs ", 6), " a.rs ");
    }

    /// 末尾を残す形で省略する。ファイル名はパスの末尾にあるため。
    #[test]
    fn elision_keeps_the_end_of_the_path() {
        let fitted = fit_title(" src/ui/viewer_panel/file_view.rs ", 16);
        assert!(fitted.starts_with(" \u{2026}"), "{fitted:?}");
        assert!(fitted.ends_with("file_view.rs "), "{fitted:?}");
    }

    /// 幅広グリフのタイトルはカラム数で予算配分されるので、バイト数や文字数から
    /// 想定するより*早く*省略される — これがかつてヘッダーコントロールに
    /// はみ出していたケース。
    #[test]
    fn wide_glyphs_are_budgeted_by_column() {
        // CJK 4文字 = 8カラムだが、12バイト。
        let fitted = fit_title(" 設計メモ.md ", 10);
        assert!(display_width(&fitted) <= 10, "{fitted:?}");
        assert!(fitted.contains('\u{2026}'), "{fitted:?}");
    }
}
