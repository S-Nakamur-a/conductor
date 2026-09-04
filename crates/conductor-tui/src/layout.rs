//! 画面の区画を決める純関数。render とマウスのヒット判定の両方がこれを呼ぶので、
//! 描画の副産物として座標を持ち回らない。

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
    RevidereOrder,
    RevidereDiff,
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
    }
}

/// 動かせる境界。3 列の幅は 2 本の縦境界で、2 つの列の上下分割は横境界で決まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    ExplorerViewer,
    ViewerTerminal,
    ExplorerSplit,
    TerminalSplit,
}

/// レビューの左列 (読む順) が取る幅の割合。項目の見出しが 2〜3 行に収まりつつ、
/// diff 側に十分な幅が残る配分。
const REVIDERE_ORDER_PCT: u16 = 32;

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
    if !ws.chrome.maximized {
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

    // レビューは 3 列アコーディオンと重ならない。最大化とも無関係に main を 2 つに割る。
    if ws.focus == Focus::Revidere {
        // 概要は 1 列。
        if !ws.panels.revidere.showing_overview() {
            let order_w = main.width * REVIDERE_ORDER_PCT / 100;
            regions.push((
                Region::RevidereOrder,
                Rect::new(main.x, main.y, order_w, main.height),
            ));
            regions.push((
                Region::RevidereDiff,
                Rect::new(main.x + order_w, main.y, main.width - order_w, main.height),
            ));
        } else {
            regions.push((Region::RevidereDiff, main));
        }
    } else if ws.chrome.maximized {
        match ws.focus {
            Focus::Worktree | Focus::Explorer => push_explorer(&mut regions, main),
            Focus::Editor => regions.push((Region::Editor, main)),
            // Revidere は上で返しているのでここには来ない。
            Focus::Viewer | Focus::Revidere => regions.push((Region::Viewer, main)),
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
        for (maximized, focus, overview) in [
            (false, Focus::Explorer, false),
            (true, Focus::Explorer, false),
            (false, Focus::Editor, false),
            (true, Focus::Editor, false),
            (false, Focus::Revidere, false),
            (false, Focus::Revidere, true),
            (true, Focus::Revidere, false),
        ] {
            ws.chrome.maximized = maximized;
            ws.focus = focus;
            ws.panels.revidere.show_overview(overview);
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
        for focus in [Focus::Explorer, Focus::Editor, Focus::Revidere] {
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

    #[test]
    fn レビューは_2_列で_main_を占め概要では_1_列になる() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::Revidere;
        ws.panels.revidere.show_overview(false);
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let order = l.rect(Region::RevidereOrder).expect("読む順の列");
        let diff = l.rect(Region::RevidereDiff).expect("diff の列");
        assert_eq!(order.x + order.width, diff.x);
        assert!(
            l.rect(Region::TerminalClaude).is_none() && l.rect(Region::Viewer).is_none(),
            "アコーディオンの列は隠れる"
        );

        ws.panels.revidere.show_overview(true);
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        assert!(l.rect(Region::RevidereOrder).is_none());
        assert_eq!(l.rect(Region::RevidereDiff).map(|r| r.width), Some(120));
    }
}
