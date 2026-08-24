//! tmux 風パネルリサイズ: キーボードでの境界移動、マウスドラッグでの境界追従、
//! 両方の入力経路が共有するクランプ済みパーセンテージ計算。

use super::App;

/// tmux 風ペインリサイズの方向。フォーカス中のパネルを基準にする。
///
/// 意味は tmux の resize-pane -L/-R/-U/-D と同じ: フォーカス中のパネルは
/// 指定方向側で隣接パネルと共有する境界を動かすことで、その方向へ広がる。
/// その方向に隣接パネルがない場合（端に接している場合）は反対側の境界が
/// 代わりに動き、パネルは縮む。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDir {
    Left,
    Right,
    Up,
    Down,
}

/// マウスでつかんでドラッグしリサイズできるパネル境界。
///
/// 各バリアントは、キーボード（Ctrl+Alt+矢印）のリサイズを駆動するのと同じ
/// クランプ済みステートミューテータにマップされる。つまりマウスとキーボードは
/// レイアウト比率について単一の真実源を共有する（[App::drag_divider_to] が
/// このマッピングを解決する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Explorer列とViewer列の間の垂直境界。
    ExplorerViewer,
    /// Viewer列とTerminal列の間の垂直境界。
    ViewerTerminal,
    /// Explorerのファイルツリーと変更ファイル一覧の間の水平境界。
    ExplorerSplit,
    /// ClaudeターミナルとShellターミナルのペインの間の水平境界。
    TerminalSplit,
}

impl App {
    pub(super) fn cmd_toggle_panel_expand(&mut self) {
        if self.expanded_panel == Some(self.focus) {
            self.expanded_panel = None;
        } else {
            self.expanded_panel = Some(self.focus);
        }
    }

    /// 水平方向のペインリサイズ1回につき列境界を動かすステップ幅（パーセントポイント）。
    const RESIZE_STEP_PCT: u16 = 5;
    /// 3つの列（Explorer、Viewer、Terminal）それぞれの最小幅パーセント。これにより
    /// tmux 風リサイズで列が消滅することはない。
    const MIN_COL_PCT: u16 = 10;
    /// 垂直方向のペインリサイズ1回につきClaude/Shell境界を動かすステップ幅
    /// （パーセントポイント）。
    const TERMINAL_SPLIT_STEP: u16 = 5;
    /// 実行時のClaude領域パーセントの上下限。2つのターミナルペインのどちらも
    /// 消えないよう、それぞれに最低限を残す。pub(super) なのは
    /// app::appearance もライブリロードされた config の値をこの上下限で
    /// クランプするため。
    pub(super) const TERMINAL_SPLIT_MIN: u16 = 20;
    pub(super) const TERMINAL_SPLIT_MAX: u16 = 80;

    /// フォーカス中のパネルを dir 方向へ広げる形で、tmux 風にリサイズする。
    ///
    /// フォーカス中のパネルと方向を、調整可能な3つの境界のいずれか
    /// （Explorer|Viewer、Viewer|Terminal、Claude|Shell）にマップする。フォーカス中の
    /// パネルは、その方向側で隣接パネルと共有する境界を動かすことで dir へ広がる。
    /// 端に接している場合は唯一持っている境界を動かし、代わりに縮む —
    /// resize-pane -L/-R/-U/-D と同じ挙動。中央（Viewer）列は両方の境界を
    /// 押せるので、縮むことしかできない窮屈なペインにはならない。
    pub fn resize_focused_pane(&mut self, dir: ResizeDir) {
        use super::focus::Focus;
        let step = Self::RESIZE_STEP_PCT as i16;
        let changed = match dir {
            ResizeDir::Left | ResizeDir::Right => {
                let grow_right = matches!(dir, ResizeDir::Right);
                match self.focus {
                    // worktree ストリップは全幅で、3つのリサイズ可能な列の1つではない —
                    // ここからリサイズするものは何もない。2 列ビューも同様に、
                    // 3 列レイアウトの外にいるので動かす境界を持たない。
                    Focus::Worktree | Focus::Revidere => false,
                    // 最左列: 左右キーはExplorer|Viewer境界を動かす。
                    Focus::Explorer => {
                        self.move_explorer_viewer_divider(if grow_right { step } else { -step })
                    }
                    // 中央列はdirが向く側の境界を押す。
                    Focus::Viewer => {
                        if grow_right {
                            self.move_viewer_terminal_divider(step)
                        } else {
                            self.move_explorer_viewer_divider(-step)
                        }
                    }
                    // 最右列: 左キーで広がり（Viewerが縮む）、右キーで縮む。
                    Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => {
                        self.move_viewer_terminal_divider(if grow_right { step } else { -step })
                    }
                }
            }
            ResizeDir::Up | ResizeDir::Down => {
                // 垂直分割を持つ列は2つ: ターミナル（Claude/Shell）とExplorer
                // （ファイルツリー / 変更ファイル一覧）。Downで上側ペインが広がり、
                // Upで縮む。
                let down = matches!(dir, ResizeDir::Down);
                match self.focus {
                    Focus::TerminalClaude | Focus::TerminalShell => {
                        let step = Self::TERMINAL_SPLIT_STEP as i16;
                        self.adjust_terminal_split(if down { step } else { -step })
                    }
                    Focus::Explorer => {
                        let step = Self::TERMINAL_SPLIT_STEP as i16;
                        self.adjust_explorer_split(if down { step } else { -step })
                    }
                    _ => false,
                }
            }
        };
        // キー押下ごとに1回だけ永続化する（比率が実際に動いたときだけ —
        // クランプの下限に当たったリサイズは何も書き込まない）。マウスドラッグの
        // 経路はリリース時に1回だけ永続化するので、両方とも同じクランプ済み
        // ミューテータを共有しつつ、中間ステップごとにconfigを書き込むことはない。
        if changed {
            self.persist_layout();
        }
    }

    /// divider をドラッグし、その境界がスクリーンセル座標 (col, row) の
    /// マウスに追従するようにする。キーボードリサイズと同じクランプ済み
    /// ミューテータを再利用する。比率が実際に動いたかを返す。永続化は
    /// **行わない** — 呼び出し側がドラッグ終了時に一度だけconfigを書き込み、
    /// マウスイベントごとのディスク書き込みを避ける。
    pub fn drag_divider_to(&mut self, divider: Divider, col: u16, row: u16) -> bool {
        // (Copy な) ジオメトリを、ミューテータのために &mut self を取る前に
        // スナップショットしておく。
        let main = self.layout.cache.main_area;
        let explorer_col = self.layout.cache.columns[1];
        let terminal_col = self.layout.cache.columns[3];
        match divider {
            // 垂直方向の境界: パーセンテージはメイン領域の幅に対する割合。
            Divider::ExplorerViewer => {
                if main.width == 0 {
                    return false;
                }
                let target_px = col.saturating_sub(main.x);
                let target_pct = (target_px as u32 * 100 / main.width as u32) as i16;
                let delta = target_pct - self.config.layout.explorer_width_pct as i16;
                self.move_explorer_viewer_divider(delta)
            }
            Divider::ViewerTerminal => {
                if main.width == 0 {
                    return false;
                }
                // 境界は (Explorer + Viewer) の右端にある。Explorerを固定すると、
                // 目標のViewerパーセントはこの合計幅からExplorerを引いたもの。
                let combined_px = col.saturating_sub(main.x);
                let combined_pct = (combined_px as u32 * 100 / main.width as u32) as i16;
                let target_v = combined_pct - self.config.layout.explorer_width_pct as i16;
                let delta = target_v - self.config.layout.viewer_width_pct as i16;
                self.move_viewer_terminal_divider(delta)
            }
            // 水平方向の境界: パーセンテージはその列の高さに対する割合。
            Divider::ExplorerSplit => {
                if explorer_col.height == 0 {
                    return false;
                }
                let target_px = row.saturating_sub(explorer_col.y);
                let target_pct = (target_px as u32 * 100 / explorer_col.height as u32) as i16;
                let delta = target_pct - self.config.layout.explorer_split_pct as i16;
                self.adjust_explorer_split(delta)
            }
            Divider::TerminalSplit => {
                if terminal_col.height == 0 {
                    return false;
                }
                let target_px = row.saturating_sub(terminal_col.y);
                let target_pct = (target_px as u32 * 100 / terminal_col.height as u32) as i16;
                let delta = target_pct - self.layout.terminal_split_pct as i16;
                self.adjust_terminal_split(delta)
            }
        }
    }

    /// Explorer|Viewer境界を delta ポイント分動かす（正の値は右方向、Explorerを
    /// 広げてViewerを縮める）。Terminal幅は保存される。ExplorerもViewerも
    /// [Self::MIN_COL_PCT] を下回らないようクランプされる。比率が変わったかを
    /// 返す。永続化は呼び出し側が行う。
    fn move_explorer_viewer_divider(&mut self, delta: i16) -> bool {
        let (new_e, new_v) = clamp_ev_divider(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            delta,
            Self::MIN_COL_PCT,
        );
        if new_e == self.config.layout.explorer_width_pct {
            return false;
        }
        self.config.layout.explorer_width_pct = new_e;
        self.config.layout.viewer_width_pct = new_v;
        self.after_horizontal_resize();
        true
    }

    /// Viewer|Terminal境界を delta ポイント分動かす（正の値は右方向、Viewerを
    /// 広げてTerminalを縮める）。Explorer幅は変わらない。ViewerもTerminalも
    /// [Self::MIN_COL_PCT] を下回らないようクランプされる。比率が変わったかを
    /// 返す。永続化は呼び出し側が行う。
    fn move_viewer_terminal_divider(&mut self, delta: i16) -> bool {
        let new_v = clamp_vt_divider(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            delta,
            Self::MIN_COL_PCT,
        );
        if new_v == self.config.layout.viewer_width_pct {
            return false;
        }
        self.config.layout.viewer_width_pct = new_v;
        self.after_horizontal_resize();
        true
    }

    /// 列リサイズ共通の後処理: 再描画して新しい分割をフラッシュ表示する。
    /// 永続化は呼び出し側に任せる（キーボード: キー押下ごと、マウス: ドラッグ
    /// リリース時）。
    fn after_horizontal_resize(&mut self) {
        self.dirty.mark_all();
        let e = self.config.layout.explorer_width_pct;
        let v = self.config.layout.viewer_width_pct;
        let t = 100u16.saturating_sub(e.saturating_add(v));
        self.set_status_info(format!(
            "Layout: Explorer {e}% / Viewer {v}% / Terminal {t}%"
        ));
    }

    /// 実行時のClaude領域高さパーセントを delta ポイント分調整する。
    /// ClaudeとShell両方のペインが使える最低限を保つようクランプされる。
    /// 正の delta はClaudeペインを広げ（Shellを縮め）、負の値はShellを
    /// 広げる。結果の分割をフラッシュ表示する。比率が変わったかを返す。
    /// 永続化は呼び出し側が行う。
    fn adjust_terminal_split(&mut self, delta: i16) -> bool {
        let next = (self.layout.terminal_split_pct as i16 + delta).clamp(
            Self::TERMINAL_SPLIT_MIN as i16,
            Self::TERMINAL_SPLIT_MAX as i16,
        ) as u16;
        if next == self.layout.terminal_split_pct {
            return false;
        }
        self.layout.terminal_split_pct = next;
        // メモリ上のconfigも同期させ、永続化時のappearanceスナップショットと
        // 一致させておく — これによりconfigウォッチャーのリロードが no-op になり
        // （スナップショットが異なるときのみ反応する）、自己書き込みループを避ける。
        self.config.layout.terminal_split_pct = next;
        self.dirty.mark_all();
        self.set_status_info(format!(
            "Terminal split: Claude {next}% / Shell {}%",
            100 - next
        ));
        true
    }

    /// Explorer列のファイルツリー高さパーセントを delta ポイント分調整する
    /// （正の値でファイルツリーが広がり、変更ファイル一覧が縮む）。両方の
    /// パネルが使える最低限を保つようクランプされる。フラッシュ表示する。
    /// 比率が変わったかを返す。永続化は呼び出し側が行う。
    fn adjust_explorer_split(&mut self, delta: i16) -> bool {
        let next = (self.config.layout.explorer_split_pct as i16 + delta).clamp(
            Self::TERMINAL_SPLIT_MIN as i16,
            Self::TERMINAL_SPLIT_MAX as i16,
        ) as u16;
        if next == self.config.layout.explorer_split_pct {
            return false;
        }
        self.config.layout.explorer_split_pct = next;
        self.dirty.mark_all();
        self.set_status_info(format!(
            "Explorer split: tree {next}% / changed files {}%",
            100 - next
        ));
        true
    }

    /// 現在のパネル比率を config.toml に永続化する。ベストエフォート:
    /// 書き込み失敗はログに残すのみで致命的エラーにはしない
    /// （メモリ上のレイアウトはそのまま適用され続ける）。
    pub(crate) fn persist_layout(&self) {
        if let Err(e) = crate::config::persist_layout_proportions(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            self.layout.terminal_split_pct,
            self.config.layout.explorer_split_pct,
        ) {
            log::warn!("failed to persist layout proportions: {e}");
        }
    }
}

/// Explorer|Viewer境界を delta ポイント分動かした後の新しい
/// (explorer, viewer) 幅パーセントを計算する。Explorer+Viewerの合計は
/// 保存され（Terminal幅は変わらない）、両方の列とも >= min に保たれる。
/// 列を下限より下げてしまう delta はクランプされるので、境界は
/// 行き過ぎず境目で止まる。
fn clamp_ev_divider(explorer: u16, viewer: u16, delta: i16, min: u16) -> (u16, u16) {
    let e = explorer as i16;
    let v = viewer as i16;
    let min = min as i16;
    let upper = (e + v - min).max(min);
    let new_e = (e + delta).clamp(min, upper);
    (new_e as u16, (e + v - new_e) as u16)
}

/// Viewer|Terminal境界を delta ポイント分動かした後の新しいViewer幅
/// パーセントを計算する。Explorerは変わらない。Viewerと暗黙のTerminal列
/// （100 - explorer - viewer）はそれぞれ >= min に保たれる。
fn clamp_vt_divider(explorer: u16, viewer: u16, delta: i16, min: u16) -> u16 {
    let e = explorer as i16;
    let v = viewer as i16;
    let min = min as i16;
    // Terminal = 100 - E - V なので、新しいVは [min, 100 - E - min] に保つ。
    let upper = (100 - e - min).max(min);
    (v + delta).clamp(min, upper) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    // tmux 風ペインリサイズの境界計算

    const MIN: u16 = 10;

    #[test]
    fn ev_divider_moves_space_between_explorer_and_viewer() {
        // Explorerを広げる (delta +5) とViewerから5ポイント奪う。Terminal
        // （保存される残り）は変わらない。
        assert_eq!(clamp_ev_divider(24, 38, 5, MIN), (29, 33));
        // Viewerを広げる (delta -5) とViewerに5ポイント戻る。
        assert_eq!(clamp_ev_divider(24, 38, -5, MIN), (19, 43));
        // Explorer + Viewerは常に保存される。
        let (e, v) = clamp_ev_divider(24, 38, 5, MIN);
        assert_eq!(e + v, 62);
    }

    #[test]
    fn ev_divider_clamps_at_min_floor() {
        // 大きく縮めてもExplorerはMINを下回れない。
        assert_eq!(clamp_ev_divider(12, 50, -5, MIN), (10, 52));
        // Explorerが広がろうとしてもViewerはMINを下回れない。
        assert_eq!(clamp_ev_divider(50, 12, 5, MIN), (52, 10));
    }

    #[test]
    fn vt_divider_protects_the_terminal_column() {
        // Explorer 24, Viewer 38 → Terminal 38。Viewerを右に広げるとTerminalを
        // 侵食するが、そのMIN下限を超えることはない: 最大Viewer = 100 - 24 - 10 = 66。
        assert_eq!(clamp_vt_divider(24, 38, 5, MIN), 43);
        assert_eq!(clamp_vt_divider(24, 64, 5, MIN), 66); // クランプ済み、Terminal=10
        // Viewerを縮める（Terminalを広げる）のはViewer = MINで下限になる。
        assert_eq!(clamp_vt_divider(24, 12, -5, MIN), 10);
    }

    #[test]
    fn dividers_never_let_a_column_vanish() {
        // 全範囲でdeltaを掃引する。3列すべてが常に >= MIN を保つこと。
        for delta in [-50i16, -20, -5, 5, 20, 50] {
            let (e, v) = clamp_ev_divider(24, 38, delta, MIN);
            let t = 100u16.saturating_sub(e + v);
            assert!(
                e >= MIN && v >= MIN && t >= MIN,
                "ev delta={delta}: {e}/{v}/{t}"
            );

            let v2 = clamp_vt_divider(24, 38, delta, MIN);
            let t2 = 100u16.saturating_sub(24 + v2);
            assert!(v2 >= MIN && t2 >= MIN, "vt delta={delta}: 24/{v2}/{t2}");
        }
    }
}
