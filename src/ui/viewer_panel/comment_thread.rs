//! インラインのレビューコメントスレッド描画: コメント付きの行の下に表示される
//! 展開済みスレッドボックス、返信、コメントごとのアクション行、新規コメント
//! 作成ボックス。

use crate::app::App;
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::thread_actions;

/// 指定行のコメントに対するインラインスレッド行を組み立てる。
///
/// スレッドボックスを表す Line の列を返す:
/// 上枠線、各コメント + 返信 + アクションアイコン、下枠線。
#[allow(clippy::too_many_arguments)]
pub(super) fn build_inline_thread_lines<'a>(
    line_1: usize,
    gutter_width: usize,
    panel_width: usize,
    review_state: &crate::review_state::ReviewState,
    reply_comment_id: Option<&str>,
    reply_buffer: &crate::text_input::TextInput,
    theme: &Theme,
    syntax_set: &syntect::parsing::SyntaxSet,
    syntect_theme: &syntect::highlighting::Theme,
    md_cache: &crate::ui::markdown::MarkdownCache,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    // 展開されたスレッドは resolved を含む全コメントを表示する。resolved なコメントは
    // あくまで「デフォルトで」折りたたまれているだけなので（expand_threads_for_file 参照）、
    // ユーザがバッジをクリックして開いたら（byline に "resolved" マーカーを付けて）
    // 表示しなければならない。さもないと箱が空のまま描画されてしまう。
    let comments: Vec<&crate::review_store::ReviewComment> =
        match review_state.file_comments.get(&line_1) {
            Some(c) if !c.is_empty() => c.iter().collect(),
            _ => return Vec::new(),
        };

    use crate::viewer::ScreenRow;
    let mut out: Vec<(Line, ScreenRow)> = Vec::new();
    let left_pad = crate::viewer::COMMENT_MARKER_W as usize + gutter_width + crate::viewer::GUTTER_FIXED_W + 2; // マーカー + ガター + バッジ
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    // 執筆者ごとに面の色味を変え、「誰が書いたか」を一目でわかるようにする。Claude の
    // コメント/返信はニュートラルな面、ユーザのものは別の色味の面に載せる。
    let author_bg = |a: crate::review_store::Author| match a {
        crate::review_store::Author::Claude => theme.comment_preview_bg,
        crate::review_store::Author::User => theme.comment_user_bg,
    };
    // ボックス内側の幅（│ と │ の間）。
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2); // 左の "  │ " + 右の " │"
    // ボックス内のインデント。ただしボックス自体より広くはしない（固定下限の 20 は、
    // 狭いパネルで枠線をはみ出していたため）。
    let wrap_width = box_inner.saturating_sub(6).max(10).min(box_inner.max(1));

    // ヘルパー: 左に │ を持ち、bg で全幅を埋めた枠付きコンテンツ行。
    let make_line = |spans: Vec<Span<'a>>, bg: Color| -> (Line<'a>, ScreenRow) {
        let bg_style = Style::default().bg(bg);
        let mut all = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled("  │ ", border_style),
        ];
        all.extend(spans);
        // 背景色が行全体を埋めるよう、panel_width までパディングする。
        let used: usize = all
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let remaining = panel_width.saturating_sub(used + 2); // +2 はブロックの枠線分
        if remaining > 0 {
            all.push(Span::styled(" ".repeat(remaining), bg_style));
        }
        (Line::from(all).style(bg_style), ScreenRow::ThreadContent)
    };

    // ヘルパー: bg で塗った全幅の枠線行。
    let make_border = |content: String, bg: Color| -> (Line<'a>, ScreenRow) {
        let bg_style = Style::default().bg(bg);
        let text = format!("{gutter_pad}{content}");
        let used = unicode_width::UnicodeWidthStr::width(text.as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        (Line::from(spans).style(bg_style), ScreenRow::ThreadContent)
    };

    // 上枠線 — 最初のコメントの執筆者の色味で塗る。
    let top_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(
        format!("  ┌{top_fill}┐"),
        author_bg(comments[0].author),
    ));

    for (ci, comment) in comments.iter().enumerate() {
        // このコメントの執筆者の面。全ての行がこれを使う。
        let cbg = author_bg(comment.author);
        let content_style = Style::default().fg(theme.fg).bg(cbg);
        let info_style = Style::default().fg(theme.info).bg(cbg);

        // 同じスレッド内のコメント間の空行スペーサー。
        if ci > 0 {
            out.push(make_line(vec![Span::styled("", content_style)], cbg));
        }

        let author_label = match comment.author {
            crate::review_store::Author::User => "you",
            crate::review_store::Author::Claude => "claude",
        };

        // 執筆者の byline: 種別バッジ（💡/❓）+ 執筆者名。GitHub のコメントヘッダーに
        // 似た形。resolved なコメントには byline の末尾に控えめな「✓ resolved」マーカーが
        // 付く（これはスレッドを明示的に開いたときのみ表示される）。
        let kind = crate::ui::review::kind_icon(comment.kind);
        let mut byline = vec![
            Span::styled(format!("{kind} "), content_style),
            Span::styled(
                author_label.to_string(),
                info_style.add_modifier(Modifier::BOLD),
            ),
        ];
        if comment.status == crate::review_store::CommentStatus::Resolved {
            byline.push(Span::styled(
                "  \u{2713} resolved".to_string(),
                Style::default().fg(theme.success).bg(cbg),
            ));
        }
        out.push(make_line(byline, cbg));

        // コメント本文。執筆者の面に GitHub 風の Markdown として描画する
        // （見出し、リスト、コードカード、インラインの code、リンクなど）。
        // 毎フレーム再パース/再ハイライトしないよう comment id ごとにキャッシュする。
        let mut body_md = md_cache.render(
            &comment.id,
            &comment.body,
            wrap_width,
            theme,
            syntax_set,
            syntect_theme,
        );
        crate::ui::markdown::apply_background(&mut body_md, cbg);
        for line in body_md {
            out.push(make_line(line.spans, cbg));
        }

        // キャッシュされていれば返信を表示する。それぞれ自分自身の執筆者の色味で塗るので、
        // Claude のコメントへのユーザの返信（あるいはその逆）が見た目で区別できる。
        if let Some(replies) = review_state.cached_replies.get(&comment.id) {
            for reply in replies {
                let rbg = author_bg(reply.author);
                let r_content = Style::default().fg(theme.fg).bg(rbg);
                let r_info = Style::default().fg(theme.info).bg(rbg);
                let reply_author = match reply.author {
                    crate::review_store::Author::User => "you",
                    crate::review_store::Author::Claude => "claude",
                };
                // 返信の byline。親コメントの下に ↳ マーカー付きでインデントする。
                out.push(make_line(
                    vec![Span::styled(
                        format!("  \u{21b3} {reply_author}"),
                        r_info.add_modifier(Modifier::BOLD),
                    )],
                    rbg,
                ));
                // 返信本文の Markdown。byline の下に2列分インデントする。reply id ごとにキャッシュする。
                let mut reply_md = md_cache.render(
                    &reply.id,
                    &reply.body,
                    wrap_width.saturating_sub(2).max(1),
                    theme,
                    syntax_set,
                    syntect_theme,
                );
                crate::ui::markdown::apply_background(&mut reply_md, rbg);
                for line in reply_md {
                    let mut spans = vec![Span::styled("  ".to_string(), r_content)];
                    spans.extend(line.spans);
                    out.push(make_line(spans, rbg));
                }
            }
        }

        // コメントごとのアクションアイコン行、または返信入力中の表示。
        let is_replying_to_this = reply_comment_id == Some(comment.id.as_str());
        let action_row_type = ScreenRow::ThreadActions {
            comment_id: comment.id.clone(),
        };
        if is_replying_to_this {
            // GitHub 風の複数行返信フォーム: byline、ブロックカーソル付きで1行ずつ描画される
            // バッファ、そしてキーヒント。上のスレッドは表示されたままなので、入力中も
            // 親コメントが常に見える。
            let muted = Style::default().fg(theme.muted).bg(cbg);
            out.push(make_line(
                vec![Span::styled(
                    "\u{21b3} reply".to_string(),
                    info_style.add_modifier(Modifier::BOLD),
                )],
                cbg,
            ));
            if reply_buffer.is_empty() {
                out.push(make_line(
                    vec![
                        Span::styled("> ".to_string(), Style::default().fg(theme.accent).bg(cbg)),
                        Span::styled("Type reply\u{2026}".to_string(), muted),
                    ],
                    cbg,
                ));
            } else {
                // モーダルと同様、ブロックカーソルは前後のテキストの間に置く。
                let display = format!(
                    "{}\u{2588}{}",
                    reply_buffer.text_before_cursor(),
                    reply_buffer.text_after_cursor()
                );
                for (li, seg) in display.split('\n').enumerate() {
                    let prefix = if li == 0 { "> " } else { "  " };
                    out.push(make_line(
                        vec![
                            Span::styled(
                                prefix.to_string(),
                                Style::default().fg(theme.accent).bg(cbg),
                            ),
                            Span::styled(seg.to_string(), content_style),
                        ],
                        cbg,
                    ));
                }
            }
            out.push(make_line(
                vec![Span::styled(
                    "Shift+Enter: newline  \u{b7}  Enter: send  \u{b7}  Esc: cancel".to_string(),
                    muted,
                )],
                cbg,
            ));
        } else {
            // クリック可能なアクション行。ラベルとヒット範囲はどちらも共有の
            // thread_actions モジュールから来ているので、マウスハンドラは
            // ここで描画される内容と常に一致する。
            let bg_style = Style::default().bg(cbg);
            let muted_style = Style::default().fg(theme.muted).bg(cbg);
            let reply_style = Style::default().fg(theme.info).bg(cbg);
            let resolve_style = Style::default().fg(theme.success).bg(cbg);
            let delete_style = Style::default().fg(theme.error).bg(cbg);
            let claude_style = Style::default().fg(Color::Rgb(180, 140, 255)).bg(cbg);
            let status_label = match comment.status {
                crate::review_store::CommentStatus::Pending => thread_actions::RESOLVE,
                crate::review_store::CommentStatus::Resolved => thread_actions::UNRESOLVE,
            };
            // resolve/unresolve のどちらが表示されても "delete" が常に同じ列から
            // 始まるよう、status のスロットを一定幅にパディングする。
            let status_pad = thread_actions::status_slot_width()
                .saturating_sub(unicode_width::UnicodeWidthStr::width(status_label));

            let gap = " ".repeat(thread_actions::GAP);
            let left_actions = vec![
                Span::styled(thread_actions::REPLY, reply_style),
                Span::styled(gap.clone(), muted_style),
                Span::styled(
                    format!("{status_label}{}", " ".repeat(status_pad)),
                    resolve_style,
                ),
                Span::styled(gap, muted_style),
                Span::styled(thread_actions::DELETE, delete_style),
            ];
            let right_label = thread_actions::ASK_CLAUDE;
            let right_label_w = thread_actions::ask_claude_width();

            let left_w: usize = left_actions
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let prefix_w = left_pad + 4; // gutter_pad + "  │ " の分
            let fill = panel_width.saturating_sub(prefix_w + left_w + right_label_w + 2 + 1);

            let mut spans = vec![
                Span::styled(gutter_pad.clone(), bg_style),
                Span::styled("  │ ", border_style),
            ];
            spans.extend(left_actions);
            if fill > 0 {
                spans.push(Span::styled(" ".repeat(fill), bg_style));
            }
            spans.push(Span::styled(right_label.to_string(), claude_style));
            spans.push(Span::styled(" ", bg_style)); // 末尾のパディング

            let line = Line::from(spans).style(bg_style);
            out.push((line, action_row_type));
        }
    }

    // 下枠線 — 最後のコメントの執筆者の色味で塗る。
    let bot_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(
        format!("  └{bot_fill}┘"),
        author_bg(comments[comments.len() - 1].author),
    ));

    out
}

/// 新規コメントを作成中で、そのアンカーが現在のファイル内にある場合、新規コメント
/// 作成ボックスを描画すべき行を返す。アンカー範囲の終端行（ボックスを差し込む位置）を返す。
pub(super) fn new_comment_anchor_end(app: &App) -> Option<usize> {
    if app.review_state.input_mode != crate::review_state::ReviewInputMode::AddingComment {
        return None;
    }
    let (file, start, end) = app.review_state.input_anchor.as_ref()?;
    if Some(file.as_str()) != app.viewer_state.content.current_file.as_deref() {
        return None;
    }
    Some(end.unwrap_or(*start) as usize)
}

/// インラインの新規コメント作成ボックスを組み立てる。ReviewInputMode::AddingComment
/// が有効なとき、アンカー行の下に差し込まれる。GitHub 風のフォームで、種別ヘッダー、
/// ブロックカーソル付きの本文バッファ、キーヒントからなり、ユーザ作成コメントと同様に
/// ユーザの面（comment_user_bg）に描画する。
pub(super) fn build_inline_compose_lines<'a>(
    kind: crate::review_store::CommentKind,
    input: &crate::text_input::TextInput,
    gutter_width: usize,
    panel_width: usize,
    theme: &Theme,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    use crate::viewer::ScreenRow;
    let bg = theme.comment_user_bg;
    let left_pad = crate::viewer::COMMENT_MARKER_W as usize + gutter_width + crate::viewer::GUTTER_FIXED_W + 2;
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    let bg_style = Style::default().bg(bg);
    let content_style = Style::default().fg(theme.fg).bg(bg);
    let muted = Style::default().fg(theme.muted).bg(bg);
    let accent_bg = Style::default().fg(theme.accent).bg(bg);
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2);

    let make_line = |spans: Vec<Span<'a>>| -> (Line<'a>, ScreenRow) {
        let mut all = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled("  │ ", border_style),
        ];
        all.extend(spans);
        let used: usize = all
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let rem = panel_width.saturating_sub(used + 2);
        if rem > 0 {
            all.push(Span::styled(" ".repeat(rem), bg_style));
        }
        (Line::from(all).style(bg_style), ScreenRow::ThreadContent)
    };
    let make_border = |content: String| -> (Line<'a>, ScreenRow) {
        let used = unicode_width::UnicodeWidthStr::width(format!("{gutter_pad}{content}").as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        (Line::from(spans).style(bg_style), ScreenRow::ThreadContent)
    };

    let mut out = Vec::new();
    let top_fill = "\u{2500}".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  \u{250c}{top_fill}\u{2510}")));

    // 種別ヘッダー（Tab で切り替え）。
    let (icon, label) = match kind {
        crate::review_store::CommentKind::Suggest => ("\u{1f4a1}", "New Suggest"),
        crate::review_store::CommentKind::Question => ("\u{2753}", "New Question"),
    };
    out.push(make_line(vec![
        Span::styled(format!("{icon} "), content_style),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(theme.info)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (Tab: toggle kind)".to_string(), muted),
    ]));

    // 本文。前後のテキストの間にブロックカーソルを置く。
    if input.is_empty() {
        out.push(make_line(vec![
            Span::styled("> ".to_string(), accent_bg),
            Span::styled("Write a comment\u{2026}".to_string(), muted),
        ]));
    } else {
        let display = format!(
            "{}\u{2588}{}",
            input.text_before_cursor(),
            input.text_after_cursor()
        );
        for (li, seg) in display.split('\n').enumerate() {
            let prefix = if li == 0 { "> " } else { "  " };
            out.push(make_line(vec![
                Span::styled(prefix.to_string(), accent_bg),
                Span::styled(seg.to_string(), content_style),
            ]));
        }
    }

    out.push(make_line(vec![Span::styled(
        "Shift+Enter: newline  \u{b7}  Enter: submit  \u{b7}  Esc: cancel".to_string(),
        muted,
    )]));
    let bot_fill = "\u{2500}".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  \u{2514}{bot_fill}\u{2518}")));
    out
}
