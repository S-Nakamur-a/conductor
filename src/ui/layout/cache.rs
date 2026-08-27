//! フレーム毎にキャッシュされるレイアウト矩形と、その元になるアコーディオン幅の計算。

use ratatui::layout::{Constraint, Layout, Rect};

/// メニューバーが占める行数。定数――バーは常に描画されるためキャッシュキーには含めない
/// （アプリ状態がこれを変えることはない）。
const MENUBAR_HEIGHT: u16 = 1;

/// フレーム毎に一度だけ計算されるレイアウト矩形のキャッシュ。
/// render_ui、マウスイベントハンドラ、PTY サイジング、装飾表示で共有される。
#[derive(Default, Clone)]
pub struct LayoutCache {
    /// このキャッシュを計算した時のフレーム領域（キャッシュキー）。
    pub frame_area: Rect,
    /// このキャッシュを計算した時の最大化パネル状態（キャッシュキー）。
    pub expanded_panel: Option<crate::types::Focus>,
    /// 通知バーが表示されていたか（キャッシュキー）。
    pub has_notifications: bool,
    /// このキャッシュ計算時に使った Explorer カラム幅の割合（キャッシュキー）。
    pub explorer_width_pct: u16,
    /// このキャッシュ計算時に使った Viewer カラム幅の割合（キャッシュキー）。
    pub viewer_width_pct: u16,
    /// ターミナルカラム内での Claude Code 領域の高さの割合（キャッシュキー）。
    pub terminal_split_pct: u16,
    /// Explorer カラム内でのファイルツリー高さの割合（キャッシュキー）。
    pub explorer_split_pct: u16,
    /// タイトルバー領域。
    pub title_area: Rect,
    /// メニューバー領域――タイトルバー直下の1行。worktree ストリップと違い、
    /// パネル最大化中でも非表示にしない。最大化を解除しないと開けないメニューは
    /// 使われなくなるし、覚えていないコマンドを呼びたくなるのはまさに
    /// パネルを最大化している時だからである。
    pub menubar_area: Rect,
    /// worktree 監視ストリップ領域（全幅、メニューバーと main の間）。
    pub wtbar_area: Rect,
    /// メインコンテンツ領域（タイトルバーとステータスバーの間）。
    pub main_area: Rect,
    /// ステータスバー領域。
    pub status_area: Rect,
    /// カラム領域: [worktree, explorer, viewer, terminal]。
    pub columns: [Rect; 4],
    /// Explorer パネルの垂直分割の中間 Y 座標。
    pub explorer_mid_y: u16,
    /// ターミナル分割: [claude_area, shell_area]。
    pub terminal_split: [Rect; 2],
}

impl LayoutCache {
    /// 入力が変化していればレイアウトを再計算する。更新した場合は true を返す。
    pub fn update(
        &mut self,
        frame_area: Rect,
        expanded_panel: Option<crate::types::Focus>,
        has_notifications: bool,
        layout: &crate::config::LayoutConfig,
        terminal_split_pct: u16,
    ) -> bool {
        if self.frame_area == frame_area
            && self.expanded_panel == expanded_panel
            && self.has_notifications == has_notifications
            && self.explorer_width_pct == layout.explorer_width_pct
            && self.viewer_width_pct == layout.viewer_width_pct
            && self.terminal_split_pct == terminal_split_pct
            && self.explorer_split_pct == layout.explorer_split_pct
        {
            return false;
        }

        self.frame_area = frame_area;
        self.expanded_panel = expanded_panel;
        self.has_notifications = has_notifications;
        self.explorer_width_pct = layout.explorer_width_pct;
        self.viewer_width_pct = layout.viewer_width_pct;
        // ターミナル分割は実行時に調整可能（shell の拡大縮小）なので、
        // config からではなくパラメータとして渡ってくる。
        self.terminal_split_pct = terminal_split_pct;
        self.explorer_split_pct = layout.explorer_split_pct;

        // worktree 監視ストリップは、パネル最大化中は非表示にして
        // 最大化パネルに全高を与える。
        let wtbar_height: u16 = if expanded_panel.is_some() { 0 } else { 1 };

        let outer = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(MENUBAR_HEIGHT),
            Constraint::Length(wtbar_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame_area);

        self.title_area = outer[0];
        self.menubar_area = outer[1];
        self.wtbar_area = outer[2];
        self.main_area = outer[3];
        self.status_area = outer[4];

        let (left_w, explorer_w, viewer_w) = accordion_widths(
            expanded_panel,
            self.main_area.width,
            layout.explorer_width_pct,
            layout.viewer_width_pct,
        );
        let right_w = self
            .main_area
            .width
            .saturating_sub(left_w.saturating_add(explorer_w).saturating_add(viewer_w));

        let cols = Layout::horizontal([
            Constraint::Length(left_w),
            Constraint::Length(explorer_w),
            Constraint::Length(viewer_w),
            Constraint::Length(right_w),
        ])
        .split(self.main_area);

        self.columns = [cols[0], cols[1], cols[2], cols[3]];

        // Explorer 50/50 の垂直分割
        let changed_files_pct = 100u16.saturating_sub(self.explorer_split_pct);
        let explorer_split = Layout::vertical([
            Constraint::Percentage(self.explorer_split_pct),
            Constraint::Percentage(changed_files_pct),
        ])
        .split(self.columns[1]);
        self.explorer_mid_y = explorer_split[1].y;

        // ターミナルの垂直分割: Claude Code が terminal_split_pct%、
        // 残りを shell が受け取る。
        let shell_pct = 100u16.saturating_sub(terminal_split_pct);
        let terminal_split = Layout::vertical([
            Constraint::Percentage(terminal_split_pct),
            Constraint::Percentage(shell_pct),
        ])
        .split(self.columns[3]);
        self.terminal_split = [terminal_split[0], terminal_split[1]];

        true
    }
}

/// パネルの最大化状態に基づいてアコーディオンパネルの幅を計算する。
///
/// (left_width, explorer_width, viewer_width) を返す。right パネルは残り全部を
/// 受け取る。explorer_pct と viewer_pct は設定された割合（0〜100）で、
/// デフォルト（未最大化）のレイアウトでのみ使われる。
pub(crate) fn accordion_widths(
    expanded_panel: Option<crate::types::Focus>,
    total_width: u16,
    explorer_pct: u16,
    viewer_pct: u16,
) -> (u16, u16, u16) {
    use crate::types::Focus;

    match expanded_panel {
        Some(Focus::Worktree) => (total_width, 0, 0),
        Some(Focus::Explorer) => (0, total_width, 0),
        Some(Focus::Viewer) => (0, 0, total_width),
        // 最大化したエディタは explorer 側の枠を通じて全幅を得る。
        // render_ui は explorer+viewer カラムを1つの editor 領域として統合するので、
        // explorer 側の枠に全幅を与える（viewer は0）ことで、ターミナルカラムが
        // 消えたフルスクリーンのエディタになる。
        Some(Focus::Editor) => (0, total_width, 0),
        Some(Focus::TerminalClaude | Focus::TerminalShell) => (0, 0, 0),
        // 2 列ビューは main_area 全体を自分で取るので、列幅は使われない。
        Some(Focus::Revidere) => (0, 0, 0),
        None => {
            // デフォルトの比率。worktree カラムは廃止済み（その状態は上部ストリップに
            // 移った）ので幅は0になり、空いたスペースは explorer と viewer の
            // レビューペインに回る。
            let min_col = 3_u16;
            let explorer = ((total_width as u32 * explorer_pct as u32 / 100) as u16).max(min_col);
            let viewer = ((total_width as u32 * viewer_pct as u32 / 100) as u16).max(min_col);
            (0, explorer, viewer)
        }
    }
}
