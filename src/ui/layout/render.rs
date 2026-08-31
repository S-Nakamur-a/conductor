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
        app.layout.expanded,
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
    super::super::chrome::render_title_bar(frame, title_area, app);

    crate::menu::render::render(frame, menubar_area, app);

    // worktree 監視ストリップ。
    crate::worktree::bar::render(frame, wtbar_area, app);

    // revidere の 2 列ビューは main_area 全体を取る。3 列アコーディオンとは
    // 並ばないので、ターミナル列も含めてここで打ち切る。
    if app.focus.current() == crate::app::Focus::Revidere {
        crate::revidere::render::render(frame, main_area, app);
        super::super::chrome::render_status_bar(frame, status_area, app);
        return;
    }

    let columns = app.layout.cache.columns;

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
        crate::terminal::render::editor::render(frame, region, app);
    } else {
        // カラム1: Explorer（ファイルツリー + 差分リスト）
        app.render_explorer(frame, columns[1]);

        // カラム2: Viewer（ファイル内容）
        if app.viewer.is_current_file_media()
            && let Some(ref rel_path) = app.viewer.content.current_file.clone()
        {
            // 画像も本文と同じ根から読む。current_file は Viewer のツリーの
            // 相対パスなので、別の根に繋ぐと「タイトルは A のファイル、絵は B の
            // ファイル」になり得る。
            let full_path = app.explorer.root().join(rel_path);
            let cols = columns[2].width;
            let rows = columns[2].height;
            app.viewer
                .media_state
                .render_if_needed(&full_path, rel_path, cols, rows);
        }
        crate::viewer::render_panel(frame, columns[2], app);
    }

    // カラム3: ターミナル分割（Claude 80% / Shell 20%）
    let terminal_split = app.layout.cache.terminal_split;
    crate::terminal::render::claude::render(frame, terminal_split[0], app);
    crate::terminal::render::shell::render(frame, terminal_split[1], app);

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
    {
        super::super::panel_overlay::render_panel_overlay(frame, app);
    }

    render_overlays(frame, main_area, app);

    // メニューのドロップダウン
    // コンテンツの中で最後に描画し、各パネルの上に乗せる。main_area より上にある
    // メニューバー行から垂れ下がるので、main_area ではなくフレーム全体でクランプする。
    crate::menu::render::render_dropdown(frame, area, app);

    // ステータスバー
    // ステータスバー右側に worktree のブランチとリポジトリを表示する。
    let _worktree_branch = app
        .worktrees
        .get(app.worktrees.selected_index())
        .map(|w| w.branch.as_str())
        .unwrap_or("");
    super::super::chrome::render_status_bar(frame, status_area, app);
    super::super::chrome::render_worktree_label(
        frame,
        status_area,
        _worktree_branch,
        &app.repo.path,
        &app.appearance.theme,
    );
}

/// s が罫線素片（U+2500..=U+257F）、つまりパネルのボーダー文字で始まるかどうか。
/// テキスト内容に触れずボーダーだけを対象にするために使う。
fn is_border_glyph(s: &str) -> bool {
    matches!(s.chars().next(), Some(c) if ('\u{2500}'..='\u{257F}').contains(&c))
}

/// crossterm では OS のカーソル形状を変えられないので、GUI の col-resize/row-resize の
/// 代わりになる。ドラッグ中はホバーより優先し、1 セルずれても光らせ続ける。
fn highlight_active_divider(frame: &mut Frame, app: &App) {
    use crate::app::Divider;

    let Some(divider) = app.layout.divider_drag.or(app.layout.divider_hover) else {
        return;
    };
    let lc = &app.layout.cache;
    let color = app.appearance.theme.accent;

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
        Divider::TerminalSplit => (
            false,
            lc.terminal_split[1].y.saturating_sub(1),
            lc.columns[3],
        ),
    };

    let buf = frame.buffer_mut();
    if vertical {
        if fixed < area.x || fixed >= area.x.saturating_add(area.width) {
            return;
        }
        for y in area.y..area.y.saturating_add(area.height) {
            if let Some(cell) = buf.cell_mut((fixed, y))
                && is_border_glyph(cell.symbol())
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
                && is_border_glyph(cell.symbol())
            {
                cell.set_fg(color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_glyph_detection() {
        // 罫線素片（細線 + 太線）はボーダーである。
        assert!(is_border_glyph("│"));
        assert!(is_border_glyph("─"));
        assert!(is_border_glyph("┏"));
        assert!(is_border_glyph("┃"));
        // 普通のテキストや空白はボーダーではない。
        assert!(!is_border_glyph("a"));
        assert!(!is_border_glyph(" "));
        assert!(!is_border_glyph(""));
        assert!(!is_border_glyph("✦"));
    }
}
