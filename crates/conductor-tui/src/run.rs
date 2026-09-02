//! メインループ。待つ → 入力を捌く → svc の結果を消費 → 描く、の 1 周を回す。

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use conductor_svc::{EventKind, Services};

use crate::effect::{Effect, apply};
use crate::layout::{Layout, Region, layout};
use crate::liveness::{Liveness, liveness};
use crate::render::render;
use crate::route::{Routed, global_effects, route};
use crate::task::TaskResult;
use crate::workspace::{Focus, Workspace};

/// フレームを描く前にイベントを捌き続ける上限。高速スクロール中に中間フレームが
/// 出なくなるのを防ぐ (トラックパッドの慣性は 100 個超のイベントを出しうる)。
const MAX_DRAIN: Duration = Duration::from_millis(8);
/// 最後の入力のあと Active の tick を使い続ける時間。
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
/// フラッシュメッセージが消えるまで。
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

pub fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
) -> Result<()> {
    let mut last_input = Instant::now();
    let mut last_layout = Layout {
        regions: Vec::new(),
    };
    let mut dirty = true;

    loop {
        // 描くのが先。起動直後に既にキーが溜まっていると、捌いてから描く並びでは
        // 1 フレームも出さないまま終了しうる。
        if dirty {
            terminal.draw(|frame| {
                last_layout = layout(ws, frame.area());
                render(frame, ws, &last_layout);
            })?;
            dirty = false;
        }

        if drain_input(
            ws,
            svc,
            &last_layout,
            tick_rate(liveness(ws, last_input.elapsed() < ACTIVITY_TIMEOUT)),
        )? {
            last_input = Instant::now();
            dirty = true;
        }
        while let Some(event) = svc.try_recv() {
            match event.kind {
                EventKind::Task(result) => match result {},
                EventKind::Watch(watch) => log::debug!("watch event (未配線): {watch:?}"),
            }
            dirty = true;
        }
        dirty |= expire_status(ws);

        if ws.should_quit {
            return Ok(());
        }
    }
}

fn tick_rate(liveness: Liveness) -> Duration {
    match liveness {
        Liveness::Terminal => Duration::from_millis(8),
        Liveness::Active => Duration::from_millis(16),
        Liveness::Idle => Duration::from_millis(500),
    }
}

/// 溜まっているイベントを [MAX_DRAIN] の予算まで捌く。何か捌いたら true。
fn drain_input(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    layout: &Layout,
    tick: Duration,
) -> Result<bool> {
    if !crossterm::event::poll(tick)? {
        return Ok(false);
    }
    let deadline = Instant::now() + MAX_DRAIN;
    loop {
        match crossterm::event::read()? {
            // オートリピートは押下と同じに扱う。Repeat が届くのは kitty プロトコル下
            // だけで、無い端末は Press の連続として送ってくる。
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                on_key(ws, svc, key);
            }
            Event::Mouse(mouse) => on_mouse(ws, svc, layout, mouse),
            _ => {}
        }
        if ws.should_quit || Instant::now() >= deadline || !crossterm::event::poll(Duration::ZERO)?
        {
            return Ok(true);
        }
    }
}

/// キー 1 つ分の route → update → apply。ループとテストの両方がここを通る。
pub fn on_key(ws: &mut Workspace, svc: &mut Services<TaskResult>, key: KeyEvent) {
    let effects = match route(ws, key) {
        Routed::Effects(effects) => effects,
        // パネルはまだ Action を消費しないので、全て既定の解釈に落ちる。
        Routed::Action(action) => global_effects(ws, action),
        // PTY はフェーズ 2。
        Routed::ForwardToPty(_) | Routed::Ignored => Vec::new(),
    };
    apply(ws, svc, effects);
}

/// クリックした区画へフォーカスを移す。パネル内の当たり判定はフェーズ 2 以降。
fn on_mouse(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    layout: &Layout,
    mouse: MouseEvent,
) {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return;
    }
    let Some(focus) = layout.hit(mouse.column, mouse.row).and_then(focus_for) else {
        return;
    };
    apply(ws, svc, vec![Effect::Focus(focus)]);
}

fn focus_for(region: Region) -> Option<Focus> {
    match region {
        Region::WorktreeStrip => Some(Focus::Worktree),
        Region::Explorer => Some(Focus::Explorer),
        Region::Viewer => Some(Focus::Viewer),
        Region::TerminalClaude => Some(Focus::TerminalClaude),
        Region::TerminalShell => Some(Focus::TerminalShell),
        Region::TitleBar | Region::MenuBar | Region::StatusBar => None,
    }
}

/// 消えたら true。Liveness が status を Active の理由にしているので、これが無いと
/// 1 度のメッセージで永久に 60fps になる。
fn expire_status(ws: &mut Workspace) -> bool {
    let expired = ws
        .chrome
        .status
        .as_ref()
        .is_some_and(|msg| msg.shown_at.elapsed() >= STATUS_TIMEOUT);
    if expired {
        ws.chrome.status = None;
    }
    expired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal::Modal;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(ws: &mut Workspace, keys: &[KeyEvent]) {
        let mut svc = Services::new();
        for k in keys {
            on_key(ws, &mut svc, *k);
        }
    }

    /// ターミナルへ入ると Tab も ctrl+q も PTY のものになり、パネルを跨ぐのは
    /// fires_in_terminal なチョード (alt+l) だけになる。
    #[test]
    fn フォーカス移動と終了はターミナルで振る舞いが変わる() {
        let alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        let mut ws = Workspace::for_test();

        press(&mut ws, &[key(KeyCode::Tab), key(KeyCode::Tab)]);
        assert_eq!(ws.focus, Focus::TerminalClaude);
        press(&mut ws, &[key(KeyCode::Tab), ctrl_q]);
        assert_eq!(ws.focus, Focus::TerminalClaude);
        assert!(!ws.should_quit);

        press(&mut ws, &[alt_l]);
        assert_eq!(ws.focus, Focus::TerminalShell);
        press(&mut ws, &[alt_l]);
        assert_eq!(ws.focus, Focus::Explorer);
        press(&mut ws, &[ctrl_q]);
        assert!(ws.should_quit);
    }

    #[test]
    fn ヘルプは開いて閉じるまで入力を独占する() {
        let mut ws = Workspace::for_test();
        press(&mut ws, &[key(KeyCode::Char('?'))]);
        assert!(matches!(ws.modals.as_slice(), [Modal::Help]));

        press(&mut ws, &[key(KeyCode::Tab)]);
        assert_eq!(
            ws.focus,
            Focus::Explorer,
            "モーダル越しにフォーカスが動いた"
        );

        press(&mut ws, &[key(KeyCode::Esc)]);
        assert!(ws.modals.is_empty());
        press(&mut ws, &[key(KeyCode::Tab)]);
        assert_eq!(ws.focus, Focus::Viewer);
    }

    #[test]
    fn クリックした区画へフォーカスが移る() {
        let mut ws = Workspace::for_test();
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let mut svc = Services::new();
        let viewer = l.rect(Region::Viewer).unwrap();
        on_mouse(
            &mut ws,
            &mut svc,
            &l,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: viewer.x,
                row: viewer.y,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(ws.focus, Focus::Viewer);
    }

    #[test]
    fn ステータスは期限で消える() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::Status(
                crate::workspace::StatusLevel::Info,
                "hello".into(),
            )],
        );
        assert!(!expire_status(&mut ws));
        assert_eq!(liveness(&ws, false), Liveness::Active);

        ws.chrome.status.as_mut().unwrap().shown_at = Instant::now() - STATUS_TIMEOUT;
        assert!(expire_status(&mut ws));
        assert_eq!(liveness(&ws, false), Liveness::Idle);
    }
}
