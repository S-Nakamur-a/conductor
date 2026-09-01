//! 公開の render エントリポイント。Claude PTY パネルの内側の矩形にトランスクリプトを描画し、
//! パネル幅が変わったときだけ build で行キャッシュを再構築する。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::App;

use super::build::{BuildCtx, LineMeta, build_lines};
use super::helpers::truncate_to_width;
use super::palette;

/// リフロー版トランスクリプトビューを area（Claude パネルの内側の矩形）に描画する。
///
/// app を &mut で受け取るのは、行リストの構築・スクロール後に更新されたスクロール/キャッシュ
/// 状態を app.reflow に書き戻すため。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // バッジを描画する前に return する経路は必ずヒット領域を取り消す。さもないと、
    // すでに存在しないチップにクリックが当たり続けてしまう。
    app.reflow.jump_hit = None;
    if area.width == 0 || area.height == 0 {
        return;
    }

    // 前フレームのテキストが透けて見えないよう、部分的にしか埋まっていない
    // トランスクリプトでも必ずクリアする（ライブ PTY の描画と同じ、scrollback のにじみ対策）。
    frame.render_widget(ratatui::widgets::Clear, area);

    // オーバーレイは意図的に未書き込みのままのセルを塗りつぶすので、閉じた後は強制再描画で
    // しかクリアできない (ratatui の diff は自身のバッファ同士を比べるので双方汚れて一致する)。
    let overlay_active = app.is_any_overlay_active();
    if app.reflow.last_overlay_active && !overlay_active {
        app.terminal.needs_clear = true;
    }
    app.reflow.last_overlay_active = overlay_active;

    // パネル幅をフルに使う。安全ガターとして 1 カラム狭く構築すると、毎行で Claude Code 本来の
    // 折り返し位置と恒久的にずれる。にじみの原因だったガターのグリフは絶対位置指定になり
    // (super::build::width_risk_hole)、紛れ込んだ幅広文字の被害は各行が再アンカーされるので
    // 伝播しない。行折り返しも無効 (main.rs の DisableLineWrap) で、ここは最も右のカラム。
    let render_area = area;
    let inner_width = render_area.width as usize;
    let inner_height = render_area.height as usize;

    // セッションログはバックグラウンドスレッドでパースされる。エントリが届くまでは、空の
    // トランスクリプトではなく中央にプレースホルダを出す。スイープ演出は続くので、境界線の
    // 遷移がそのままローディングインジケータを兼ねる。
    if app.reflow.loading {
        let msg = "Loading transcript\u{2026}";
        let y = area.y + area.height / 2;
        let msg_cols = UnicodeWidthStr::width(msg).min(inner_width) as u16;
        let x = render_area.x + (render_area.width.saturating_sub(msg_cols)) / 2;
        let line = Line::from(Span::styled(
            truncate_to_width(msg, inner_width),
            Style::default().fg(palette::INACTIVE),
        ));
        frame.render_widget(Paragraph::new(line), Rect::new(x, y, msg_cols, 1));
        return;
    }

    // パネル幅や展開状態が変わったときにキャッシュ済みの行を再構築する。
    // anchored は「先頭行はどこへ行ったか」という再構築の答えを、下にある唯一の
    // スクロール決定処理まで運ぶ。何も再構築しなかったフレームでは None のままになる。
    let mut anchored: Option<usize> = None;
    if app.reflow.last_width != render_area.width || app.reflow.needs_rebuild {
        let built = {
            let ctx = BuildCtx {
                entries: &app.reflow.entries,
                cache: &app.reflow.cache,
                theme: &app.appearance.theme,
                syntax_set: &app.appearance.highlight.syntax_set,
                syntect_theme: &app.appearance.highlight.theme,
                expanded: app.reflow.expanded,
            };
            build_lines(&ctx, inner_width)
        };
        // 再構築前にビューポート先頭に何があったかを記憶する。scroll は生の行インデックスで、幅や
        // 展開状態が変わるとその意味が別物になり、ビューが飛ぶ。
        let anchor = app.reflow.line_meta.get(app.reflow.scroll).copied();
        let total = built.lines.len();

        app.reflow.cached_lines = built.lines;
        app.reflow.line_meta = built.meta;
        app.reflow.total_lines = total;
        app.reflow.last_width = render_area.width;
        app.reflow.needs_rebuild = false;
        // 行の位置がすべて動いたので、物理的に再描画する。そうしないと、未書き込みの
        // セルに前のレイアウトのグリフが残ってしまう。
        app.terminal.needs_clear = true;

        anchored = anchor.map(|a| anchor_index(&app.reflow.line_meta, a));
    }

    app.reflow.last_inner_height = area.height;

    // ジオメトリが動きうるすべてのケース (幅・高さの変更、展開トグル) を 1 つの決定にまとめる。
    // 追従中なら最新行に再固定し、履歴を読んでいる人は anchor が解決した行に戻る。上限は
    // total - inner_height なので、論理的な末尾では最後のコンテンツ行が最後の表示行に収まる。
    app.reflow.scroll = crate::reflow::input::scroll_after_reflow(
        app.reflow.follow,
        anchored,
        app.reflow.scroll,
        app.reflow.total_lines,
        inner_height,
    );

    let scroll = app.reflow.scroll;

    // 遷移演出の完了処理
    // 表示切り替え時のタイマーを進める。エントリのスイープ演出が終わったらクリアし、
    // 境界線を安定した読み取りモードの色に落ち着かせる。境界線自体の色遷移の描画は
    // ここではなく terminal::render::claude::render で行う。
    let entry_done = app.reflow.sweep.as_ref().is_some_and(|s| {
        crate::reflow::input::sweep_progress(&s.start, crate::reflow::input::TRANSITION_DURATION_MS)
            >= 1.0
    });
    if entry_done {
        app.reflow.sweep = None;
    }

    // 表示中のウィンドウをキャッシュから参照で直接転送する。毎フレーム行ベクタを
    // クローンしない。折り返しは行わない。markdown_cache.render がすでに body_width
    // カラム以下の行を生成しており、各行にはさらに MARKER_COLS 幅のプレフィックスが
    // 付くので、合計幅は render_area.width 以下に収まる。論理行1つ = 表示行1つなので、
    // 見えない過大行なしにスクロール計算が正確になる。
    let buf = frame.buffer_mut();
    let rows = app
        .reflow
        .cached_lines
        .iter()
        .zip(app.reflow.line_meta.iter())
        .skip(scroll)
        .take(inner_height);
    for (i, (line, meta)) in rows.enumerate() {
        let y = render_area.y + i as u16;
        buf.set_line(render_area.x, y, line, render_area.width);
        // 幅の曖昧なガターグリフの直後のセルを1つ、未書き込みのまま残す。diff は
        // それをスキップするので次のセルはグリフのセルと連続しなくなり、crossterm
        // バックエンドは本文の前に絶対位置指定の MoveTo を発行する。これにより、
        // 端末が実際にグリフをどれだけ幅広く描画しようと正しいカラムに固定される。
        // skip は毎フレーム Buffer::reset でクリアされるので、フレームごとに
        // 再適用する必要がある。
        if let Some(col) = meta.skip_col
            && col < render_area.width
            && let Some(cell) = buf.cell_mut((render_area.x + col, y))
        {
            cell.set_skip(true);
        }
    }

    app.reflow.jump_hit = render_jump_badge(frame, render_area, app.reflow.follow);
}

/// 追従解除バッジの各幅段階でのテキスト。長い順に並んでいる。意図的に ASCII のみにしている。
/// バッジはパネルの右端に配置されるため、端末が unicode-width の計測より幅広く描画する
/// グリフ（ガターが未書き込みセルで解決している ⏺/⎿ の問題）があると末尾が境界線に
/// はみ出してしまう。そのためここには矢印を使っていない。
pub(super) const JUMP_BADGE_LABELS: [&str; 3] = [" Jump to latest (G) ", " Latest (G) ", " (G) "];

/// 「最新ターンにいない」ことを示すバッジを描画し、クリックを戻せるよう画面上の矩形を返す。
///
/// 追従が外れているときだけ表示される。それ自体がフィードバックになっている。追従中は
/// バッジが存在せず、この関数はヒット領域を返さないので、読み手が末尾に戻った後に
/// 古い矩形がクリックを飲み込み続けることはない。
///
/// 意図的に、まだ的として機能する範囲で画面上もっとも控えめなものにしてある。最終行に
/// 右寄せで置いた1個のチップで、トランスクリプト自身の淡いグレーを user ターンブロックの
/// 背景の上に重ねている。これより目立つものにすると、よくあるケースであるスクロールアップの
/// たびにトランスクリプトと注意を奪い合ってしまう。
pub(super) fn render_jump_badge(frame: &mut Frame, area: Rect, following: bool) -> Option<Rect> {
    if following || area.height == 0 {
        return None;
    }
    // 収まる中で最も長いラベルを選び、パネル端に対して1カラムの余裕を残す。最短のラベルでも
    // パネルに収まらない場合は、読めない形に切り詰めるのではなくバッジ自体を出さない。
    let label = JUMP_BADGE_LABELS
        .iter()
        .find(|l| UnicodeWidthStr::width(**l) < area.width as usize)?;
    let w = UnicodeWidthStr::width(*label) as u16;

    let rect = Rect::new(area.x + area.width - w, area.y + area.height - 1, w, 1);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            *label,
            Style::default().fg(palette::INACTIVE).bg(palette::USER_BG),
        ))),
        rect,
    );
    Some(rect)
}

/// 再構築後に anchor と一致する行のインデックス。(entry, block, offset) が anchor の
/// それ以上である最初の行を返すので、短くなった（あるいは消えた）ブロックでも、
/// 無関係な位置へスクロールするのではなく、今その位置を占めている何かに着地する。
pub(super) fn anchor_index(meta: &[LineMeta], anchor: LineMeta) -> usize {
    let key = (anchor.entry, anchor.block, anchor.offset);
    meta.iter()
        .position(|m| (m.entry, m.block, m.offset) >= key)
        .unwrap_or_else(|| meta.len().saturating_sub(1))
}
