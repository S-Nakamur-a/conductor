//! 画面の区画を決める純関数。render とマウスのヒット判定の両方がこれを呼ぶので、
//! 描画の副産物として座標を持ち回らない。

use ratatui::layout::Rect;

use crate::workspace::{Focus, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    TitleBar,
    MenuBar,
    WorktreeStrip,
    Explorer,
    Viewer,
    TerminalClaude,
    TerminalShell,
    StatusBar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
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
    if !ws.chrome.maximized {
        push_row(Region::WorktreeStrip, 1, &mut y);
    }
    let status_y = area.y + area.height.saturating_sub(1);
    let main = Rect::new(area.x, y, area.width, status_y.saturating_sub(y));

    if ws.chrome.maximized {
        let region = match ws.focus {
            Focus::Worktree | Focus::Explorer => Region::Explorer,
            Focus::Viewer | Focus::Editor | Focus::Revidere => Region::Viewer,
            Focus::TerminalClaude => Region::TerminalClaude,
            Focus::TerminalShell => Region::TerminalShell,
        };
        regions.push((region, main));
    } else {
        let cfg = &ws.config.layout;
        let explorer_w = main.width * cfg.explorer_width_pct / 100;
        let viewer_w = main.width * cfg.viewer_width_pct / 100;
        let terminal_w = main.width.saturating_sub(explorer_w + viewer_w);
        let claude_h = main.height * cfg.terminal_split_pct / 100;
        regions.push((
            Region::Explorer,
            Rect::new(main.x, main.y, explorer_w, main.height),
        ));
        regions.push((
            Region::Viewer,
            Rect::new(main.x + explorer_w, main.y, viewer_w, main.height),
        ));
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
    Layout { regions }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 全ての区画は_hitで自分に戻る() {
        let mut ws = Workspace::for_test();
        for maximized in [false, true] {
            ws.chrome.maximized = maximized;
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
        let ws = Workspace::for_test();
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let total: u32 = l.regions.iter().map(|(_, r)| r.area()).sum();
        assert_eq!(total, 120 * 40);
    }

    #[test]
    fn 最大化はフォーカス中の列だけを残す() {
        let mut ws = Workspace::for_test();
        ws.chrome.maximized = true;
        ws.focus = Focus::TerminalShell;
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        assert!(l.rect(Region::TerminalShell).is_some());
        assert!(l.rect(Region::Explorer).is_none());
        assert!(l.rect(Region::WorktreeStrip).is_none());
    }
}
