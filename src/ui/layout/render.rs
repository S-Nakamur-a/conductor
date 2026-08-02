//! トップレベルのフレームレンダラ: 3カラムアコーディオン、ステータス/タイトルバー、
//! リサイズ用ディバイダのハイライトを組み立てる。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;

use super::overlays::render_overlays;

/// トップレベルの UI レンダラ — 3カラムアコーディオンレイアウト + ステータスバー。
pub(crate) fn render_ui(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let has_notifications = !app.terminal.cc_waiting_worktrees.is_empty();

    // レイアウトキャッシュを更新する（変化がなければ何もしない）。
    // app.layout.cache を可変、app.config.layout を不変で借用しているが、
    // 別々の構造体フィールドなので Rust ではこれが許される。
    app.layout.cache.update(
        area,
        app.expanded_panel,
        has_notifications,
        &app.config.layout,
        app.layout.terminal_split_pct,
    );

    let title_area = app.layout.cache.title_area;
    let menubar_area = app.layout.cache.menubar_area;
    let wtbar_area = app.layout.cache.wtbar_area;
    let main_area = app.layout.cache.main_area;
    let status_area = app.layout.cache.status_area;

    // タイトルバー
    super::super::common::render_title_bar(frame, title_area, app);

    // メニューバー（常に表示、タイトル直下）
    super::super::menu_bar::render(frame, menubar_area, app);

    // worktree 監視ストリップ（旧左カラムと、以前あった
    // CC 待機通知バーの後継）
    super::super::worktree_bar::render(frame, wtbar_area, app);

    // アコーディオンのカラム幅（キャッシュから取得）
    let columns = app.layout.cache.columns;

    // カラム0（worktree）は廃止済み――その状態は上部ストリップにある。

    if app.editor.is_some() {
        // 組み込みエディタは Explorer + Viewer カラムを1つの結合された PTY パネルに
        // 置き換える（ターミナルカラムはそのまま）。最大化時は accordion_widths が
        // explorer 側に全幅を、viewer 側に0を与えるので、以下の union はメイン領域
        // 全体になる。
        let region = Rect {
            x: columns[1].x,
            y: columns[1].y,
            width: columns[1].width.saturating_add(columns[2].width),
            height: columns[1].height,
        };
        super::super::editor_panel::render(frame, region, app);
    } else {
        // カラム1: Explorer（ファイルツリー + 差分リスト）
        super::super::explorer_panel::render(frame, columns[1], app);

        // カラム2: Viewer（ファイル内容）
        if app.viewer_state.is_current_file_media()
            && let Some(ref rel_path) = app.viewer_state.content.current_file.clone()
        {
            // 画像も本文と同じ根から読む。current_file は Viewer のツリーの
            // 相対パスなので、別の根に繋ぐと「タイトルは A のファイル、絵は B の
            // ファイル」になり得る。
            let full_path = app.viewer_state.root().join(rel_path);
            let cols = columns[2].width;
            let rows = columns[2].height;
            // Tier B: グラフィックスプロトコル経由のピクセル品質レンダリング。
            let picker = if app.rich.has_graphics() {
                app.rich.picker
            } else {
                None
            };
            app.viewer_state
                .media_state
                .render_if_needed(&full_path, rel_path, cols, rows, picker);
        }
        super::super::viewer_panel::render(frame, columns[2], app);
    }

    // カラム3: ターミナル分割（Claude 80% / Shell 20%）
    let terminal_split = app.layout.cache.terminal_split;
    super::super::terminal_claude::render(frame, terminal_split[0], app);
    super::super::terminal_shell::render(frame, terminal_split[1], app);

    // リサイズのアフォーダンス: hover/ドラッグ中のディバイダを点灯させる
    highlight_active_divider(frame, app);

    // パネル番号オーバーレイ（Alt+/ でトグル）
    // 他のオーバーレイ/モーダルが有効でない時だけ表示する。
    if app.panel_number_overlay.is_visible()
        && app.overlays.active == crate::overlay::ActiveOverlay::None
        && app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::Normal
        && app.review_state.input_mode == crate::review_state::ReviewInputMode::Normal
        && !app.update.is_active()
        && !app.review_state.comment_detail_active
        && app.worktree_mgr.skip_reason.is_none()
    {
        super::super::panel_overlay::render_panel_overlay(frame, app);
    }

    render_overlays(frame, main_area, app);

    // メニューのドロップダウン
    // コンテンツの中で最後に描画し、各パネルの上に乗せる。main_area より上にある
    // メニューバー行から垂れ下がるので、main_area ではなくフレーム全体でクランプする。
    super::super::menu_bar::render_dropdown(frame, area, app);

    // ステータスバー
    // ステータスバー右側に worktree のブランチとリポジトリを表示する。
    let _worktree_branch = app
        .worktrees
        .get(app.worktrees.selected_index())
        .map(|w| w.branch.as_str())
        .unwrap_or("");
    super::super::common::render_status_bar(frame, status_area, app);
    super::super::common::render_worktree_label(
        frame,
        status_area,
        _worktree_branch,
        &app.repo.path,
        &app.theme,
    );

    // リッチモード（Tier A）
    // 完成したフレームに、グラデーションの呼吸するボーダーと Claude 待機時の
    // グロー効果を後処理として加える。パーティモード有効時はスキップする。
    // パーティモードはフォーカス中のボーダーを border_focused との色の一致で
    // 見つけるため、グラデーションを加えるとそれが壊れてしまう。
    if app.rich.is_rich() && !app.party_mode {
        super::super::rich::apply_rich_effects(frame, app);
    }

    // パーティモード（隠しコマンド）
    // 完成したフレームに、レインボーボーダー・きらめくタイトルバー・紙吹雪を
    // （オーバーレイも含めた）全体の上に後処理として重ねる。
    if app.party_mode {
        super::super::party::apply_party_effects(frame, app);
    }
}

/// 現在 hover 中またはドラッグ中のディバイダをテーマのアクセントカラーで塗る。
/// crossterm では OS のカーソル形状を切り替えられないので、GUI でいう
/// col-resize/row-resize カーソルの代わりの表現になる。ドラッグ中はホバーより
/// 優先され、ドラッグ中にカーソルが1セルずれても境界を光らせたままにする。
/// ボーダーのグリフだけ再着色するので、パネルの内容には触れない。
/// リッチ/パーティの後処理より前に実行される。それらはフォーカス中ボーダー色に
/// 一致するセルだけを再着色するので、このアクセント線には影響しない。
fn highlight_active_divider(frame: &mut Frame, app: &App) {
    use crate::app::Divider;

    let Some(divider) = app.layout.divider_drag.or(app.layout.divider_hover) else {
        return;
    };
    let lc = &app.layout.cache;
    let color = app.theme.accent;

    // ディバイダを (is_vertical, 固定座標, 対象領域) に解決する。固定座標は
    // 上/左パネルのボーダーセル（edge - 1）で、これが目に見えるディバイダ線になる。
    let (vertical, fixed, area) = match divider {
        Divider::ExplorerViewer => {
            let edge = lc.columns[1].x.saturating_add(lc.columns[1].width);
            (true, edge.saturating_sub(1), lc.main_area)
        }
        Divider::ViewerTerminal => {
            let edge = lc.columns[2].x.saturating_add(lc.columns[2].width);
            (true, edge.saturating_sub(1), lc.main_area)
        }
        Divider::ExplorerSplit => (false, lc.explorer_mid_y.saturating_sub(1), lc.columns[1]),
        Divider::TerminalSplit => {
            (false, lc.terminal_split[1].y.saturating_sub(1), lc.columns[3])
        }
    };

    let buf = frame.buffer_mut();
    if vertical {
        if fixed < area.x || fixed >= area.x.saturating_add(area.width) {
            return;
        }
        for y in area.y..area.y.saturating_add(area.height) {
            if let Some(cell) = buf.cell_mut((fixed, y))
                && super::super::party::is_border_glyph(cell.symbol())
            {
                cell.set_fg(color);
            }
        }
    } else {
        if fixed < area.y || fixed >= area.y.saturating_add(area.height) {
            return;
        }
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, fixed))
                && super::super::party::is_border_glyph(cell.symbol())
            {
                cell.set_fg(color);
            }
        }
    }
}
