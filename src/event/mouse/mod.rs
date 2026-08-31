//! マウスイベント処理 — クリック、スクロール、ドラッグ操作。
//!
//! このモジュールはエントリポイント（handle_mouse_event）と、各パネル別サブ
//! モジュールで共有するヒットテスト用ジオメトリ（ClickGeometry/Column）、
//! ダブルクリック判定のヘルパーを持つ。各サブモジュールはレイアウトの1領域を
//! 担当する: [bars]（通知バー/worktreeバー/タイトルバー）、そして
//! [scroll]（全パネル共通のホイールスクロール）。
//! Viewer カラムのクリック処理は [crate::viewer::mouse]、Explorer カラムは
//! [crate::explorer::mouse]、Worktree カラムは [crate::worktree::mouse]、
//! Terminal カラムは [crate::terminal::mouse] にある。

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Focus};
use crate::overlay::ActiveOverlay;
use crate::viewer::comment_actions::open_viewer_comment;

mod bars;
mod scroll;

#[cfg(test)]
mod tests;

use crate::terminal::mouse::handle_terminal_column_click;
use crate::viewer::mouse::handle_viewer_column_click;
use crate::worktree::mouse::handle_worktree_column_click;
use bars::{handle_title_bar_click, handle_wtbar_click, wtbar_page_step};
use scroll::handle_mouse_scroll;

/// タブ行を描いていないフレームでは領域が空なので false になり、ホイールは本文の
/// スクロールへ落ちる。
fn on_viewer_tab_row(app: &App, col: u16, row: u16, geom: &ClickGeometry) -> bool {
    !app.viewer.tab_row_hits.is_empty()
        && row == geom.main_area.y + 1
        && matches!(geom.column_at(col), Column::Viewer)
}

/// Claude / Shell のタブ帯（各パネルの 1 行目）の上なら、Claude 側かどうかを
/// 返す。
fn terminal_tab_row_at(col: u16, row: u16, geom: &ClickGeometry) -> Option<bool> {
    if !matches!(geom.column_at(col), Column::Terminal) {
        return None;
    }
    if row == geom.terminal_claude_y {
        Some(true)
    } else if row == geom.terminal_split_y {
        Some(false)
    } else {
        None
    }
}

/// 画面上の行オフセット（inner_yからの相対値）を、インラインスレッド行を
/// 考慮した1始まりのファイル行番号に解決する。画面行マッピングが無い場合は
/// 単純な算術にフォールバックする。
pub(crate) fn resolve_screen_line(app: &App, screen_offset: usize) -> Option<usize> {
    let map = &app.viewer.content.screen_row_map;
    if !map.is_empty() {
        match map.get(screen_offset) {
            Some(crate::viewer::ScreenRow::Code(line)) => Some(*line),
            _ => None,
        }
    } else {
        let line_1 = app.viewer.content.file_scroll + screen_offset + 1;
        if line_1 <= app.viewer.content.file_content.len() {
            Some(line_1)
        } else {
            None
        }
    }
}

/// col が折りたたみマーカーの当たり判定に入るか。gutter_end はガターの右端
/// （排他）。
///
/// マーカーの1列だけでは三角が小さすぎて狙えないので、行番号より右のガター
/// （隙間・マーカー・区切り線・その右の空白）をまとめて受ける。行番号そのもの
/// はコメント作成に残す。ホバーの罫線とクリックが同じ範囲を見るように、判定は
/// ここにしかない。
pub(crate) fn in_fold_zone(col: u16, gutter_end: u16) -> bool {
    col < gutter_end && col + 5 >= gutter_end
}

/// ホバーのモーダル階層へマウスを配る。消費したら true を返し、呼び出し側は
/// そこで打ち切って背景のパネルに届かないようにする。
fn handle_hover_modal_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    if !app.code_nav.hover_info.is_shown() {
        return false;
    }
    let col = mouse.column;
    let row = mouse.row;
    let in_rect = |r: ratatui::layout::Rect| {
        r.width > 0
            && r.height > 0
            && col >= r.x
            && col < r.x + r.width
            && row >= r.y
            && row < r.y + r.height
    };
    let pinned = app.code_nav.hover_info.pinned;

    match mouse.kind {
        MouseEventKind::Moved => {
            if app.hover_point_hit(col, row) {
                app.hover_keep_alive();
                return true;
            }
            // ポップアップの外: 固定中はそれでも消費し、はぐれたマウス移動が
            // モーダルを壊さないようにする。一時的な状態なら通常のmoveハンドラに
            // 候補/猶予の管理を任せる。
            pinned
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // レベル2: プレビューをクリックするとそこへジャンプし、全部閉じる。
            if let Some(pr) = app
                .code_nav
                .hover_info
                .refs
                .as_ref()
                .and_then(|r| r.preview.as_ref())
                && in_rect(pr.rect)
            {
                app.hover_jump_to_preview();
                return true;
            }
            // レベル1: 参照行をクリックするとそのプレビューを開く。
            if let Some(refs) = app.code_nav.hover_info.refs.as_ref() {
                if let Some((idx, _)) = refs.row_hits.iter().find(|(_, r)| in_rect(*r)).copied() {
                    app.open_hover_preview(idx);
                    return true;
                }
                if in_rect(refs.rect) {
                    return true; // リストの余白 — 開いたままにする
                }
            }
            // 基本ポップアップ: 「N refs」をクリックするとリストを開く。本文をクリックすると維持する。
            if in_rect(app.code_nav.hover_info.refs_hit) {
                app.open_hover_refs();
                return true;
            }
            // 場所の行をクリックするとその定義へ飛ぶ。
            if in_rect(app.code_nav.hover_info.def_hit) {
                app.jump_to_hover_definition();
                return true;
            }
            if in_rect(app.code_nav.hover_info.info_rect) {
                return true;
            }
            // 全ての要素の外側: 固定中のモーダルは閉じてクリックを飲み込む。
            // 一時的なポップアップはクリックを通過させる（トップレベルの
            // 非Movedクリアがそれを消してくれる）。
            if pinned {
                app.clear_hover();
                app.request_redraw();
                return true;
            }
            false
        }
        MouseEventKind::ScrollDown => {
            if app
                .code_nav
                .hover_info
                .refs
                .as_ref()
                .is_some_and(|r| in_rect(r.rect))
            {
                app.hover_refs_move(1);
                return true;
            }
            pinned
        }
        MouseEventKind::ScrollUp => {
            if app
                .code_nav
                .hover_info
                .refs
                .as_ref()
                .is_some_and(|r| in_rect(r.rect))
            {
                app.hover_refs_move(-1);
                return true;
            }
            pinned
        }
        _ => pinned,
    }
}

fn has_blocking_overlay(app: &App) -> bool {
    use crate::app::WorktreeInputMode;
    use crate::review_state::ReviewInputMode;

    app.update.is_active()
        || app.review_state.comment_detail_active
        || app.review_state.input_mode != ReviewInputMode::Normal
        || app.worktree_mgr.input_mode != WorktreeInputMode::Normal
        || app.overlays.active != ActiveOverlay::None
        || app.viewer.filename_search.filename_search_active
        || app.review_state.search_active
        || app.review_state.template_picker_active
        || app.code_nav.references.active
        || app.code_nav.symbol_action.active
}

/// クリックとホバーの両方がここを通るので、光る場所と押せる場所は構造的にずれない。
/// 埋め込みエディタが出ている間はチップも無いものとして扱う。
fn revidere_badge_hit(app: &App, col: u16, row: u16, geom: &ClickGeometry) -> bool {
    if app.editor.is_some()
        || row != geom.explorer_mid_y
        || app.explorer.bottom() != crate::explorer::BottomView::Changes
    {
        return false;
    }
    crate::explorer::render::revidere_badge_cols(
        app.diff_state.files.len(),
        app.diff_state.error.is_some(),
        app.config.ui.icon_set(),
        geom.left_end,
        geom.explorer_w,
    )
    .is_some_and(|cols| cols.contains(&col))
}

/// 最大化中は列が両端に畳まれて境界の意味が失われるので常に不可。エディタが
/// Explorer+Viewer を1つの PTY に合体させている間も Explorer 側は不可。
fn divider_draggable(app: &App, divider: crate::app::Divider) -> bool {
    use crate::app::Divider;
    if app.expanded_panel.is_some() {
        return false;
    }
    if app.editor.is_some() && matches!(divider, Divider::ExplorerViewer | Divider::ExplorerSplit) {
        return false;
    }
    true
}

/// 画面上の列が、メインの4カラムのうちどれに属するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Worktree,
    Explorer,
    Viewer,
    Terminal,
}

/// マウスのヒットテストに使うフレームごとのレイアウトジオメトリ。
/// [handle_mouse_event] の冒頭でレイアウトキャッシュからスナップショットする。
/// これらの値をまとめておくことで、各カラムのクリックハンドラが長い引数リストを
/// 取らずに済む。
///
/// crate::explorer::mouse からも参照するため crate 全体に公開している。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClickGeometry {
    pub(crate) main_area: ratatui::layout::Rect,
    pub(crate) left_w: u16,
    pub(crate) explorer_w: u16,
    pub(crate) viewer_w: u16,
    pub(crate) left_end: u16,
    pub(crate) explorer_end: u16,
    pub(crate) viewer_end: u16,
    pub(crate) explorer_mid_y: u16,
    pub(crate) terminal_claude_y: u16,
    pub(crate) terminal_split_y: u16,
}

impl ClickGeometry {
    /// 画面上の列 col がどのカラムに属するかを決定する。
    fn column_at(&self, col: u16) -> Column {
        if col < self.left_end {
            Column::Worktree
        } else if col < self.explorer_end {
            Column::Explorer
        } else if col < self.viewer_end {
            Column::Viewer
        } else {
            Column::Terminal
        }
    }

    /// 隣接する列がそれぞれ自分の枠を描くので、縦の境界は2セルの厚みを持つ。両方を
    /// つかむ対象とし、角で重なる場合は縦を優先する。合体・最大化の判定は呼び出し側。
    fn divider_at(&self, col: u16, row: u16) -> Option<crate::app::Divider> {
        use crate::app::Divider;

        let top = self.main_area.y;
        let bottom = self.main_area.y.saturating_add(self.main_area.height);
        let right = self.main_area.x.saturating_add(self.main_area.width);
        let on_boundary = |v: u16, edge: u16| edge > 0 && (v == edge - 1 || v == edge);

        // 縦の境界: 画面高さ全体にわたるカラム境界。
        if row >= top && row < bottom {
            if on_boundary(col, self.explorer_end) {
                return Some(Divider::ExplorerViewer);
            }
            if on_boundary(col, self.viewer_end) {
                return Some(Divider::ViewerTerminal);
            }
        }
        // 横の境界: 1つのカラム内部の分割。
        if col >= self.left_end && col < self.explorer_end && on_boundary(row, self.explorer_mid_y)
        {
            return Some(Divider::ExplorerSplit);
        }
        if col >= self.viewer_end && col < right && on_boundary(row, self.terminal_split_y) {
            return Some(Divider::TerminalSplit);
        }
        None
    }

    /// クリックが上枠の行にあることは呼び出し側が保証する。
    fn expand_button_at(&self, col: u16) -> Option<Focus> {
        if col < self.left_end && self.left_w >= 7 {
            let btn_start = self.main_area.x + self.left_w - 6;
            let btn_end = self.main_area.x + self.left_w - 1;
            (col >= btn_start && col < btn_end).then_some(Focus::Worktree)
        } else if col >= self.left_end && col < self.explorer_end && self.explorer_w >= 7 {
            let btn_start = self.left_end + self.explorer_w - 6;
            let btn_end = self.left_end + self.explorer_w - 1;
            (col >= btn_start && col < btn_end).then_some(Focus::Explorer)
        } else if col >= self.explorer_end && col < self.viewer_end && self.viewer_w >= 7 {
            let btn_start = self.explorer_end + self.viewer_w - 6;
            let btn_end = self.explorer_end + self.viewer_w - 1;
            (col >= btn_start && col < btn_end).then_some(Focus::Viewer)
        } else {
            None
        }
    }
}

/// 反転ではなく、押した側のモードをそのまま選ぶ。見えているラベルを押しても
/// 何も起きないだけで済む。列も出す条件もレンダラと同じ関数から引く。
fn handle_md_toggle_click(app: &mut App, col: u16, geom: &ClickGeometry) -> bool {
    if !app.viewer.markdown_toggle_available() {
        return false;
    }
    let viewer_x = geom.explorer_end;
    let Some(seg) = crate::viewer::render::toggle_segments(viewer_x, geom.viewer_w) else {
        return false;
    };
    let want_rendered = if seg.raw.contains(&col) {
        false
    } else if seg.rendered.contains(&col) {
        true
    } else {
        return false;
    };
    if app.viewer.md_rendered != want_rendered {
        app.cmd_toggle_markdown_render();
    }
    true
}

/// フォーカスは動かさない。このビューを出るのは Esc だけという約束を、節を選ぶ
/// クリックで破らないため。
fn handle_revidere_mouse(app: &mut App, mouse: MouseEvent) {
    // 総括は 1 列で読むだけの画面。ホイールだけ効かせる。
    if app.revidere.show_overview {
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                crate::revidere::input::scroll_overview(app, SCROLL_LINES)
            }
            MouseEventKind::ScrollUp => crate::revidere::input::scroll_overview(app, -SCROLL_LINES),
            _ => {}
        }
        return;
    }

    let list = app.revidere.list_area;
    let in_list = list.width > 0 && mouse.column >= list.x && mouse.column < list.x + list.width;

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) if in_list => {
            // 枠線の内側が 1 行目。
            let Some(offset) = mouse.row.checked_sub(list.y + 1) else {
                return;
            };
            if let Some(idx) = app.revidere.list_rows.get(offset as usize).copied() {
                crate::revidere::input::select_section(app, idx);
            }
        }
        // 左列のホイールは節を送る。行ではなく節で動かすのは、左列の
        // スクロール位置が選択に従属していて単独では動かせないため。
        MouseEventKind::ScrollDown if in_list => crate::revidere::input::step_section(app, 1),
        MouseEventKind::ScrollUp if in_list => crate::revidere::input::step_section(app, -1),
        MouseEventKind::ScrollDown => crate::revidere::input::scroll_diff(app, SCROLL_LINES),
        MouseEventKind::ScrollUp => crate::revidere::input::scroll_diff(app, -SCROLL_LINES),
        _ => {}
    }
}

/// 右列のホイール 1 段で動かす行数。
const SCROLL_LINES: isize = 3;

pub fn handle_mouse_event(app: &mut App, mouse: MouseEvent, _frame_area: ratatui::layout::Rect) {
    // インタラクティブなホバーモーダルスタック（ポップアップ → 参照リスト → プレビュー）が
    // マウスを最初に受け取る: その各部分へのクリックはさらに下の階層へ潜り、上での移動は
    // 生存を維持し、固定中のモーダルは外側へのクリックを飲み込む（閉じるため）。
    if handle_hover_modal_mouse(app, mouse) {
        return;
    }

    // 何らかのオーバーレイ/モーダルがアクティブな場合、マウスイベントを全て消費して
    // 背景のパネル（スクロール、クリックなど）に届かないようにする。
    if has_blocking_overlay(app) {
        // ここでreturnすると、オーバーレイが開いている間は背景のパネルがMovedイベントを
        // 受け取らなくなる。Movedハンドラはマウスがそこから離れたときにツリー/差分リストの
        // 行ハイライトやジャンプ下線、ホバーポップアップを自然にクリアする役目を持つが、
        // それが働かなくなるということ。なので先にここでクリアしておき、モーダルの裏に
        // 何も光ったまま残らないようにする。
        app.clear_all_hover();
        return;
    }

    // レイアウトをキャッシュから読み込む（描画時に計算済み）。
    let lc = &app.layout.cache;
    let wtbar_area = lc.wtbar_area;
    let main_area = lc.main_area;

    let left_w = lc.columns[0].width;
    let explorer_w = lc.columns[1].width;
    let viewer_w = lc.columns[2].width;
    let left_end = lc.columns[0].x + left_w;
    let explorer_end = lc.columns[1].x + explorer_w;
    let viewer_end = lc.columns[2].x + viewer_w;

    let explorer_mid_y = lc.explorer_mid_y;
    let terminal_claude_y = lc.terminal_split[0].y;
    let terminal_split_y = lc.terminal_split[1].y;

    let col = mouse.column;
    let row = mouse.row;

    // 単なる移動以外のマウス操作（スクロール、クリック、ドラッグ）は自動ホバー
    // ポップアップを無効化する: それは今は古くなった行に紐づいていたもの。
    // Movedは下で自分のcandidateを別途管理する。
    if !matches!(mouse.kind, MouseEventKind::Moved) && app.clear_hover() {
        app.request_redraw();
    }

    // revidere の 2 列ビューは main_area をアコーディオンとは別の割り方で
    // 使うので、以下のカラム判定に流すと必ず違うペインに当たり、そこの
    // ハンドラがフォーカスを移してビューが閉じてしまう。ここで止める。
    // main_area の外 (タイトル・メニュー・worktree ストリップ) はこのビューでも
    // 出ているので、そのまま下へ通す。
    if app.focus == Focus::Revidere
        && main_area.height > 0
        && row >= main_area.y
        && row < main_area.y + main_area.height
    {
        handle_revidere_mouse(app, mouse);
        return;
    }

    let geom = ClickGeometry {
        main_area,
        left_w,
        explorer_w,
        viewer_w,
        left_end,
        explorer_end,
        viewer_end,
        explorer_mid_y,
        terminal_claude_y,
        terminal_split_y,
    };

    // ターミナルのタブ帯の上のホイールは、本文ではなくタブを横へ送る
    // （Viewer のタブ行・worktree ストリップと同じ）。この 1 行の上では
    // スクロールバックを遡れなくなる。
    if let Some(is_claude) = terminal_tab_row_at(col, row, &geom) {
        let scroll = if is_claude {
            &mut app.terminal.claude.tab_scroll
        } else {
            &mut app.terminal.shell.tab_scroll
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                *scroll += 1;
                return;
            }
            MouseEventKind::ScrollUp => {
                *scroll = scroll.saturating_sub(1);
                return;
            }
            _ => {}
        }
    }

    match mouse.kind {
        MouseEventKind::ScrollDown if wtbar_area.height > 0 && row == wtbar_area.y => {
            // worktreeストリップ上でのホイールは、横方向に約1画面ぶん
            // （チップ1つぶん重ねて）ページングする。トラックパッドの連続入力も
            // ホイールの1段も、チップを飛ばさずに意味のある量だけ動くようにする。
            app.wtbar.scroll = app.wtbar.scroll.saturating_add(wtbar_page_step(app));
        }
        MouseEventKind::ScrollUp if wtbar_area.height > 0 && row == wtbar_area.y => {
            app.wtbar.scroll = app.wtbar.scroll.saturating_sub(wtbar_page_step(app));
        }
        MouseEventKind::ScrollDown if on_viewer_tab_row(app, col, row, &geom) => {
            app.viewer.tab_scroll += 1;
        }
        MouseEventKind::ScrollUp if on_viewer_tab_row(app, col, row, &geom) => {
            app.viewer.tab_scroll = app.viewer.tab_scroll.saturating_sub(1);
        }
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(app, col, row, &geom, 3);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(app, col, row, &geom, -3);
        }
        MouseEventKind::ScrollLeft
            // 横スクロール — viewerパネルのみに作用する。
            if col >= explorer_end && col < viewer_end => {
                app.viewer.content.h_scroll = app.viewer.content.h_scroll.saturating_sub(4);
            }
        MouseEventKind::ScrollRight
            if col >= explorer_end && col < viewer_end => {
                app.viewer.scroll_right(4);
            }
        MouseEventKind::Down(MouseButton::Left) => {
            // メニューバーを最初にチェックする。理由は、下でworktreeストリップが
            // タイトルバーより先にチェックされるのと同じ: handle_title_bar_click は
            // main_areaより上の行を全て「タイトル」として扱ってしまう。メニューバーは
            // 外側クリックでの閉じる処理も持っており、それはどのパネルより先に
            // クリックを見る必要がある。
            if crate::menu::mouse::handle_menu_click(app, col, row) {
                return;
            }
            // worktree/タイトルバーへのクリックを最初に消費する。
            // worktreeバーはタイトルバーより先にチェックする必要がある。後者は
            // main_areaより上の全ての行を「タイトル」として扱ってしまい、そうしないと
            // worktreeストリップの行を飲み込んでしまう。
            if handle_wtbar_click(app, col, row, wtbar_area) {
                return;
            }
            if handle_title_bar_click(app, col, row, main_area) {
                return;
            }

            // main_area内のクリックのみを処理する。
            if row < main_area.y || row >= main_area.y + main_area.height {
                return;
            }

            // revidere の状態チップは Explorer の横境界と同じ行にあるので、
            // 境界より先に見る。そうしないと 10 セルぶんが常に境界に食われて
            // 押せない。チップは右枠の内側にあり、縦の境界のセルとは重ならない。
            if revidere_badge_hit(app, col, row, &geom) {
                app.cmd_revidere_badge_click();
                return;
            }

            // パネル境界をつかんでマウスリサイズを開始する。下のエディタ再フォーカスや
            // カラムのルーティングより先にチェックすることで、境界は常にその上に
            // 乗っているパネルより優先される（[<=>] 展開ボタンは境界より数セル内側に
            // あるので、つかむ範囲と重ならない）。
            if let Some(divider) = geom.divider_at(col, row)
                && divider_draggable(app, divider)
            {
                app.layout.divider_drag = Some(divider);
                app.layout.divider_hover = Some(divider);
                return;
            }

            // 埋め込みエディタは合体したExplorer+Viewer領域を占有する。この中への
            // クリックは単にエディタを(再)フォーカスするだけ — その裏にあるExplorerと
            // Viewerのパネルは隠れているので、それらのクリックハンドラは動いては
            // ならない。ターミナル列へのクリックはそのまま通過する。
            if app.editor.is_some() && col >= left_end && col < viewer_end {
                app.set_focus(Focus::Editor);
                return;
            }

            // Viewerの [Raw|Rendered] トグルと [<=>] 展開ボタン、どちらも上枠の行に
            // ある。トグルを先にチェックする。展開ボタンより左にあり、両者は
            // 重ならない。
            if row == main_area.y && handle_md_toggle_click(app, col, &geom) {
                return;
            }
            if row == main_area.y
                && let Some(target) = geom.expand_button_at(col) {
                    app.expanded_panel = if app.expanded_panel == Some(target) {
                        None
                    } else {
                        Some(target)
                    };
                    return;
                }

            match geom.column_at(col) {
                Column::Worktree => handle_worktree_column_click(app, row, &geom),
                Column::Explorer => app.explorer_click(col, row),
                Column::Viewer => handle_viewer_column_click(app, mouse, col, row, &geom),
                Column::Terminal => handle_terminal_column_click(app, mouse, col, row, &geom),
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            // divider（境界）のドラッグを最優先する: つかんだ境界をカーソルに
            // 追従させる。クランプされたミューテータは範囲外のターゲットを
            // 拒否するので、パネルの最小サイズを超えてドラッグしても、境界は
            // そこにピン留めされるだけになる。
            if let Some(divider) = app.layout.divider_drag {
                app.drag_divider_to(divider, col, row);
                return;
            }
            // 進行中のガター範囲選択を、ドラッグ先の行まで延長する。
            if let Some(anchor) = app.viewer.click.gutter_drag_anchor {
                let inner_y = main_area.y + 1;
                if row >= inner_y && col >= explorer_end && col < viewer_end {
                    let screen_offset = (row - inner_y) as usize;
                    if let Some(line) = resolve_screen_line(app, screen_offset) {
                        let (start, end) = if anchor <= line {
                            (anchor, line)
                        } else {
                            (line, anchor)
                        };
                        app.viewer.selection =
                            crate::viewer::LineSelection::Selected { start, end };
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // dividerのドラッグを終える: 最終的な比率を一度だけ永続化する
            // （ドラッグ中はイベントごとの設定書き込みを意図的に省略している）。
            if app.layout.divider_drag.take().is_some() {
                app.layout.divider_hover = geom.divider_at(col, row);
                app.persist_layout();
                return;
            }
            // ガターのドラッグを終える: そのための（単一行または範囲の）選択を
            // 確定させ、コメント作成欄を開く。
            let was_dragging = app.viewer.click.gutter_drag_anchor.take().is_some();
            if was_dragging {
                open_viewer_comment(app);
            }
        }
        MouseEventKind::Moved => {
            // メニューのホバーを最初にチェックし、メニューが開いている間はイベント
            // 全体を横取りする — 開いているドロップダウンの下にあるパネルが、
            // あたかも到達可能であるかのように光ってはいけない。
            if crate::menu::mouse::handle_menu_hover(app, col, row) {
                return;
            }

            // カーソル下のdividerをリサイズのアフォーダンスとして光らせる —
            // col-/row-resizeマウスカーソルのターミナル上の代替表現（単なるホバーは
            // 比率を変更しない。変更するのはドラッグのみ）。
            app.layout.divider_hover = geom
                .divider_at(col, row)
                .filter(|&d| divider_draggable(app, d));

            // worktreeバーのチップ / [x]のホバーを追跡する。カーソルがバーの1行上に
            // ない限り常にNoneになり、これは「マウスがバーから離れた」ことの
            // クリアも兼ねる。
            app.wtbar.hover = if wtbar_area.height > 0 && row == wtbar_area.y {
                app.wtbar.hits.at(col)
            } else {
                None
            };

            // Claude/Shellターミナルのタブバーの [x] 閉じるボタンも同様。
            // ターミナル列であることもゲート条件に含めている。そうしないと、
            // タブバーの行がExplorer/Viewer列の無関係な行と一致してしまう
            // ことがある。
            app.terminal.claude.tab_hover = if col >= viewer_end && row == terminal_claude_y {
                app.terminal.claude.tab_hits.at(col)
            } else {
                None
            };
            app.terminal.shell.tab_hover = if col >= viewer_end && row == terminal_split_y {
                app.terminal.shell.tab_hits.at(col)
            } else {
                None
            };

            // Explorerファイルツリーの行ハイライトのホバーを追跡する。カーソルが
            // ツリーの行の上にない場合（列が違う、Explorerの下半分、リストより
            // 上、など）は常にNoneになり、これがマウスが離れたときにホバーを
            // クリアするのに必要な処理そのものになる — 「ツリーから離れたか」を
            // 別途チェックする必要はない。
            let (tree_row, changes_row) = app.explorer_hover(col, row);
            app.list_hover.explorer_tree.set(tree_row);

            // revidere の状態チップ。カーソルがそこから外れれば false に戻るので、
            // 離れたときの消灯も同じ 1 行で済む。
            app.revidere.badge_hover = revidere_badge_hit(app, col, row, &geom);

            // Explorer の下半分、変更ファイル一覧についても同様。
            app.list_hover.diff_list.set(changes_row);

            // viewerパネルのガターハイライト用にホバー行を追跡する。レンダリング
            // 済みmarkdownはガターも行単位のハイライトも描画せず、その行は
            // ソース行でもないので、カーソルがパネルの外にあるかのように下の
            // 「全てクリアする」分岐に入る。
            let inner_y = main_area.y + 1;
            if !app.viewer.is_showing_rendered_markdown() && col >= explorer_end && col < viewer_end && row >= inner_y && row < main_area.y + main_area.height.saturating_sub(1) {
                let line_offset = (row - inner_y) as usize;
                let inner_x = explorer_end + 1;
                let gutter_w = app.viewer.gutter_total_width();
                // コメントマーカー列（左）と2セル分の「+」バッジ列（右）を含める
                // ことで、カーソルが左マージン全体の上にある間は「+」ボタンが
                // 光ったまま（かつクリック可能）になる。
                let badge_w: u16 = 2;
                let marker_w = crate::viewer::COMMENT_MARKER_W;
                let on_gutter =
                    col >= inner_x && col < inner_x + marker_w + gutter_w + badge_w;

                // diff表示とファイル内容表示はどちらもscreen_row_mapを埋めるように
                // なった（diff表示はインラインのコメントスレッドを差し込む）ので、
                // 1回の画面行の参照でどちらのモードでもホバー中の行を解決できる。
                let resolved = resolve_screen_line(app, line_offset);
                app.viewer.click.hover_line = resolved;
                app.viewer.click.hover_gutter_line = if on_gutter { resolved } else { None };

                // 折りたたみマーカーの上にいる間だけ、その範囲の端から端までを
                // マーカー列の罫線で示す。
                let on_fold = !app.viewer.diff_view.diff_mode
                    && in_fold_zone(col, inner_x + marker_w + gutter_w);
                app.viewer.content
                    .folds
                    .set_hover(if on_fold { resolved } else { None });

                let has_jump_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
                    || mouse.modifiers.contains(KeyModifiers::CONTROL);

                // 共有の抽出処理: マウスのcontent列の下にあるシンボル（あれば）と、
                // その行、0始まりのcontent列。下のジャンプ下線と自動ホバー
                // ポップアップの両方がこれを必要とする。違うのはどのcandidate
                // setterがそれを消費するかと、diffモードによる制限（下記参照）
                // だけ。
                let gutter_w = app.viewer.gutter_total_width();
                let inner_x = explorer_end + 1;
                let badge_w: u16 = 2;
                let content_start_x =
                    inner_x + crate::viewer::COMMENT_MARKER_W + gutter_w + badge_w;
                let symbol_here = if col >= content_start_x {
                    resolve_screen_line(app, line_offset).and_then(|line_1| {
                        let content_col =
                            (col - content_start_x) as usize + app.viewer.content.h_scroll;
                        app.viewer.content
                            .file_content
                            .get(line_1 - 1)
                            .and_then(|text| {
                                crate::viewer::code_nav::masked_symbol_at_column(
                                    text,
                                    content_col,
                                    line_1,
                                    &app.viewer.content.code_mask,
                                )
                                .map(|(symbol, start, end)| (symbol, line_1, start, end))
                            })
                    })
                } else {
                    None
                };

                // ジャンプ用の下線: ジャンプ可能なシンボルの上に静止すれば修飾キー
                // なしで表示される。色だけがhas_jump_modifierに依存する
                // （tick_underline_hoverで解決）。ここでも!diff_modeに限定して
                // いるのは、実際のCmd+クリックによるジャンプハンドラ
                // （crate::viewer::mouse）自体が!diff_mode限定だから。diff表示で下線を
                // 出すと、クリックしても実現できないジャンプを約束することに
                // なってしまう。
                if app.viewer.diff_view.diff_mode {
                    app.set_underline_candidate(None, has_jump_modifier);
                } else {
                    app.set_underline_candidate(symbol_here.clone(), has_jump_modifier);
                }

                // 自動ホバーポップアップの候補: 同じ抽出結果を使うが、diffモードに
                // よる制限はない（ポップアップは読み取り専用なので、実行できない
                // アフォーダンスを見せてしまうことはない）。修飾キーも不要 —
                // 以前から変わっていない。
                let auto_cand = symbol_here.map(|(symbol, line_1, start, end)| {
                    let anchor_col = content_start_x
                        + (start.saturating_sub(app.viewer.content.h_scroll)) as u16;
                    (symbol, line_1, row, anchor_col, start, end)
                });
                app.set_mouse_hover_candidate(auto_cand);
            } else {
                app.viewer.click.hover_line = None;
                app.viewer.click.hover_gutter_line = None;
                app.viewer.content.folds.set_hover(None);
                app.set_underline_candidate(None, false);
                app.set_mouse_hover_candidate(None);
            }
        }
        _ => {}
    }
}
