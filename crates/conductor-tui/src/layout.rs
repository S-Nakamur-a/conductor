//! 画面の区画を決める純関数。render とマウスのヒット判定の両方がこれを呼ぶので、
//! 描画の副産物として座標を持ち回らない。

use conductor_core::config::LayoutConfig;
use ratatui::layout::Rect;

use crate::workspace::{Focus, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    TitleBar,
    MenuBar,
    WorktreeStrip,
    ExplorerTree,
    ExplorerChanges,
    Viewer,
    /// 埋め込み $EDITOR。Explorer と Viewer の列を併合して占める。
    Editor,
    TerminalClaude,
    TerminalShell,
    FleetList,
    /// 一覧で選んだ行のブランチの変更ファイルか、その worktree のシェル。フォーカスを持つのはシェルのときだけ。
    FleetChanges,
    /// 一覧で選んだセッション。
    FleetSession,
    StatusBar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// 画面全体。モーダルは区画の上に重なるので、区画からは引けない。
    pub area: Rect,
    /// 帯を除いた中央の行。境界のドラッグは、この幅と高さに対する割合で比率を決める。
    pub main: Rect,
    pub regions: Vec<(Region, Rect)>,
}

impl Layout {
    pub fn rect(&self, region: Region) -> Option<Rect> {
        self.regions
            .iter()
            .find(|(r, _)| *r == region)
            .map(|(_, rect)| *rect)
    }

    pub fn hit(&self, x: u16, y: u16) -> Option<Region> {
        self.regions
            .iter()
            .find(|(_, rect)| {
                x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
            })
            .map(|(region, _)| *region)
    }

    /// マウスでつかんで動かせる境界。隣り合う 2 つの枠線のどちらを掴んでも同じ境界になる。
    /// 動かす比率はキーボードのリサイズと同じなので、クランプは [crate::command::exec] が持つ。
    pub fn divider_at(&self, x: u16, y: u16) -> Option<Divider> {
        let touches = |edge: u16, v: u16| v == edge || v + 1 == edge;
        // 境界は右側 (下側) の区画の始まりにある。左 (上) が無い配置では境界も無い。
        let vertical = |right: Region, divider: Divider| {
            let r = self.rect(right)?;
            (r.x > self.main.x && touches(r.x, x) && y >= r.y && y < r.y + r.height)
                .then_some(divider)
        };
        let horizontal = |bottom: Region, divider: Divider| {
            let b = self.rect(bottom)?;
            (b.y > self.main.y && touches(b.y, y) && x >= b.x && x < b.x + b.width)
                .then_some(divider)
        };
        // 縦を先に見る。角では列の幅を動かす方が意図に近い。
        vertical(Region::Viewer, Divider::ExplorerViewer)
            .or_else(|| vertical(Region::TerminalClaude, Divider::ViewerTerminal))
            .or_else(|| horizontal(Region::ExplorerChanges, Divider::ExplorerSplit))
            .or_else(|| horizontal(Region::TerminalShell, Divider::TerminalSplit))
            .or_else(|| vertical(Region::FleetSession, Divider::FleetWidth))
            .or_else(|| horizontal(Region::FleetSession, Divider::FleetStack))
            .or_else(|| horizontal(Region::FleetChanges, Divider::FleetSplit))
    }
}

/// 動かせる境界。3 列の幅は 2 本の縦境界で、2 つの列の上下分割は横境界で決まる。
/// Sessions の画面では、一覧の幅・一覧の下の区画の高さ・縦長のときの一覧の高さの 3 本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    ExplorerViewer,
    ViewerTerminal,
    ExplorerSplit,
    TerminalSplit,
    FleetWidth,
    FleetSplit,
    FleetStack,
}

/// 上下・左右に分ける割合の範囲。外れた値は区画を潰すので、config に書かれていても収める。
pub const SPLIT_MIN: u16 = 20;
pub const SPLIT_MAX: u16 = 80;

/// 一覧の列。2 アカウントの 5h / 7d がバーつきで上の 1 行に入る幅で頭打ちにし、残りは右の区画に回す。
const FLEET_LIST_PCT: u16 = 61;
const FLEET_LIST_MAX: u16 = 125;
/// これより低いと一覧の本文が数行しか残らないので、下区画を出さない。
const FLEET_CHANGES_MIN_HEIGHT: u16 = 20;

/// 1 マスは縦が横のほぼ 2 倍なので、桁数が行数の 2 倍に満たない窓は見た目が縦長。
fn is_portrait(main: Rect) -> bool {
    u32::from(main.width) < u32::from(main.height) * 2
}

fn share(extent: u16, pct: u16) -> u16 {
    (u32::from(extent) * u32::from(pct.clamp(SPLIT_MIN, SPLIT_MAX)) / 100) as u16
}

fn push_fleet(regions: &mut Vec<(Region, Rect)>, main: Rect, cfg: &LayoutConfig) {
    if is_portrait(main) {
        let list_h = share(main.height, cfg.fleet_stack_pct);
        regions.push((
            Region::FleetList,
            Rect::new(main.x, main.y, main.width, list_h),
        ));
        regions.push((
            Region::FleetSession,
            Rect::new(main.x, main.y + list_h, main.width, main.height - list_h),
        ));
        return;
    }
    let list_w = match cfg.fleet_list_width_pct {
        Some(pct) => share(main.width, pct),
        None => share(main.width, FLEET_LIST_PCT).min(FLEET_LIST_MAX),
    };
    let changes_h = if main.height >= FLEET_CHANGES_MIN_HEIGHT {
        share(main.height, cfg.fleet_changes_pct)
    } else {
        0
    };
    let list_h = main.height - changes_h;
    regions.push((Region::FleetList, Rect::new(main.x, main.y, list_w, list_h)));
    if changes_h > 0 {
        regions.push((
            Region::FleetChanges,
            Rect::new(main.x, main.y + list_h, list_w, changes_h),
        ));
    }
    regions.push((
        Region::FleetSession,
        Rect::new(main.x + list_w, main.y, main.width - list_w, main.height),
    ));
}

/// 幅の配分。フォーカスされた列が広がるアコーディオン。
pub fn layout(ws: &Workspace, area: Rect) -> Layout {
    let mut regions = Vec::new();
    let mut y = area.y;
    let mut push_row = |region: Region, height: u16, y: &mut u16| {
        if *y + height <= area.y + area.height {
            regions.push((region, Rect::new(area.x, *y, area.width, height)));
            *y += height;
        }
    };
    push_row(Region::TitleBar, 1, &mut y);
    push_row(Region::MenuBar, 1, &mut y);
    // 一覧は全 repo のものなので、今の repo の worktree の帯は出さない。
    if !ws.chrome.maximized && !ws.chrome.fleet_open {
        push_row(Region::WorktreeStrip, 1, &mut y);
    }
    let status_y = area.y + area.height.saturating_sub(1);
    let main = Rect::new(area.x, y, area.width, status_y.saturating_sub(y));

    let cfg = &ws.config.layout;
    // 最大化してもこの分割は残す。変更ファイル一覧ごと消えると、レビュー中に
    // 広げただけで導線が切れる。
    let push_explorer = |regions: &mut Vec<(Region, Rect)>, area: Rect| {
        let tree_h = area.height * cfg.explorer_split_pct / 100;
        regions.push((
            Region::ExplorerTree,
            Rect::new(area.x, area.y, area.width, tree_h),
        ));
        regions.push((
            Region::ExplorerChanges,
            Rect::new(
                area.x,
                area.y + tree_h,
                area.width,
                area.height.saturating_sub(tree_h),
            ),
        ));
    };

    if ws.chrome.fleet_open {
        push_fleet(&mut regions, main, cfg);
    } else if ws.chrome.maximized {
        match ws.focus {
            Focus::Worktree
            | Focus::Explorer
            | Focus::Fleet
            | Focus::FleetSession
            | Focus::FleetShell => push_explorer(&mut regions, main),
            Focus::Editor => regions.push((Region::Editor, main)),
            Focus::Viewer => regions.push((Region::Viewer, main)),
            Focus::TerminalClaude => regions.push((Region::TerminalClaude, main)),
            Focus::TerminalShell => regions.push((Region::TerminalShell, main)),
        }
    } else {
        let explorer_w = main.width * cfg.explorer_width_pct / 100;
        let viewer_w = main.width * cfg.viewer_width_pct / 100;
        let terminal_w = main.width.saturating_sub(explorer_w + viewer_w);
        let claude_h = main.height * cfg.terminal_split_pct / 100;
        // エディタは Explorer と Viewer の 2 列を併合して 1 区画になる。
        if ws.focus == Focus::Editor {
            regions.push((
                Region::Editor,
                Rect::new(main.x, main.y, explorer_w + viewer_w, main.height),
            ));
        } else {
            push_explorer(
                &mut regions,
                Rect::new(main.x, main.y, explorer_w, main.height),
            );
            regions.push((
                Region::Viewer,
                Rect::new(main.x + explorer_w, main.y, viewer_w, main.height),
            ));
        }
        let tx = main.x + explorer_w + viewer_w;
        regions.push((
            Region::TerminalClaude,
            Rect::new(tx, main.y, terminal_w, claude_h),
        ));
        regions.push((
            Region::TerminalShell,
            Rect::new(tx, main.y + claude_h, terminal_w, main.height - claude_h),
        ));
    }
    regions.push((
        Region::StatusBar,
        Rect::new(area.x, status_y, area.width, 1),
    ));
    Layout {
        area,
        main,
        regions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 全ての区画は_hitで自分に戻る() {
        let mut ws = Workspace::for_test();
        for (maximized, focus) in [
            (false, Focus::Explorer),
            (true, Focus::Explorer),
            (false, Focus::Editor),
            (true, Focus::Editor),
        ] {
            ws.chrome.maximized = maximized;
            ws.focus = focus;
            let l = layout(&ws, Rect::new(0, 0, 120, 40));
            for (region, rect) in &l.regions {
                for (x, y) in [
                    (rect.x, rect.y),
                    (rect.x + rect.width - 1, rect.y + rect.height - 1),
                ] {
                    assert_eq!(l.hit(x, y), Some(*region), "{region:?} at ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn 区画は重ならず画面を埋める() {
        let mut ws = Workspace::for_test();
        for focus in [Focus::Explorer, Focus::Editor] {
            ws.focus = focus;
            let l = layout(&ws, Rect::new(0, 0, 120, 40));
            let total: u32 = l.regions.iter().map(|(_, r)| r.area()).sum();
            assert_eq!(total, 120 * 40, "{focus:?}");
        }
    }

    #[test]
    fn 最大化はフォーカス中の列だけを残す() {
        let mut ws = Workspace::for_test();
        ws.chrome.maximized = true;
        ws.focus = Focus::TerminalShell;
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        assert!(l.rect(Region::TerminalShell).is_some());
        assert!(l.rect(Region::ExplorerTree).is_none());
        assert!(l.rect(Region::WorktreeStrip).is_none());

        ws.focus = Focus::Explorer;
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        assert!(
            l.rect(Region::ExplorerTree).is_some() && l.rect(Region::ExplorerChanges).is_some(),
            "最大化しても Explorer の 2 区画は残る"
        );
    }

    #[test]
    fn 境界は隣り合う枠線のどちらを掴んでも同じになる() {
        let ws = Workspace::for_test();
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let viewer = l.rect(Region::Viewer).unwrap();
        let claude = l.rect(Region::TerminalClaude).unwrap();
        let shell = l.rect(Region::TerminalShell).unwrap();
        let changes = l.rect(Region::ExplorerChanges).unwrap();
        for (x, y, expected) in [
            (viewer.x - 1, viewer.y, Some(Divider::ExplorerViewer)),
            (viewer.x, viewer.y, Some(Divider::ExplorerViewer)),
            (claude.x, claude.y + 3, Some(Divider::ViewerTerminal)),
            (shell.x + 2, shell.y, Some(Divider::TerminalSplit)),
            (shell.x + 2, shell.y - 1, Some(Divider::TerminalSplit)),
            (changes.x + 1, changes.y, Some(Divider::ExplorerSplit)),
            (viewer.x + 5, viewer.y + 5, None),
            (0, 0, None),
        ] {
            assert_eq!(l.divider_at(x, y), expected, "({x},{y})");
        }
    }

    fn fleet(area: Rect, cfg: impl FnOnce(&mut LayoutConfig)) -> Layout {
        let mut ws = Workspace::for_test();
        ws.chrome.fleet_open = true;
        cfg(&mut ws.config.layout);
        layout(&ws, area)
    }

    #[test]
    fn 縦長の一覧は上が一覧で下がセッションの2段になり下の区画を出さない() {
        let l = fleet(Rect::new(0, 0, 80, 60), |_| {});
        let list = l.rect(Region::FleetList).unwrap();
        let session = l.rect(Region::FleetSession).unwrap();
        assert_eq!(l.rect(Region::FleetChanges), None);
        assert_eq!(
            (list.x, list.y, list.width),
            (l.main.x, l.main.y, l.main.width)
        );
        assert_eq!((session.x, session.y), (l.main.x, list.bottom()));
        assert_eq!(session.width, l.main.width);
        assert_eq!(list.height + session.height, l.main.height);

        let edge = fleet(Rect::new(0, 0, 2 * 40, 40 + 3), |_| {});
        assert_eq!(edge.main.width, edge.main.height * 2);
        assert!(
            edge.rect(Region::FleetChanges).is_some(),
            "桁数がちょうど行数の 2 倍なら横長"
        );
    }

    #[test]
    fn 比率を動かしていなければ一覧の区画は前と同じ座標になる() {
        for (width, height) in [(160, 24), (160, 40), (160, 60), (250, 40), (100, 40)] {
            let l = fleet(Rect::new(0, 0, width, height), |_| {});
            let h = l.main.height;
            let changes_h = h * 40 / 100;
            let list = l.rect(Region::FleetList).unwrap();
            let changes = l.rect(Region::FleetChanges).unwrap();
            assert_eq!(list.width, (width * 61 / 100).min(125), "{width}x{height}");
            assert_eq!(
                (list.height, changes.height),
                (h - changes_h, changes_h),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn 掴んで決めた一覧の幅には125桁の上限を掛けない() {
        let l = fleet(Rect::new(0, 0, 250, 40), |c| {
            c.fleet_list_width_pct = Some(70)
        });
        assert_eq!(l.rect(Region::FleetList).unwrap().width, 175);
    }

    #[test]
    fn 範囲外の比率は20から80に収めて区画を潰さない() {
        for pct in [0, 5, 95, 150, u16::MAX] {
            for area in [Rect::new(0, 0, 160, 40), Rect::new(0, 0, 80, 60)] {
                let l = fleet(area, |c| {
                    c.fleet_list_width_pct = Some(pct);
                    c.fleet_changes_pct = pct;
                    c.fleet_stack_pct = pct;
                });
                for (region, rect) in &l.regions {
                    if matches!(region, Region::FleetList | Region::FleetSession) {
                        assert!(rect.width > 0 && rect.height > 0, "{pct} {region:?}");
                    }
                }
                let total: u32 = l.regions.iter().map(|(_, r)| r.area()).sum();
                assert_eq!(
                    total,
                    u32::from(area.width) * u32::from(area.height),
                    "{pct}"
                );
            }
        }
    }

    #[test]
    fn 一覧の画面の境界は枠線のどちらを掴んでも同じになる() {
        let l = fleet(Rect::new(0, 0, 160, 40), |_| {});
        let session = l.rect(Region::FleetSession).unwrap();
        let changes = l.rect(Region::FleetChanges).unwrap();
        for (x, y, expected) in [
            (session.x, session.y + 3, Some(Divider::FleetWidth)),
            (session.x - 1, session.y + 3, Some(Divider::FleetWidth)),
            (changes.x + 2, changes.y, Some(Divider::FleetSplit)),
            (changes.x + 2, changes.y - 1, Some(Divider::FleetSplit)),
            (session.x - 1, changes.y, Some(Divider::FleetWidth)),
            (session.x + 5, session.y, None),
            (changes.x + 2, changes.y + 2, None),
        ] {
            assert_eq!(l.divider_at(x, y), expected, "({x},{y})");
        }

        let tall = fleet(Rect::new(0, 0, 80, 60), |_| {});
        let below = tall.rect(Region::FleetSession).unwrap();
        for (x, y, expected) in [
            (below.x + 4, below.y, Some(Divider::FleetStack)),
            (below.x + 4, below.y - 1, Some(Divider::FleetStack)),
            (below.x, below.y + 4, None),
        ] {
            assert_eq!(tall.divider_at(x, y), expected, "({x},{y})");
        }
    }

    #[test]
    fn 最大化中と最上段の枠線は境界にならない() {
        let mut ws = Workspace::for_test();
        ws.chrome.maximized = true;
        ws.focus = Focus::Viewer;
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let viewer = l.rect(Region::Viewer).unwrap();
        assert_eq!(l.divider_at(viewer.x, viewer.y), None);
        assert_eq!(l.divider_at(viewer.x + 5, viewer.y), None);
    }
}
