//! revidere のレビュービュー。画面は 2 つあり、行き先ごとにキーが分かれている。
//!
//! - 概要 (1 列, o): 段階 1 の 5 欄と、機能への影響
//! - 項目 + diff (2 列, d): 左に読む順 (項目一覧)、右にその順で並べた diff
//!
//! 概要を 2 列側の先頭に混ぜず別画面にしているのは、読むのが最初の一度きり
//! だから。混ぜると、そのあとずっと縦を取り続ける。GitHub が PR の説明と
//! Files changed を分けているのと同じ切り分け。
//!
//! 画面全体を占有する。3 列アコーディオンの一部として狭いペインに押し込むと、
//! 項目の説明と diff を同時に読むという、このビューの唯一の用途が成立しない。
//!
//! 右の列を歩くのは diff であって項目ではない
//!
//! 行を出しているのは [revidere::ReadingOrder] で、その中のループの主語は
//! 変更一覧 (diff) の側にある。項目が漏らしても変更行は消えず、最悪でも帯の無い
//! 素の diff に退化する。ここで項目を回して「その項目が触るファイル」を出す形に
//! 書き直すと、その保証が失われる。
//!
//! 項目の先頭行は描画中に記録する
//!
//! n/N のジャンプ先は「その項目より前が出した表示行の総和」だが、本文の折り返しが
//! 幅に依存するので、幅を知らない場所では数えられない。描画のたびに
//! [crate::app::RevidereState::section_rows] へ書き込み、キー処理はそれを読む
//! (diff ペインの screen_entry_map と同じ作り)。左列も同様に、画面の行から
//! 項目を引くための対応表 (list_rows) を描画中に書く。
//!
//! 右列はキャッシュする
//!
//! syntect のハイライトと本文の折り返しは、成果物の全変更行ぶんを毎フレーム
//! やり直すには重い。幅・テーマ・成果物のどれかが変わったときだけ組み直し、
//! それ以外のフレームは組み上がった行から窓を切り出すだけにする。

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};

use syntect::easy::HighlightLines;

use revidere::Tag;

use crate::app::App;
use crate::revidere::{Review, importance_color};

/// 左列 (読む順) が取る幅の割合。revidere-view と同じ配分で、項目の見出しが
/// 2〜3 行に収まりつつ diff 側に十分な幅が残る。
const LIST_PCT: u16 = 32;

/// 機能への影響の行の字下げと、ラベル欄の幅 (表示列)。一番長い「確かめる」に
/// 合わせてある。
const IMPACT_INDENT: usize = 6;
const IMPACT_LABEL_W: usize = 8;

/// 左列の重要度ラベルが取る幅 (表示列)。一番長い「影響あり」に合わせる。
/// 揃えないと、ラベルの長さの違いだけで見出しの左端がぎざぎざになる。
const LABEL_W: usize = 8;

/// 概要の 1 列表示で本文を流し込む最大の幅。端から端まで伸びた 1 行は、
/// 折り返した先で目が戻る場所を見失う。
const READING_W: usize = 110;

/// 前回からの進みに出すファイル名の数。溢れた分は件数だけ添える。
///
/// 履歴が書き換わったあとは「別々の履歴どうしの全差分」になって数百件に
/// なりうる。全部出すと、この節の下にある概要の 5 欄が画面外へ押し出されて、
/// 先に読ませたくて先頭に置いたものが逆に読まれなくなる。
const SINCE_PREVIOUS_FILES_MAX: usize = 12;

/// 組み立て済みの右列。
///
/// key は「これが変わったら中身も変わる」入力の指紋。幅は折り返しを、
/// ハイライト世代と diff の背景色はテーマの切り替えを、版は成果物の
/// 差し替えを捕まえる (背景色まで見るのは、syntax_theme_file を明示した
/// 設定では UI テーマだけが変わっても世代が進まないため)。
pub struct DiffRender {
    key: (u16, u64, u64, Color),
    lines: Vec<Line<'static>>,
    section_rows: Vec<usize>,
}

/// レビュービューを area 全体に描画する。概要の 1 列か、項目 + diff の 2 列。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // 成果物が無い状態でここへ来ることはない (cmd_show_revidere が弾く) が、
    // worktree の切り替えで手元から消えることはある。
    let Some(review) = app.revidere.current.take() else {
        // 当たり判定を消しておく。列が無いのに残しておくと、マウスが
        // 存在しない項目を選ぶ。
        app.revidere.list_area = Rect::default();
        app.revidere.diff_area = Rect::default();
        app.revidere.list_rows.clear();
        // どちらの区間が無いのかを言う。伏せたままだと、p で切り替えた先が
        // 未解析なだけなのに「レビューが消えた」ように読める。
        frame.render_widget(
            Paragraph::new(format!(
                "[{}] のレビューはまだ無い — W で解析、p でもう一方の区間へ。",
                crate::revidere::scope_label(app.revidere.scope)
            ))
            .style(Style::default().fg(app.appearance.theme.warning))
            .block(bordered(" Review ", app)),
            area,
        );
        return;
    };

    if app.revidere.show_overview {
        // 項目の当たり判定は消す。左列が無いので、残っていると概要の本文を
        // クリックしたときに見えない項目が選ばれる。
        app.revidere.list_area = Rect::default();
        app.revidere.diff_area = area;
        app.revidere.list_rows.clear();
        render_overview(frame, area, app, &review);
        app.revidere.current = Some(review);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(LIST_PCT), Constraint::Min(20)])
        .split(area);

    app.revidere.list_area = columns[0];
    app.revidere.diff_area = columns[1];

    let list_rows = render_section_list(frame, columns[0], app, &review);
    render_diff_column(frame, columns[1], app, &review);

    app.revidere.list_rows = list_rows;
    app.revidere.current = Some(review);
}

/// 概要だけを 1 列で描く。
fn render_overview(frame: &mut Frame, area: Rect, app: &mut App, review: &Review) {
    // 本文が長いので幅は取り過ぎない。端から端まで伸びた 1 行は、折り返した先で
    // 目が戻る場所を見失う。余った幅は枠の外に出して中央に置く — 枠だけ全幅で
    // 伸ばすと、右側の空きが折り返しの失敗に見える。
    let area = centered(area, READING_W as u16 + 2);
    let inner_w = area.width.saturating_sub(2) as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    push_overview(&mut lines, review, &app.appearance.theme, inner_w);

    let height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = app.revidere.overview_scroll.min(max_scroll);
    app.revidere.overview_scroll = scroll;
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();

    let title = format!(
        " 概要 [{}]  {}..作業ツリー  (d: 項目と diff へ / p: 区間を切り替え) ",
        crate::revidere::scope_label(app.revidere.scope),
        review.base
    );
    frame.render_widget(Paragraph::new(visible).block(bordered(&title, app)), area);
}

/// area を横方向に max_w まで狭めて中央に置く。狭ければそのまま返す。
fn centered(area: Rect, max_w: u16) -> Rect {
    if area.width <= max_w {
        return area;
    }
    Rect {
        x: area.x + (area.width - max_w) / 2,
        width: max_w,
        ..area
    }
}

fn bordered<'a>(title: &'a str, app: &App) -> Block<'a> {
    Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(app.appearance.theme.fg)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(app.appearance.theme.border_focused))
}

/// 戻り値は「画面に出した行 → 項目の番号」。見出しの折り返しで 1 項目が何行にも
/// なるので、クリックした行から割り算では引けない。
fn render_section_list(frame: &mut Frame, area: Rect, app: &App, review: &Review) -> Vec<usize> {
    let theme = &app.appearance.theme;
    let inner_w = area.width.saturating_sub(4) as usize;
    let sections = review.annotations.sections();

    let mut items: Vec<ListItem> = Vec::new();
    let mut section_of_row: Vec<usize> = Vec::new();
    let mut row_of_section: Vec<usize> = Vec::with_capacity(review.order.sections.len());
    for (i, placed) in review.order.sections.iter().enumerate() {
        row_of_section.push(items.len());
        let indent = "  ".repeat(placed.depth);
        let (label, color) = match placed.importance {
            Some(imp) => (imp.label_ja(), importance_color(imp)),
            // どの項目でも説明されていない変更。末尾にまとまる。
            None => ("説明なし", theme.muted),
        };
        let title = placed
            .section
            .and_then(|s| sections.get(s))
            .map(|s| s.title.as_str())
            .unwrap_or("(どの項目でも説明されていない変更)");

        let selected = i == app.revidere.selected;
        let title_style = if selected {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        // 見出しは折り返して全部出す。切ると、似た書き出しの項目が見分けられない。
        let pad = " ".repeat(LABEL_W.saturating_sub(unicode_width::UnicodeWidthStr::width(label)));
        let width = inner_w.saturating_sub(indent.len() + LABEL_W + 2);
        for (n, chunk) in wrap(title, width.max(8)).into_iter().enumerate() {
            let head = if n == 0 {
                format!("{indent}▌{label}{pad} ")
            } else {
                format!("{indent}▌{} ", " ".repeat(LABEL_W))
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(head, Style::default().fg(color)),
                Span::styled(chunk, title_style),
            ])));
            section_of_row.push(i);
        }
        // 項目が在るのに指す行が diff に 1 つも無い状態。黙って消すと
        // 「在ると言った変更が無かった」ことに気付けない。
        if placed.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("{indent}   (この項目が指す変更が diff に無い)"),
                Style::default().fg(theme.warning),
            ))));
            section_of_row.push(i);
        }
    }

    let height = area.height.saturating_sub(2) as usize;
    let anchor = row_of_section
        .get(app.revidere.selected)
        .copied()
        .unwrap_or(0);
    let scroll = anchor
        .saturating_sub(height / 3)
        .min(items.len().saturating_sub(1));
    let visible: Vec<ListItem> = items.into_iter().skip(scroll).take(height).collect();

    // 概要は別画面なので、そこへ戻る道は枠題に書いておかないと分からない。
    frame.render_widget(
        List::new(visible).block(bordered(" 読む順  (o: 概要へ) ", app)),
        area,
    );

    section_of_row
        .into_iter()
        .skip(scroll)
        .take(height)
        .collect()
}

/// 右列を描く。組み上がった行はキャッシュし、変わっていなければ窓を切り出す
/// だけにする。
fn render_diff_column(frame: &mut Frame, area: Rect, app: &mut App, review: &Review) {
    let key = (
        area.width,
        app.appearance.highlight.generation,
        app.revidere.epoch,
        app.appearance.theme.diff_add_bg,
    );

    let mut cache = app.revidere.diff_cache.take();
    if cache.as_ref().map(|c| c.key) != Some(key) {
        let (lines, section_rows) = build_diff_lines(app, area, review);
        cache = Some(DiffRender {
            key,
            lines,
            section_rows,
        });
    }
    let cache = cache.expect("diff cache is filled just above");

    let height = area.height.saturating_sub(2) as usize;
    let max_scroll = cache.lines.len().saturating_sub(height);
    let scroll = app.revidere.diff_scroll.min(max_scroll);
    let visible: Vec<Line> = cache
        .lines
        .iter()
        .skip(scroll)
        .take(height)
        .cloned()
        .collect();

    let title = format!(
        " [{}] {}..作業ツリー  変更行 {}  {} ",
        crate::revidere::scope_label(app.revidere.scope),
        review.base,
        review.total_positions(),
        if review.is_complete() {
            "全部の変更行に説明あり"
        } else {
            "説明の無い変更行あり"
        }
    );
    frame.render_widget(Paragraph::new(visible).block(bordered(&title, app)), area);

    app.revidere.section_rows = cache.section_rows.clone();
    app.revidere.diff_cache = Some(cache);
}

/// 読む順そのものを 1 本の流れとして組み立てる。戻り値は行と、項目ごとの先頭行。
fn build_diff_lines(app: &App, area: Rect, review: &Review) -> (Vec<Line<'static>>, Vec<usize>) {
    let theme = &app.appearance.theme;
    let inner_w = area.width.saturating_sub(2) as usize;
    let tab_width = app.config.viewer.tab_width;
    let sections = review.annotations.sections();

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut section_rows: Vec<usize> = Vec::with_capacity(review.order.sections.len());

    for placed in &review.order.sections {
        section_rows.push(lines.len());
        let (label, color) = match placed.importance {
            Some(imp) => (imp.label_ja(), importance_color(imp)),
            None => ("説明なし", theme.muted),
        };
        let section = placed.section.and_then(|s| sections.get(s));
        let title = section
            .map(|s| s.title.as_str())
            .unwrap_or("どの項目でも説明されていない変更");

        lines.push(Line::from(Span::styled(
            format!("── {label} {title}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        if let Some(section) = section {
            for chunk in wrap(&section.body, inner_w.saturating_sub(2)) {
                lines.push(Line::from(Span::styled(
                    format!("  {chunk}"),
                    Style::default().fg(theme.fg),
                )));
            }
            // なぜその重要度なのかは全項目必須。誤分類は機械では見つからないが、
            // 理由が読めれば人が見つけられる — だから畳まずに出す。
            if let Some(reason) = &section.reason {
                let first = format!("  なぜ{label}: ");
                let indent_w = unicode_width::UnicodeWidthStr::width(first.as_str());
                for (n, chunk) in wrap(reason, inner_w.saturating_sub(indent_w))
                    .into_iter()
                    .enumerate()
                {
                    let head = if n == 0 {
                        first.clone()
                    } else {
                        " ".repeat(indent_w)
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{head}{chunk}"),
                        Style::default().fg(theme.muted),
                    )));
                }
            }
        }
        lines.push(Line::from(""));

        for block in &placed.blocks {
            let head = if block.hunk.is_empty() {
                format!("  {}", block.path)
            } else {
                format!("  {}  @@ {}", block.path, block.hunk)
            };
            lines.push(Line::from(Span::styled(
                head,
                Style::default().fg(theme.diff_section_header),
            )));
            if block.whole_file {
                // 行を持たない変更 (バイナリ、モードのみ、純粋な rename)。
                // 落とすと「変更が無かった」と区別が付かなくなる。
                lines.push(Line::from(Span::styled(
                    "   (行を持たない変更)".to_string(),
                    Style::default().fg(theme.muted),
                )));
            }
            let tokens = highlight_block(app, block, tab_width);
            for (i, ordered) in block.lines.iter().enumerate() {
                lines.push(diff_line(
                    &ordered.line,
                    ordered.owned,
                    tokens.get(i),
                    theme,
                    inner_w,
                ));
            }
            lines.push(Line::from(""));
        }
    }

    (lines, section_rows)
}

/// 畳んだり別画面にしたりしない。これを読まずに項目から読み始めると、個々の変更が
/// 何のためかが分からないまま進むことになるため。
fn push_overview(
    lines: &mut Vec<Line<'static>>,
    review: &Review,
    theme: &crate::theme::Theme,
    inner_w: usize,
) {
    let overview = review.annotations.overview();
    let head = |text: &str, color| {
        Line::from(Span::styled(
            text.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
    };

    lines.push(head("── 概要", theme.accent));
    lines.push(Line::from(""));
    push_since_previous(lines, review, theme, inner_w);
    for (key, value) in [
        ("困っていたこと", &overview.problem),
        ("やったこと", &overview.change),
        ("仕組み", &overview.mechanism),
        ("置き場所", &overview.placement),
        ("範囲", &overview.scope),
    ] {
        lines.push(head(&format!("  {key}"), theme.diff_section_header));
        for chunk in wrap(value, inner_w.saturating_sub(4)) {
            lines.push(Line::from(Span::styled(
                format!("    {chunk}"),
                Style::default().fg(theme.fg),
            )));
        }
        lines.push(Line::from(""));
    }

    let impacts = review.annotations.impacts();
    if impacts.is_empty() {
        return;
    }
    lines.push(head("  機能への影響", theme.warning));
    for impact in impacts {
        // 事実と推測を分けて出す。推測を事実の顔で出されると、確かめずに
        // 信じてしまう。
        let (tag, tag_color) = match impact.confidence {
            revidere::Confidence::Fact => ("事実", theme.diff_add),
            revidere::Confidence::Guess => ("推測", theme.diff_section_header),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("    [{tag}] "), Style::default().fg(tag_color)),
            Span::styled(
                impact.feature.clone(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
        for (label, value, color) in [
            ("変化", Some(&impact.change), theme.fg),
            ("確かめる", Some(&impact.verify), theme.muted),
            ("残る穴", impact.gap.as_ref(), theme.warning),
        ] {
            let Some(value) = value else { continue };
            push_labeled(lines, label, value, color, inner_w);
        }
        lines.push(Line::from(""));
    }
}

/// 概要の先頭に置く。2 度目以降の読者が最初に知りたいのは「前と何が違うか」で、
/// 本体を読み直すかどうかもそれで決まる。初回は何も出さない。
fn push_since_previous(
    lines: &mut Vec<Line<'static>>,
    review: &Review,
    theme: &crate::theme::Theme,
    inner_w: usize,
) {
    let Some(since) = review.annotations.since_previous() else {
        return;
    };
    // 本文でも警告でもない補足はここに寄せる。muted は背景に埋もれるテーマが
    // あり、この節は 1 行しか出ないことがあるので、消えると節ごと壊れて見える。
    let note = theme.diff_section_header;

    lines.push(Line::from(Span::styled(
        "  前回のレビューから".to_string(),
        Style::default().fg(note).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("    {} → {}", since.previous_head, since.head),
        Style::default().fg(theme.fg),
    )));
    // 履歴が変わっていたら、それを先に言う。前回のコミットが辿れない以上、
    // 下のファイル一覧は「積み上げ」ではなく「別の履歴との比較」になっている。
    if since.history_rewritten {
        push_note(
            lines,
            "前回のコミットは今の履歴から辿れない (rebase / amend / force push)。\
             下の一覧は前回との積み上げではなく、別々の履歴どうしの比較になる。",
            theme.warning,
            inner_w,
        );
    }
    match &since.files {
        // 引けなかったことを「無い」に畳まない。
        None => push_note(
            lines,
            "変わったファイルは一覧にできない (前回のコミットがもう残っていない)。",
            theme.warning,
            inner_w,
        ),
        Some(files) if files.is_empty() => {
            push_note(lines, "変わったファイルは無い", note, inner_w)
        }
        Some(files) => {
            for path in files.iter().take(SINCE_PREVIOUS_FILES_MAX) {
                lines.push(Line::from(Span::styled(
                    format!("    {path}"),
                    Style::default().fg(theme.fg),
                )));
            }
            let rest = files.len().saturating_sub(SINCE_PREVIOUS_FILES_MAX);
            if rest > 0 {
                push_note(lines, &format!("ほか {rest} 件"), note, inner_w);
            }
            // ファイル名だけでは、指摘をどう直したのかは読めない。その行き先を指す。
            push_note(
                lines,
                "p: この区間だけのレビューへ (どこがどう変わったかを読む)",
                note,
                inner_w,
            );
        }
    }
    lines.push(Line::from(""));
}

/// 概要の本文と同じ 4 桁下げで、折り返して積む。
fn push_note(lines: &mut Vec<Line<'static>>, text: &str, color: Color, inner_w: usize) {
    for chunk in wrap(text, inner_w.saturating_sub(4)) {
        lines.push(Line::from(Span::styled(
            format!("    {chunk}"),
            Style::default().fg(color),
        )));
    }
}

/// 機能への影響の 1 項目 (変化・確かめる・残る穴)。ラベルの幅は
/// [IMPACT_LABEL_W] に揃え、折り返した続きも本文の左端に合わせる。
fn push_labeled(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    color: ratatui::style::Color,
    inner_w: usize,
) {
    let pad = IMPACT_LABEL_W.saturating_sub(unicode_width::UnicodeWidthStr::width(label));
    let indent = IMPACT_INDENT + IMPACT_LABEL_W + 1;
    for (n, chunk) in wrap(value, inner_w.saturating_sub(indent + 1))
        .into_iter()
        .enumerate()
    {
        let head = if n == 0 {
            format!("{}{label}{} ", " ".repeat(IMPACT_INDENT), " ".repeat(pad))
        } else {
            " ".repeat(indent)
        };
        lines.push(Line::from(Span::styled(
            format!("{head}{chunk}"),
            Style::default().fg(color),
        )));
    }
}

/// ハンクは飛び飛びなのでパーサの状態が続かず、数行ぶん色がずれる。それでも全部を
/// 無彩色にするよりは読める、という判断でブロック単位に割り切っている。
fn highlight_block(
    app: &App,
    block: &revidere::Block,
    tab_width: usize,
) -> Vec<Vec<(Style, String)>> {
    let syntax_set = &app.appearance.highlight.syntax_set;
    let syntax = crate::viewer::find_syntax(
        syntax_set,
        Some(block.path.as_str()),
        block.lines.first().map(|l| l.line.text.as_str()),
    );
    let mut h = HighlightLines::new(syntax, &app.appearance.highlight.theme);

    block
        .lines
        .iter()
        .map(|ordered| {
            // syntect は行末の改行を前提にする。
            let with_nl = format!("{}\n", ordered.line.text);
            let Ok(ranges) = h.highlight_line(&with_nl, syntax_set) else {
                return vec![(
                    Style::default().fg(app.appearance.theme.fg),
                    ordered.line.text.clone(),
                )];
            };
            let mut col = 0;
            ranges
                .into_iter()
                .map(|(style, text)| {
                    let style = syntect_tui::translate_style(style)
                        .unwrap_or_default()
                        .bg(Color::Reset);
                    let text = crate::viewer::render::expand_tabs_at(
                        text.trim_end_matches('\n'),
                        tab_width,
                        &mut col,
                    );
                    (style, text)
                })
                .collect()
        })
        .collect()
}

/// 追加・削除は背景色で示す。構文の色を前景に使う以上そこでは表せず、記号と背景で
/// 二重に言うと重要度の帯と合わせて 3 つの印が並んで読めなくなる。
fn diff_line(
    line: &revidere::DiffLine,
    owned: bool,
    tokens: Option<&Vec<(Style, String)>>,
    theme: &crate::theme::Theme,
    inner_w: usize,
) -> Line<'static> {
    let (band_color, bg) = match line.tag {
        Tag::Add => (theme.diff_add, Some(theme.diff_add_bg)),
        Tag::Del => (theme.diff_del, Some(theme.diff_del_bg)),
        Tag::Context => (theme.muted, None),
    };
    let no = line
        .new_line
        .or(line.old_line)
        .map(|n| format!("{n:>5}"))
        .unwrap_or_else(|| "     ".to_string());
    // 帯はこの行がこの項目の持ち物であることの印。借りてきた文脈行には付かない。
    let band = if owned { "▌" } else { " " };
    let band_style = if owned {
        Style::default().fg(band_color)
    } else {
        Style::default().fg(theme.muted)
    };

    let mut spans = vec![Span::styled(format!("{band}{no} "), band_style)];
    match tokens {
        Some(tokens) if !tokens.is_empty() => spans.extend(tokens.iter().map(|(style, text)| {
            // 構文の前景色は残したまま、背景だけ diff の色で塗る。
            let style = match bg {
                Some(bg) => style.bg(bg),
                None => *style,
            };
            Span::styled(text.clone(), style)
        })),
        _ => spans.push(Span::styled(
            line.text.clone(),
            Style::default().fg(theme.fg).bg(bg.unwrap_or(Color::Reset)),
        )),
    }

    // 追加・削除は行末まで背景を伸ばす (GitHub 風のブロック塗り)。途中で
    // 切れると、背景色が差分の印ではなく文字列リテラルの色に見える。
    if let Some(bg) = bg {
        let used: usize = spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        if used < inner_w {
            spans.push(Span::styled(
                " ".repeat(inner_w - used),
                Style::default().bg(bg),
            ));
        }
    }

    Line::from(spans)
}

/// 切るのは文字数ではなく表示幅。日本語は 1 文字 2 列なので、文字数で切ると幅の
/// 2 倍に伸びて、はみ出した分が枠で黙って落ちる。
fn wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;

    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for ch in para.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            // 1 文字も入らない行は作らない (幅 1 に全角が来ても進む)。
            if used + w > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(ch);
            used += w;
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全角は 1 文字 2 列として数えること。文字数で切っていた頃は、幅の 2 倍に
    /// 伸びた行が枠で黙って切り落とされ、本文の後半が読めなかった。
    #[test]
    fn wrap_splits_on_display_width_and_keeps_every_character() {
        let got = wrap("あいうえおかきくけこ", 4);
        assert_eq!(got, vec!["あい", "うえ", "おか", "きく", "けこ"]);
        assert_eq!(got.concat().chars().count(), 10);
    }

    /// 半角と全角が混ざっても、どの行も幅を超えないこと。
    #[test]
    fn wrap_never_exceeds_the_width_on_mixed_text() {
        use unicode_width::UnicodeWidthStr;
        for line in wrap("abcあいdef うえおgh", 7) {
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= 7,
                "{line:?} is wider than 7"
            );
        }
    }

    /// 幅 0 で無限ループや空返しにならないこと。狭い端末で項目一覧の幅が
    /// 潰れたときにここへ来る。
    #[test]
    fn wrap_with_zero_width_returns_the_text_untouched() {
        assert_eq!(wrap("abc", 0), vec!["abc"]);
    }

    #[test]
    fn wrap_keeps_blank_lines_between_paragraphs() {
        assert_eq!(wrap("a\n\nb", 8), vec!["a", "", "b"]);
    }
}
