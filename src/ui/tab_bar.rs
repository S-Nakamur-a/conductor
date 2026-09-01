//! Claude / Shell ターミナルパネルで共有する、スクロール可能なタブバー。
//!
//! 素の ratatui::widgets::Tabs はタブを左から右へ描画して右端をはみ出した分を黙って
//! 切り捨てるので、セッション数が増えると最重要な [+] が消える。ここでは [+] と展開
//! トグルを右端に固定し、セッションタブだけを残りのスペースで横スクロールさせる。
//!
//! 描画時にクリック可能な領域 (絶対スクリーン列) を記録するので、マウス処理は幅を
//! 再計算するのではなく、まったく同じジオメトリを参照する。

use crate::hit_map::ColumnSpans;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use crate::ui::common::strip::visible_window;
use crate::ui::common::text::truncate_to_width;

fn w(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// タブバーのクリック可能な領域が何をするか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAction {
    /// このグローバル PTY インデックスのセッションへ切り替える。
    Select(usize),
    /// このグローバル PTY インデックスのセッションを閉じる。
    Close(usize),
    /// 新しいセッションを起動する。
    Add,
    /// パネルの展開状態を切り替える。
    Expand,
    /// タブストリップを左へスクロールする（左端に隠れたタブを表示する）。
    ScrollLeft,
    /// タブストリップを右へスクロールする（右端に隠れたタブを表示する）。
    ScrollRight,
}

/// 描画する1つのセッションタブ。
pub struct TabItem {
    /// グローバル PTY セッションインデックス（Select/Close が運ぶ値）。
    pub global_idx: usize,
    /// 整形済みのラベル。例: "[CC:🎹]"。
    pub label: String,
    /// これがアクティブなセッションかどうか。
    pub is_active: bool,
    /// *非アクティブな* タブのラベルの基本スタイル（待機中のパルスなど）。
    /// アクティブなタブでは無視され、代わりに強い選択色の塗りつぶしが使われる。
    pub label_style: Style,
}

/// タブバーを描画し、クリック可能な領域と解決後のスクロール位置を返す。
///
/// reveal はアクティブなタブを見せるのに必要な最小限だけウィンドウをパンする。hover が
/// 描画に反映されるのは Close だけで、「押下」スタイルは実装しない — mouse-down/up は
/// せいぜい 1〜2 フレームしか続かず、実装コストに見合わない。
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    items: &[TabItem],
    scroll: usize,
    reveal: bool,
    is_expanded: bool,
    hover: Option<TabAction>,
) -> (ColumnSpans<TabAction>, usize) {
    let mut hits = ColumnSpans::default();
    if area.width == 0 || area.height == 0 {
        return (hits, scroll);
    }

    let max_x = area.x + area.width;

    let add = "[+]";
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };
    // " [+] [<=>]" — 先頭のスペースでクラスタとタブを分離する。
    let right_w = 1 + w(add) + 1 + w(expand_label);
    let tabs_region_w = area.width.saturating_sub(right_w);

    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;

    // アクティブなタブのインデックス（reveal 用）と、タブごとのスロット幅。
    let selected = items.iter().position(|t| t.is_active).unwrap_or(0);
    let close = " [x]";
    let close_w = w(close);
    let sep = " ";
    let sep_w = w(sep);
    let total = items.len();
    let hint_reserve_per_side = 4u16; // "‹NN " / " NN›"

    // 1つでも非常に長いセッション名がスクロール領域をはみ出して固定された
    // [+]/expand クラスタを画面外へ押し出さないよう、各ラベルに上限を設ける
    // — これが「長い名前が new/close ボタンを隠す」バグの原因だった。妥当な
    // 上限を設けることで複数のタブを同時に表示できるようにもなる。切り詰められた
    // ラベルは末尾に "…" が付く。
    let max_label_w = tabs_region_w
        .saturating_sub(close_w + sep_w + hint_reserve_per_side * 2)
        .clamp(4, 28);
    let labels: Vec<String> = items
        .iter()
        .map(|t| truncate_to_width(&t.label, max_label_w as usize))
        .collect();
    let slots: Vec<u16> = labels.iter().map(|l| w(l) + close_w).collect();

    // ヒントなしですべて収まるか? 収まるならヒント用の予約分を省く。
    let all_fit = visible_window(&slots, sep_w, tabs_region_w, 0, 0, false).1 == total;
    let avail = if all_fit {
        tabs_region_w
    } else {
        tabs_region_w.saturating_sub(hint_reserve_per_side * 2)
    };
    let (start, end) = if all_fit {
        (0, total)
    } else {
        visible_window(&slots, sep_w, avail, scroll, selected, reveal)
    };

    // 左側のオーバーフローヒント（クリックすると左へスクロール）。
    if start > 0 {
        let hint = format!("\u{2039}{} ", start);
        let hw = w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.muted)));
        hits.push(x, x + hw, TabAction::ScrollLeft);
        x += hw;
    }

    for (offset, item) in items[start..end].iter().enumerate() {
        let label = &labels[start + offset];
        if offset > 0 {
            spans.push(Span::raw(sep));
            x += sep_w;
        }
        let label_w = w(label);
        // どちらのタブの [x] も theme.error にする: 1クリックで非アクティブな
        // タブも閉じられるようになったため、実行中のセッションを黙って
        // 終了させるボタンは、目立たないグレーではなく危険な色として
        // 見える必要がある。
        let close_style = Style::default().fg(theme.error);
        let close_style = if hover == Some(TabAction::Close(item.global_idx)) {
            close_style.bg(theme.gutter_hover_bg)
        } else {
            close_style
        };
        if item.is_active {
            // アクティブなセッションが一目でわかるよう、強く塗りつぶしたタブに
            // する。[x] は塗りつぶしの外側（デフォルト背景の上）に置くことで
            // 危険を示す赤が読みやすいままになる — 塗りつぶしの中に置くと
            // アクセント背景の上に赤が乗ってコントラストが悪くなる。worktree
            // バーのチップ + [x] と同じ考え方。
            let fill = Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD);
            spans.push(Span::styled(label.clone(), fill));
        } else {
            spans.push(Span::styled(label.clone(), item.label_style));
        }
        // " [x]" の先頭のスペースは Select のヒット領域に属する（ボタンの前の
        // セパレータであるため）ので、プレーンに描画する。hover 背景が付くのは
        // "[x]" の文字そのもの（Close のヒット領域）だけ。
        spans.push(Span::raw(" "));
        spans.push(Span::styled("[x]", close_style));
        // Select はラベル（+ close サフィックスの先頭スペース）をカバーし、
        // Close は "[x]" の文字だけをカバーする。
        hits.push(x, x + label_w + 1, TabAction::Select(item.global_idx));
        hits.push(
            x + label_w + 1,
            x + label_w + close_w,
            TabAction::Close(item.global_idx),
        );
        x += label_w + close_w;
    }

    // 右側のオーバーフローヒント（クリックすると右へスクロール）。固定クラスタの手前。
    if end < total {
        let hint = format!(" {}\u{203a}", total - end);
        let hw = w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.muted)));
        hits.push(x, x + hw, TabAction::ScrollRight);
        x += hw;
    }

    // 固定クラスタが右端にぴったり収まるようパディングする。
    let cluster_x = max_x.saturating_sub(right_w);
    if x < cluster_x {
        let pad = (cluster_x - x) as usize;
        spans.push(Span::raw(" ".repeat(pad)));
        x = cluster_x;
    }

    // 固定された [+]（新規セッション）。
    spans.push(Span::raw(sep));
    x += sep_w;
    spans.push(Span::styled(
        add,
        Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD),
    ));
    hits.push(x, x + w(add), TabAction::Add);
    x += w(add);

    // 固定された展開トグル。
    spans.push(Span::raw(sep));
    x += sep_w;
    spans.push(Span::styled(
        expand_label,
        Style::default().fg(expand_color),
    ));
    hits.push(x, x + w(expand_label), TabAction::Expand);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    (hits, start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn items(n: usize) -> Vec<TabItem> {
        (0..n)
            .map(|i| TabItem {
                global_idx: i,
                label: format!("[CC:{i}]"),
                is_active: i == 0,
                label_style: Style::default(),
            })
            .collect()
    }

    fn render_hits(width: u16, items: &[TabItem], scroll: usize) -> ColumnSpans<TabAction> {
        render_hits_hover(width, items, scroll, None)
    }

    fn render_hits_hover(
        width: u16,
        items: &[TabItem],
        scroll: usize,
        hover: Option<TabAction>,
    ) -> ColumnSpans<TabAction> {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        let mut captured = ColumnSpans::default();
        terminal
            .draw(|f| {
                let area = f.area();
                let (hits, _) = render(f, area, &theme, items, scroll, false, false, hover);
                captured = hits;
            })
            .unwrap();
        captured
    }

    /// terminal を返してセルスタイルを直接見られるようにする。ヒット領域の出力では
    /// 答えられない hover の背景色を確かめるため。
    fn render_buffer(
        width: u16,
        items: &[TabItem],
        hover: Option<TabAction>,
    ) -> ratatui::buffer::Buffer {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                render(f, area, &theme, items, 0, false, false, hover);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn タブが溢れても追加と展開は押せる() {
        // バーに収まる数よりはるかに多いタブでも、[+] は常に存在してクリックできなければならない。
        let hits = render_hits(30, &items(20), 0);
        assert!(
            hits.spans().any(|(_, _, a)| *a == TabAction::Add),
            "the [+] new-session button must always be hittable"
        );
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Expand));
    }

    #[test]
    fn 溢れるとスクロールの手がかりが出る() {
        let hits = render_hits(24, &items(20), 5);
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::ScrollLeft));
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::ScrollRight));
    }

    #[test]
    fn 固定クラスタは右端にぴったり付く() {
        let width = 40u16;
        let hits = render_hits(width, &items(3), 0);
        let rightmost = hits.spans().map(|(_, x1, _)| x1).max().unwrap();
        assert_eq!(
            rightmost, width,
            "expand toggle should end at the right edge"
        );
    }

    #[test]
    fn 極端に長いラベル1つでも追加と展開は固定される() {
        // 巨大な名前を持つ単一のセッションでも [+]/[x] を隠さない。
        let items = vec![TabItem {
            global_idx: 0,
            label: "[CC:a-really-extremely-long-session-name-that-overflows]".to_string(),
            is_active: true,
            label_style: Style::default(),
        }];
        let width = 30u16;
        let hits = render_hits(width, &items, 0);
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Add));
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Expand));
        // バーの右端を超えて何かがはみ出してはならない。
        assert!(hits.spans().all(|(_, x1, _)| x1 <= width));
        // 単一のタブは選択可能なままである。
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Select(0)));
    }

    #[test]
    fn 収まっていれば全タブが押せる() {
        let hits = render_hits(80, &items(3), 0);
        for i in 0..3 {
            assert!(
                hits.spans().any(|(_, _, a)| *a == TabAction::Select(i)),
                "tab {i} should be selectable when everything fits"
            );
        }
    }

    /// アクティブと非アクティブで close ボタンのスタイルを区別でき、hover が漏れない
    /// ことを示す 2 つ目の Close ヒットも取れる。
    fn two_tabs() -> Vec<TabItem> {
        vec![
            TabItem {
                global_idx: 0,
                label: "[CC:0]".to_string(),
                is_active: true,
                label_style: Style::default(),
            },
            TabItem {
                global_idx: 1,
                label: "[CC:1]".to_string(),
                is_active: false,
                label_style: Style::default(),
            },
        ]
    }

    #[test]
    fn 非アクティブなタブの閉じるはmutedではなくerror色() {
        // 1クリックで非アクティブなタブも閉じられるようになったので、その
        // [x] は以前の目立たないグレーではなく危険を示す色（theme.error）で
        // 表示されなければならない。目立たないグレーだと、破壊的な1クリック
        // ボタンがほぼ見えなくなってしまっていた（最悪のケースは
        // solarized-dark で、muted ≈ 背景色だった）。
        let theme = Theme::default();
        let items = two_tabs();
        let hits = render_hits(80, &items, 0);
        let close_hit = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Close(1))
            .unwrap();
        let buf = render_buffer(80, &items, None);
        assert_eq!(buf[(close_hit.0, 0)].fg, theme.error);
    }

    #[test]
    fn アクティブなタブの閉じるもerror色() {
        // 以前の実装から変わっていないが、今後の編集でアクティブなタブの
        // close ボタンだけが黙って劣化しないよう、ここで固定して検証する。
        let theme = Theme::default();
        let items = two_tabs();
        let hits = render_hits(80, &items, 0);
        let close_hit = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Close(0))
            .unwrap();
        let buf = render_buffer(80, &items, None);
        assert_eq!(buf[(close_hit.0, 0)].fg, theme.error);
    }

    #[test]
    fn hoverの背景はhover中の閉じるにだけ付く() {
        let theme = Theme::default();
        let items = two_tabs();
        let hits = render_hits(80, &items, 0);
        let hovered = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Close(1))
            .unwrap();
        let other = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Close(0))
            .unwrap();

        let hovered_buf = render_buffer(80, &items, Some(TabAction::Close(1)));
        assert_eq!(hovered_buf[(hovered.0, 0)].bg, theme.gutter_hover_bg);
        // 他方のタブの close ボタンは、タブ1の hover の影響を受けない。
        assert_ne!(hovered_buf[(other.0, 0)].bg, theme.gutter_hover_bg);

        // hover がまったくない場合、どちらの close ボタンにも背景は付かない。
        let no_hover_buf = render_buffer(80, &items, None);
        assert_ne!(no_hover_buf[(hovered.0, 0)].bg, theme.gutter_hover_bg);
    }

    #[test]
    fn hit_atはその桁を持つアクションを見つける() {
        let items = two_tabs();
        let hits = render_hits(80, &items, 0);
        let close_hit = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Close(1))
            .unwrap();
        assert_eq!(hits.at(close_hit.0), Some(TabAction::Close(1)));
    }

    #[test]
    fn どの領域の外でもhit_atはnoneになる() {
        let items = two_tabs();
        let hits = render_hits(80, &items, 0);
        let past_everything = hits.spans().map(|(_, x1, _)| x1).max().unwrap() + 1;
        assert_eq!(hits.at(past_everything), None);
    }

    #[test]
    fn 閉じるの前の区切りスペースには色が付かない() {
        // ラベルと "[x]" の間のスペースは Close ではなく Select のヒット
        // 領域に属する — 隣の close ボタンが hover されている間も hover 背景を
        // 拾ってはならない。拾ってしまうとハイライトがラベルのクリック可能な
        // 領域に見た目上にじみ出てしまう。
        let theme = Theme::default();
        let items = two_tabs();
        let hits = render_hits(80, &items, 0);
        let close_hit = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Close(1))
            .unwrap();
        let buf = render_buffer(80, &items, Some(TabAction::Close(1)));
        assert_ne!(buf[(close_hit.0 - 1, 0)].bg, theme.gutter_hover_bg);
    }
}
