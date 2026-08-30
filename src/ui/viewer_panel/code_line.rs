//! プレーン（diff でない）ファイルビュー用の行ごとの行構築。コード行の span に加え、
//! その下に配置されるインラインコメントスレッドや新規コメント作成ボックスの行も
//! 組み立てる。

use crate::app::App;
use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::theme::Theme;
use crate::viewer::ScreenRow;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::comment_thread::{build_inline_compose_lines, build_inline_thread_lines};
use super::span_utils::{apply_hint_labels, apply_underline_range, h_scroll_spans};
use super::syntax::{merge_syntax_with_inline, render_inline_diff_spans, syntax_spans_for_line};

/// プレーンファイルビューのコード行を描画するためのフレーム共有コンテキスト。
pub(super) struct FileLineRenderCtx<'a> {
    pub(super) vs: &'a crate::viewer::ViewerState,
    pub(super) theme: &'a Theme,
    pub(super) tab_width: usize,
    pub(super) area_width: u16,
    pub(super) gutter_width: usize,
    pub(super) diff_annotations:
        &'a std::collections::HashMap<usize, (DiffLineTag, Vec<InlineSegment>)>,
    pub(super) comment_lines: &'a std::collections::HashSet<usize>,
    pub(super) comment_end_lines: &'a std::collections::HashSet<usize>,
}

/// ソース1行ぶんの行を組み立てる: コード行そのものに加え、その下に配置される
/// インラインコメントスレッドや新規コメント作成ボックスの行も含む。
#[allow(clippy::too_many_arguments)]
pub(super) fn render_code_line_rows(
    app: &App,
    ctx: &FileLineRenderCtx,
    line_no: usize,
    content: &str,
    expanded_threads: &std::collections::HashSet<usize>,
    inline_reply_line: Option<usize>,
    compose_anchor_end: Option<usize>,
) -> Vec<(Line<'static>, ScreenRow)> {
    let vs = ctx.vs;
    let theme = ctx.theme;
    let tab_width = ctx.tab_width;
    let gutter_width = ctx.gutter_width;
    let icon_set = app.config.ui.icon_set();

    let line_1 = line_no + 1;
    let is_selected = vs.is_line_selected(line_1);
    let is_hovered = vs.click.hover_line == Some(line_1);
    let is_gutter_hovered = vs.click.hover_gutter_line == Some(line_1);

    // diff ガターのマーカー。
    let annotation = ctx.diff_annotations.get(&line_1);
    let diff_tag = annotation.map(|(tag, _)| *tag);
    let (gutter_prefix, gutter_bg) = match diff_tag {
        Some(DiffLineTag::Insert) => ("+", Some(app.theme.diff_add_bg)),
        Some(DiffLineTag::Delete) => ("-", None),
        _ => (" ", None),
    };

    // ガター（行番号）。末尾の空白は折りたたみマーカーとの間の隙間。
    let num = format!("{gutter_prefix}{line_1:>gutter_width$} ");
    let is_grep_highlight = vs.content.grep_highlight_line == Some(line_1);
    let gutter_style = if is_selected {
        Style::default()
            .fg(theme.gutter_selected_fg)
            .bg(theme.gutter_selected_bg)
            .add_modifier(Modifier::BOLD)
    } else if is_grep_highlight {
        Style::default()
            .fg(theme.search_current_fg)
            .bg(theme.search_match_bg)
            .add_modifier(Modifier::BOLD)
    } else if is_gutter_hovered {
        Style::default()
            .fg(theme.gutter_hover_fg)
            .bg(theme.gutter_hover_bg)
    } else if is_hovered {
        Style::default().fg(theme.gutter_hover_fg)
    } else if diff_tag == Some(DiffLineTag::Insert) {
        Style::default().fg(theme.diff_add)
    } else if diff_tag == Some(DiffLineTag::Delete) {
        Style::default().fg(theme.diff_del)
    } else {
        Style::default().fg(theme.muted)
    };
    let gutter_span = Span::styled(num, gutter_style);

    // 折りたたみマーカー。マウスが乗っている間は、その1列を罫線として使って
    // 範囲のどこからどこまでかを見せる。入れ子のブロックのマーカーは残す。
    let folds = &vs.content.folds;
    let own_glyph = if folds.is_collapsed(line_1) {
        Some(crate::icons::expand_arrow(false, icon_set))
    } else if folds.is_foldable(line_1) {
        Some(crate::icons::expand_arrow(true, icon_set))
    } else {
        None
    };
    let accent = gutter_style.fg(theme.accent);
    let (fold_glyph, fold_style) = match (folds.hover_rule(line_1), own_glyph) {
        (Some(crate::viewer::FoldRule::Tail), _) => ("\u{2570}", accent),
        (Some(_), Some(g)) => (g, accent.add_modifier(Modifier::BOLD)),
        (Some(_), None) => ("\u{2502}", accent),
        (None, Some(g)) if folds.is_collapsed(line_1) => (g, accent.add_modifier(Modifier::BOLD)),
        (None, Some(g)) => (g, gutter_style.fg(theme.hint)),
        (None, None) => (" ", gutter_style),
    };
    let fold_span = Span::styled(fold_glyph, fold_style);
    let separator_span = Span::styled(" \u{2502} ", gutter_style);

    // コメントマーカー列（行番号より前、一番左）。上から順に:
    // コメント範囲の最終行、範囲の途中、選択範囲の終端、そしてガターに hover して
    // いる行のコメント開始ボタン。押せるものを行の左端に揃えてあるのは、GitHub や
    // VSCode と同じ位置に手が伸びるようにするためである。
    //
    // 選択範囲の終端マーカーはコメントのマーカーより優先度が低い。選択は一時的な
    // 状態なので、既にそこにあるコメントを隠してまで出すものではない。
    let range_end = vs
        .selected_range()
        .filter(|(start, end)| end > start)
        .is_some_and(|(_, end)| end == line_1);
    let accent_marker = Style::default().fg(theme.accent);
    let marker = if ctx.comment_end_lines.contains(&line_1) {
        Span::styled(crate::icons::COMMENT.labeled(icon_set), accent_marker)
    } else if ctx.comment_lines.contains(&line_1) {
        Span::styled(crate::icons::COMMENT_SPAN.labeled(icon_set), accent_marker)
    } else if range_end {
        Span::styled(crate::icons::RANGE_END.labeled(icon_set), accent_marker)
    } else if is_gutter_hovered {
        Span::styled(
            crate::icons::ADD_COMMENT.labeled(icon_set),
            accent_marker.add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    };

    // バッジ列（行番号の右）: 実行可能なテスト行の再生ボタン。クリックすると
    // テストコマンド（go test …や cargo test …）を Shell の PTY に送る
    // （event/mouse.rs で処理）。
    let badge = if vs.content.test_runs.contains_key(&line_1) {
        Span::styled(
            crate::icons::RUN_TEST.labeled(icon_set),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    };

    // コンテンツのスタイリング。
    let is_match = vs.search.search_matches.contains(&line_no);
    let is_current_match =
        vs.search.search_matches.get(vs.search.search_match_idx) == Some(&line_no);

    let content_spans: Vec<Span> = if is_current_match {
        vec![Span::styled(
            content.to_string(),
            Style::default()
                .fg(theme.search_current_fg)
                .bg(theme.search_match_bg),
        )]
    } else if is_match {
        vec![Span::styled(
            content.to_string(),
            Style::default()
                .fg(theme.search_match_fg)
                .add_modifier(Modifier::BOLD),
        )]
    } else if is_selected {
        vec![Span::styled(
            content.to_string(),
            Style::default()
                .bg(theme.line_selected_bg)
                .fg(theme.line_selected_fg),
        )]
    } else if let Some((ann_tag, ann_segments)) = annotation {
        if !ann_segments.is_empty() {
            // 単語単位の diff: 各セグメントを適切な背景色で描画する。
            let (diff_bg, emphasis_bg) = match ann_tag {
                DiffLineTag::Insert => (app.theme.diff_add_bg, app.theme.diff_add_bg_emphasis),
                DiffLineTag::Delete => (app.theme.diff_del_bg, app.theme.diff_del_bg_emphasis),
                _ => (Color::Reset, Color::Reset),
            };

            if *ann_tag == DiffLineTag::Insert {
                vs.content
                    .highlighted_lines
                    .get(line_no)
                    .filter(|t| !t.is_empty())
                    .and_then(|tokens| {
                        merge_syntax_with_inline(
                            ann_segments,
                            tokens,
                            diff_bg,
                            emphasis_bg,
                            tab_width,
                        )
                    })
                    .unwrap_or_else(|| syntax_spans_for_line(vs, line_no, Some(diff_bg), theme.fg))
            } else {
                render_inline_diff_spans(ann_segments, diff_bg, emphasis_bg, theme.fg, tab_width)
            }
        } else {
            // 行単位の diff のみ: diff の背景色でシンタックスハイライトを使う。
            let diff_bg = match ann_tag {
                DiffLineTag::Insert => Some(app.theme.diff_add_bg),
                DiffLineTag::Delete => Some(app.theme.diff_del_bg),
                _ => None,
            };
            syntax_spans_for_line(vs, line_no, diff_bg, theme.fg)
        }
    } else {
        syntax_spans_for_line(vs, line_no, gutter_bg, theme.fg)
    };

    // コンテンツの span に水平スクロールを適用し、パネル幅（枠線＋マーカー列＋
    // ガター＋バッジ）でクリップする。
    let content_max_w = (ctx.area_width as usize).saturating_sub(
        crate::viewer::COMMENT_MARKER_W as usize + gutter_width + crate::viewer::GUTTER_FIXED_W + 4,
    );
    let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

    // ジャンプ用の下線を適用する（ジャンプ可能なシンボル上でのホバーであれば
    // 表示され、Cmd/Ctrl が押されているかどうかで色が変わる — hover_symbol は
    // tick_underline_hover がジャンプ可能だと確認した後にしか存在しないので、
    // ジャンプ不可な単語に下線を引かないという扱いは hover_symbol が None である
    // ことによってすでに満たされている）。
    let content_spans = if let Some(ref hs) = vs.click.hover_symbol {
        if hs.line == line_1 {
            // hover_symbol はジャンプ可能だとすでに確認済みのシンボルに対してしか
            // 存在しない（tick_underline_hover を参照）ので、ここは常に Some になる —
            // unwrap_or はこの不変条件が将来崩れた場合の panic 回避のためだけにある。
            let color = match crate::app::underline_color_kind(true, hs.has_jump_modifier)
                .unwrap_or(crate::app::UnderlineColorKind::Hint)
            {
                crate::app::UnderlineColorKind::Hint => theme.hint,
                crate::app::UnderlineColorKind::Accent => theme.accent,
            };
            apply_underline_range(
                content_spans,
                hs.start_col,
                hs.end_col,
                vs.content.h_scroll,
                color,
            )
        } else {
            content_spans
        }
    } else {
        content_spans
    };

    // 上の下線とは独立して、ホバー情報ポップアップ自身の対象シンボルは、ポップアップが
    // 表示されている間はハイライトし続ける — マウスはすでにそこから離れている
    // かもしれないし（下線側には離脱猶予がない）、ポップアップ自身の離脱猶予の
    // ウィンドウ内で別の場所を指しているかもしれない。
    let content_spans = match crate::app::popup_highlight_range(
        app.code_nav.hover_info.is_shown(),
        app.code_nav.hover_info.target_line,
        app.code_nav.hover_info.target_start_col,
        app.code_nav.hover_info.target_end_col,
        line_1,
    ) {
        Some((start, end)) => {
            apply_underline_range(content_spans, start, end, vs.content.h_scroll, theme.accent)
        }
        None => content_spans,
    };

    // シンボルのヒントラベルを適用する（Vimium 風）。
    let content_spans = if app.code_nav.symbol_hint.active {
        let hints_on_line: Vec<_> = app
            .code_nav
            .symbol_hint
            .hints
            .iter()
            .filter(|h| h.line == line_1)
            .collect();
        if hints_on_line.is_empty() {
            content_spans
        } else {
            apply_hint_labels(
                content_spans,
                &hints_on_line,
                &app.code_nav.symbol_hint.input,
                vs.content.h_scroll,
                theme,
            )
        }
    } else {
        content_spans
    };

    // 畳んだ行は、隠れている行数を見出し行の末尾に出す。畳んだこと自体は
    // マーカーで分かるが、どれだけ隠れているかはここでしか分からない。
    let content_spans = match vs.content.folds.hidden_count(line_1) {
        Some(n) => {
            let mut spans = content_spans;
            let unit = if n == 1 { "line" } else { "lines" };
            spans.push(Span::styled(
                format!(" \u{22ef} {n} {unit}"),
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            ));
            spans
        }
        None => content_spans,
    };

    let mut spans = vec![marker, gutter_span, fold_span, separator_span, badge];
    spans.extend(content_spans);

    let mut rows: Vec<(Line<'static>, ScreenRow)> =
        vec![(Line::from(spans), ScreenRow::Code(line_1))];

    // コメント範囲の最終行の下にインラインスレッドの行を追加する。
    if expanded_threads.contains(&line_1) {
        let reply_cid = if inline_reply_line == Some(line_1) {
            app.viewer.inline.reply_comment_id.as_deref()
        } else {
            None
        };
        let thread_lines = build_inline_thread_lines(
            line_1,
            gutter_width,
            ctx.area_width as usize,
            &app.review_state,
            reply_cid,
            &app.viewer.inline.reply_buffer,
            theme,
            &app.highlight.syntax_set,
            &app.highlight.theme,
            &app.markdown_cache,
            icon_set,
        );
        rows.extend(thread_lines);
    }

    // 新規コメント作成ボックスを、それが紐づく行の下に追加する。
    if compose_anchor_end == Some(line_1) {
        let compose = build_inline_compose_lines(
            app.review_state.input_kind,
            &app.review_state.input_buffer,
            gutter_width,
            ctx.area_width as usize,
            theme,
        );
        rows.extend(compose);
    }

    rows
}
