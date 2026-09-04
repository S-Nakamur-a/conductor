//! メインループ。待つ → 入力を捌く → svc の結果を消費 → 描く、の 1 周を回す。

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};

use conductor_svc::{EventKind, Services};

use crate::effect::{Effect, apply};
use crate::layout::{Layout, Region, layout};
use crate::liveness::{Liveness, liveness};
use crate::render::render;
use crate::route::{Routed, global_effects, route};
use crate::task::{Task, TaskResult};
use crate::timer::{PTY_CLEANUP, Timer, WORKTREE_POLL};
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
    let mut worktree_poll = Timer::new(WORKTREE_POLL, last_input);
    let mut pty_cleanup = Timer::new(PTY_CLEANUP, last_input);
    let update_interval = Duration::from_secs(ws.config.updates.check_interval_secs);
    let mut update_poll = ws
        .config
        .updates
        .check_on_startup
        .then(|| Timer::new(update_interval, last_input));
    let mut dirty = true;

    let mut startup = vec![
        Effect::Spawn(Task::ListWorktrees),
        Effect::Spawn(Task::LoadGrabState),
    ];
    if update_poll.is_some() {
        startup.push(Effect::Spawn(Task::CheckForUpdate {
            max_age: update_interval,
            announce: false,
        }));
    }
    apply(ws, svc, startup);

    loop {
        // 区画は描く前に決める。PTY のリサイズが描画の副産物になると、
        // 子プロセスは 1 フレーム古い幅に描いたものを送ってくる。
        let size = terminal.size()?;
        let last_layout = layout(ws, Rect::new(0, 0, size.width, size.height));
        ws.sync_layout(&last_layout);
        let prepared = ws.prepare();
        apply(ws, svc, prepared);

        // 描くのが先。起動直後に既にキーが溜まっていると、捌いてから描く並びでは
        // 1 フレームも出さないまま終了しうる。
        if dirty {
            // 未書き込みで残したセルは ratatui の diff では決して塗り直されないので、
            // トランスクリプトの行が動いたフレームだけは端末ごと消す。消してから描くまでを
            // 同期出力で囲むのは、消えた瞬間を見せないため。非対応の端末は無視するだけ。
            let _ = execute!(io::stdout(), BeginSynchronizedUpdate);
            if ws.panels.terminal.take_clear_request() {
                terminal.clear()?;
            }
            ws.entrance.start_if_pending();
            terminal.draw(|frame| render(frame, ws, &last_layout))?;
            let _ = execute!(io::stdout(), EndSynchronizedUpdate);
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
            let effects = match event.kind {
                EventKind::Task(result) => ws.accept(result),
                EventKind::Watch(watch) => watch_effects(ws, watch),
            };
            apply(ws, svc, effects);
            dirty = true;
        }

        dirty |= expire_status(ws);
        dirty |= tick_modals(ws, svc);
        dirty |= tick_index(ws, svc);
        dirty |= ws.tick_viewer();
        dirty |= tick_editor(ws, svc);
        dirty |= ws.panels.terminal.took_output();
        ws.panels.terminal.nudge();
        dirty |= run_timers(
            ws,
            svc,
            &mut worktree_poll,
            &mut pty_cleanup,
            update_poll.as_mut().map(|timer| (timer, update_interval)),
        );
        dirty |= ws.entrance.is_animating();

        if ws.should_quit {
            return Ok(());
        }
    }
}

/// エディタが終わっていたら Viewer へ戻し、編集直後のファイルを読み直す。
/// ファイルウォッチャーのデバウンスを待たせない。
fn tick_editor(ws: &mut Workspace, svc: &mut Services<TaskResult>) -> bool {
    let Some(path) = ws.panels.terminal.poll_editor_exit() else {
        return false;
    };
    // 編集で差分も動くので、本文と一緒に変更ファイル一覧も取り直す。
    let mut effects = ws.panels.explorer.refresh();
    effects.push(Effect::OpenFile {
        path,
        line: None,
        diff: None,
        preview: false,
    });
    apply(ws, svc, effects);
    true
}

/// 締切を持つモーダルを一押しする。何か動いたら true。
///
/// 進めるのは top だけ。下のモーダルは入力を受けないので締切も進まない。
fn tick_modals(ws: &mut Workspace, svc: &mut Services<TaskResult>) -> bool {
    if ws.modals.is_empty() {
        return false;
    }
    let effects = ws.tick_top_modal();
    if effects.is_empty() {
        return false;
    }
    apply(ws, svc, effects);
    true
}

fn tick_index(ws: &mut Workspace, svc: &mut Services<TaskResult>) -> bool {
    let effects = crate::index::tick(ws);
    if effects.is_empty() {
        return false;
    }
    apply(ws, svc, effects);
    true
}

/// watcher からの合図の行き先。MCP のレビュー更新だけがパネルの外に効く。
fn watch_effects(ws: &mut Workspace, watch: conductor_svc::watch::WatchEvent) -> Vec<Effect> {
    use conductor_svc::watch::WatchEvent;
    match watch {
        WatchEvent::RefreshRequested => vec![Effect::Spawn(Task::LoadReview)],
        // 索引の作り直しは自前の静穏時間で数えるので、1 件ずつそのまま渡す。
        WatchEvent::FsChanged(path) => {
            crate::index::note_change(ws, &path);
            Vec::new()
        }
        watch => ws.panels.terminal.on_watch(&watch),
    }
}

fn run_timers(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    worktree_poll: &mut Timer,
    pty_cleanup: &mut Timer,
    update_poll: Option<(&mut Timer, Duration)>,
) -> bool {
    let now = Instant::now();
    let mut dirty = false;
    if worktree_poll.due(now) {
        apply(ws, svc, vec![Effect::Spawn(Task::ListWorktrees)]);
    }
    if pty_cleanup.due(now) {
        dirty |= ws.panels.terminal.cleanup_dead();
    }
    if let Some((update_poll, interval)) = update_poll
        && update_poll.due(now)
    {
        apply(
            ws,
            svc,
            vec![Effect::Spawn(Task::CheckForUpdate {
                max_age: interval,
                announce: false,
            })],
        );
    }
    dirty
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
    ws.entrance.skip();
    loop {
        match crossterm::event::read()? {
            // オートリピートは押下と同じに扱う。Repeat が届くのは kitty プロトコル下
            // だけで、無い端末は Press の連続として送ってくる。
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                on_key(ws, svc, key);
            }
            Event::Mouse(mouse) => on_mouse(ws, svc, layout, mouse),
            Event::Paste(data) => on_paste(ws, data),
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
        Routed::Action(action) => ws
            .dispatch(action)
            .unwrap_or_else(|| global_effects(ws, action)),
        Routed::ForwardToPty(key) => {
            ws.panels.terminal.forward_key(key, ws.focus);
            Vec::new()
        }
        Routed::Ignored => Vec::new(),
    };
    apply(ws, svc, effects);
}

/// 貼り付け 1 つ分。macOS の端末は IME で確定したマルチバイト文字をキーではなく
/// bracketed paste で届けるので、これを捨てると日本語が 1 文字ずつしか入らない。
/// キーと同じ順で最前面のモーダルに優先権を渡す (裏の PTY へ流れると入力欄から消える)。
pub fn on_paste(ws: &mut Workspace, data: String) {
    if let Some(modal) = ws.modals.last_mut() {
        modal.paste(&data);
        return;
    }
    if matches!(
        ws.focus,
        Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
    ) {
        ws.panels.terminal.paste(&data, ws.focus);
    }
}

/// クリックした区画へフォーカスを移し、その区画に行を渡す。ホイールは
/// フォーカスを動かさない — 覗くだけでキーの行き先が変わると事故になる。
fn on_mouse(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    layout: &Layout,
    mouse: MouseEvent,
) {
    if let Some(effects) = drag_divider(ws, layout, mouse) {
        apply(ws, svc, effects);
        return;
    }
    let Some(region) = layout.hit(mouse.column, mouse.row) else {
        return;
    };
    // メニューが最前面なので、区画より先に見る。開いている間の空振りも飲み込む。
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && let Some(effects) = crate::menu::click(ws, layout, mouse.column, mouse.row)
    {
        apply(ws, svc, effects);
        return;
    }
    match mouse.kind {
        MouseEventKind::ScrollDown => scroll_region(ws, svc, layout, region, mouse, 3),
        MouseEventKind::ScrollUp => scroll_region(ws, svc, layout, region, mouse, -3),
        // 端末はポインタが窓の外に出たことを報せないので、区画の外に出た時点で降ろす。
        MouseEventKind::Moved => {
            let Workspace { panels, review, .. } = ws;
            let over = (region == Region::Viewer)
                .then(|| {
                    panels
                        .viewer
                        .word_at_screen(mouse.column, mouse.row, review)
                })
                .flatten();
            panels.viewer.note_pointer(over);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // 帯はフォーカスを持たない。ここで Focus::Worktree にすると、押した先の
            // 代わりに中央の一覧が開く。
            if region == Region::WorktreeStrip {
                let Some(rect) = layout.rect(region) else {
                    return;
                };
                let slots = crate::panels::worktree::strip::slots(ws, rect.width);
                let effects = ws
                    .panels
                    .worktree
                    .strip_click(&slots, mouse.column - rect.x);
                apply(ws, svc, effects);
                return;
            }
            let Some(focus) = focus_for(region) else {
                return;
            };
            apply(ws, svc, vec![Effect::Focus(focus)]);
            if focus == Focus::Viewer
                && let Some(effects) = viewer_popup_click(ws, mouse)
            {
                apply(ws, svc, effects);
                return;
            }
            // タブ行は PTY のグリッドの外なので、中身を見る経路より先に捌く。
            if matches!(focus, Focus::TerminalClaude | Focus::TerminalShell)
                && let Some(tabs) = layout
                    .rect(region)
                    .map(crate::panels::terminal::render::tab_area)
                && tabs.contains(Position::new(mouse.column, mouse.row))
                && let Some(effects) =
                    ws.panels
                        .terminal
                        .tab_click(focus, mouse.column - tabs.x, tabs.width)
            {
                apply(ws, svc, effects);
                return;
            }
            // 「最新へ」チップは PTY の中身より上にある。
            if focus == Focus::TerminalClaude
                && let Some(rect) = layout.rect(Region::TerminalClaude)
                && ws
                    .panels
                    .terminal
                    .transcript_click(rect, mouse.column, mouse.row)
            {
                return;
            }
            let effects = match focus {
                Focus::Revidere => {
                    ws.panels.revidere.click(region, mouse.row);
                    Vec::new()
                }
                Focus::Explorer => ws.panels.explorer.click(mouse.row, &ws.review),
                Focus::Viewer => {
                    let root = ws.panels.viewer.root().to_path_buf();
                    let (panels, _, ctx) = ws.split(&root);
                    panels.viewer.click(
                        mouse.column,
                        mouse.row,
                        mouse.modifiers.contains(KeyModifiers::SHIFT),
                        &ctx,
                    )
                }
                Focus::TerminalClaude | Focus::TerminalShell => ws.panels.terminal.click(focus),
                _ => Vec::new(),
            };
            apply(ws, svc, effects);
        }
        _ => {}
    }
}

/// つかんだ境界は区画に属さないので、区画の割り出しより先に捌く。
fn drag_divider(ws: &mut Workspace, layout: &Layout, mouse: MouseEvent) -> Option<Vec<Effect>> {
    if let Some(divider) = ws.chrome.drag {
        return Some(match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                crate::command::drag_divider(ws, divider, layout.main, mouse.column, mouse.row);
                Vec::new()
            }
            MouseEventKind::Up(MouseButton::Left) => {
                ws.chrome.drag = None;
                vec![crate::command::persist_layout(ws)]
            }
            _ => Vec::new(),
        });
    }
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && ws.chrome.menu.open_index().is_none()
    {
        ws.chrome.drag = layout.divider_at(mouse.column, mouse.row);
        return ws.chrome.drag.map(|_| Vec::new());
    }
    None
}

/// ポップアップと Cmd+クリックはガターの当たり判定より先。どちらも本文の上に
/// 重なっているので、後ろに回すと下の行が選ばれる。
fn viewer_popup_click(ws: &mut Workspace, mouse: MouseEvent) -> Option<Vec<Effect>> {
    let root = ws.panels.viewer.root().to_path_buf();
    let (panels, _, ctx) = ws.split(&root);
    if let Some(effects) = panels.viewer.click_hover(mouse.column, mouse.row, &ctx) {
        return Some(effects);
    }
    let jump = mouse
        .modifiers
        .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL);
    jump.then(|| panels.viewer.jump_at_screen(mouse.column, mouse.row, &ctx))
        .flatten()
}

fn scroll_region(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    layout: &Layout,
    region: Region,
    mouse: MouseEvent,
    delta: isize,
) {
    match region {
        Region::ExplorerTree | Region::ExplorerChanges => {
            let Workspace { panels, review, .. } = ws;
            panels.explorer.scroll(region, delta, review)
        }
        Region::Viewer => ws.panels.viewer.scroll_lines(delta),
        // 端末はホイールの行き先が中身次第 (子プロセス / トランスクリプト /
        // スクロールバック) なので、判断はパネルが持つ。
        Region::TerminalClaude | Region::TerminalShell | Region::Editor => {
            let Some(rect) = layout.rect(region) else {
                return;
            };
            let effects = ws
                .panels
                .terminal
                .wheel(region, rect, mouse.column, mouse.row, delta);
            apply(ws, svc, effects);
        }
        Region::RevidereOrder | Region::RevidereDiff => ws.panels.revidere.scroll(region, delta),
        _ => {}
    }
}

fn focus_for(region: Region) -> Option<Focus> {
    match region {
        Region::WorktreeStrip => Some(Focus::Worktree),
        Region::ExplorerTree | Region::ExplorerChanges => Some(Focus::Explorer),
        Region::Viewer => Some(Focus::Viewer),
        Region::Editor => Some(Focus::Editor),
        Region::TerminalClaude => Some(Focus::TerminalClaude),
        Region::TerminalShell => Some(Focus::TerminalShell),
        Region::RevidereOrder | Region::RevidereDiff => Some(Focus::Revidere),
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
    use conductor_svc::pty::SessionKind;
    use crossterm::event::KeyCode;
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

    #[test]
    fn 貼り付けはモーダルが最前面ならそちらへ行く() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::TerminalShell;
        ws.modals.push(Modal::Prompt(crate::modal::Prompt {
            title: "t".into(),
            input: Default::default(),
            on_submit: |_| Vec::new(),
        }));
        on_paste(&mut ws, "日本語".into());
        let Some(Modal::Prompt(prompt)) = ws.modals.last() else {
            panic!("{:?}", ws.modals);
        };
        assert_eq!(prompt.input.text(), "日本語");
    }

    /// タブ行のクリックが区画の座標からタブの列へ落ちるところまでを通す。
    #[test]
    fn タブ行のチップをクリックするとセッションが増える() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let dir = tempfile::tempdir().unwrap();
        ws.config.general.shell = "/bin/sh".into();
        ws.panels
            .terminal
            .follow_worktree(Some(dir.path().to_path_buf()));
        ws.repo.root = dir.path().to_path_buf();

        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let tabs =
            crate::panels::terminal::render::tab_area(l.rect(Region::TerminalShell).unwrap());
        let add = ws
            .panels
            .terminal
            .tab_slots(Region::TerminalShell, tabs.width)
            .into_iter()
            .find(|slot| slot.kind == crate::panels::terminal::tabs::SlotKind::Add)
            .expect("チップが無い");

        on_mouse(
            &mut ws,
            &mut svc,
            &l,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: tabs.x + add.start,
                row: tabs.y,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(ws.panels.terminal.sessions(SessionKind::Shell).len(), 1);
        assert_eq!(ws.focus, Focus::TerminalShell);
    }

    #[test]
    fn 端末に映すセッションが無ければ貼り付けは捨てる() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::TerminalShell;
        on_paste(&mut ws, "日本語".into());
        assert!(ws.modals.is_empty());
        assert!(ws.chrome.status.is_none());
    }

    /// Viewer の e から埋め込みエディタを起こし、プロセスが終わったら Viewer に戻って
    /// 読み直すまでを 1 本で通す。どのエディタかは環境が決めるので、ここは即座に
    /// 終わるコマンドを直接渡す。
    #[test]
    fn eでエディタを起こし終了したらviewerへ戻って読み直す() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "ALPHA\n").unwrap();

        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        crate::testing::select_only_worktree(&mut ws, &mut svc, dir.path());
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::OpenFile {
                path: dir.path().join("a.txt"),
                line: None,
                diff: None,
                preview: false,
            }],
        );
        crate::testing::pump(&mut ws, &mut svc);
        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));

        // Viewer の e は「このファイルをエディタで」だけを言う。
        let effects = ws
            .dispatch(conductor_core::keymap::Action::OpenInEditor)
            .unwrap();
        assert_eq!(
            effects,
            vec![Effect::OpenInEditor(dir.path().join("a.txt"))],
            "{effects:?}"
        );

        let argv: Vec<String> = ["/bin/sh", "-c", "exit 0"].map(String::from).to_vec();
        ws.panels
            .terminal
            .open_editor(&dir.path().join("a.txt"), dir.path(), &argv)
            .unwrap();
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::Editor)]);

        // エディタは Explorer と Viewer の列を併合した 1 区画を占める。
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        assert!(l.rect(Region::Editor).is_some());
        assert!(l.rect(Region::Viewer).is_none() && l.rect(Region::ExplorerTree).is_none());

        let deadline = Instant::now() + Duration::from_secs(10);
        while !tick_editor(&mut ws, &mut svc) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(ws.focus, Focus::Viewer, "終了したら Viewer へ戻る");
        crate::testing::pump(&mut ws, &mut svc);
        assert_eq!(
            ws.panels.viewer.content.lines,
            ["ALPHA"],
            "読み直していない"
        );
        assert!(
            layout(&ws, Rect::new(0, 0, 120, 40))
                .rect(Region::Editor)
                .is_none(),
            "区画が残っている"
        );
    }

    /// Explorer で Enter を押してから Viewer に中身が出るまでを、実キーと svc の
    /// 往復を通して 1 本で確かめる。パネル単体では「根が食い違う」経路が出ない。
    #[test]
    fn enterで開いた1枚のタブがworktree切替まで生き残る() {
        let a = tempfile::TempDir::new().unwrap();
        std::fs::write(a.path().join("a.txt"), "ALPHA\n").unwrap();
        std::fs::write(a.path().join("b.txt"), "BRAVO\n").unwrap();
        let b = tempfile::TempDir::new().unwrap();
        std::fs::write(b.path().join("b.txt"), "OTHER\n").unwrap();

        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        crate::testing::select_only_worktree(&mut ws, &mut svc, a.path());
        assert_eq!(ws.panels.explorer.root(), a.path());
        assert_eq!(ws.panels.viewer.root(), a.path());

        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        ws.sync_layout(&l);
        ws.focus = Focus::Explorer;
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);

        assert_eq!(ws.focus, Focus::Viewer);
        assert_eq!(ws.panels.viewer.tabs().len(), 1);
        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));
        assert_eq!(ws.panels.viewer.content.lines, ["ALPHA"]);

        // 同じファイルをもう一度開いてもタブは増えない。
        ws.focus = Focus::Explorer;
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);
        assert_eq!(ws.panels.viewer.tabs().len(), 1);

        // worktree を切り替えると 2 つの根が揃って動き、新しい根に無いタブは落ちる。
        let info = ws.panels.worktree.list()[0].clone();
        let moved = conductor_core::git_engine::WorktreeInfo {
            path: b.path().to_path_buf(),
            branch: "feature".into(),
            is_main: false,
            ..info
        };
        let effects = ws.accept(TaskResult::Worktrees(Ok(vec![moved])));
        apply(&mut ws, &mut svc, effects);
        crate::testing::pump(&mut ws, &mut svc);

        assert_eq!(ws.panels.explorer.root(), b.path());
        assert_eq!(ws.panels.viewer.root(), b.path());
        assert!(ws.panels.viewer.tabs().is_empty(), "a.txt は新しい根に無い");
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
        assert!(matches!(ws.modals.as_slice(), [Modal::Help(_)]));

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
                column: viewer.x + 2,
                row: viewer.y + 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(ws.focus, Focus::Viewer);
    }

    #[test]
    fn 境界をドラッグすると比率が追従し離したときに書き出す() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let l = layout(&ws, Rect::new(0, 0, 100, 40));
        let viewer = l.rect(Region::Viewer).unwrap();
        let before = ws.config.layout.explorer_width_pct;
        let at = |kind, column| MouseEvent {
            kind,
            column,
            row: viewer.y + 3,
            modifiers: KeyModifiers::NONE,
        };

        on_mouse(
            &mut ws,
            &mut svc,
            &l,
            at(MouseEventKind::Down(MouseButton::Left), viewer.x),
        );
        assert_eq!(ws.chrome.drag, Some(crate::layout::Divider::ExplorerViewer));
        assert_eq!(
            ws.focus,
            Focus::Explorer,
            "枠線を掴んでもフォーカスは動かない"
        );

        on_mouse(
            &mut ws,
            &mut svc,
            &l,
            at(MouseEventKind::Drag(MouseButton::Left), viewer.x + 10),
        );
        assert_eq!(ws.config.layout.explorer_width_pct, before + 10);

        let released = drag_divider(
            &mut ws,
            &l,
            at(MouseEventKind::Up(MouseButton::Left), viewer.x + 10),
        );
        assert_eq!(ws.chrome.drag, None);
        assert!(
            matches!(
                released.as_deref(),
                Some([Effect::Spawn(Task::PersistConfig(_))])
            ),
            "離したときに 1 回書き出す: {released:?}"
        );
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

    /// メニューを開いて、その CommandId の行に降りて Enter を押す。実際の経路
    /// (メニュー → コマンド → Effect) をそのまま通す。
    fn menu_pick(
        ws: &mut Workspace,
        svc: &mut Services<TaskResult>,
        id: crate::command::CommandId,
    ) {
        let menu = crate::menu::MENUS
            .iter()
            .position(|m| m.items.iter().any(|item| item.command() == Some(id)))
            .unwrap_or_else(|| panic!("{id:?} はどのメニューにも無い"));
        on_key(ws, svc, key(KeyCode::F(10)));
        for _ in 0..crate::menu::MENUS.len() {
            if ws.chrome.menu.index() == Some(menu) {
                break;
            }
            on_key(ws, svc, key(KeyCode::Right));
        }
        assert_eq!(ws.chrome.menu.index(), Some(menu), "メニューが開かない");
        on_key(ws, svc, key(KeyCode::Down));
        let row = |ws: &Workspace| match ws.chrome.menu {
            crate::menu::MenuBar::Open { selected, .. } => selected,
            other => panic!("ドロップダウンが開いていない: {other:?}"),
        };
        for _ in 0..crate::menu::MENUS[menu].items.len() {
            if crate::menu::MENUS[menu].items[row(ws)].command() == Some(id) {
                break;
            }
            on_key(ws, svc, key(KeyCode::Down));
        }
        assert_eq!(crate::menu::MENUS[menu].items[row(ws)].command(), Some(id));
        on_key(ws, svc, key(KeyCode::Enter));
    }

    /// 更新の導線を端から端まで: 新しいリリースが届く → バッジが出る → メニューの
    /// 行が有効になる → 実行で確認のモーダルが積まれる。
    #[test]
    fn 新しいリリースが届くとバッジと更新コマンドが生きる() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let id = crate::command::CommandId::UpdateAndRestart;
        let title = |ws: &Workspace| {
            crate::render::title_line(ws, 120)
                .to_string()
                .trim_end()
                .to_string()
        };

        assert!(!title(&ws).contains("available"), "{}", title(&ws));
        assert!(!crate::command::enabled(&ws, id).is_yes());

        let effects = ws.accept(TaskResult::UpdateCheck {
            outcome: crate::task::UpdateCheck::Newer(Box::new(crate::modal::update::tests::info())),
            announce: true,
        });
        apply(&mut ws, &mut svc, effects);
        assert!(title(&ws).ends_with("v9.9.9 available"), "{}", title(&ws));
        assert!(
            crate::render::status_line(&ws)
                .to_string()
                .contains("9.9.9")
        );
        assert!(crate::command::enabled(&ws, id).is_yes());

        menu_pick(&mut ws, &mut svc, id);
        assert!(
            matches!(ws.modals.last(), Some(Modal::Update(_))),
            "{:?}",
            ws.modals.last()
        );
    }

    /// 届かなかったことと最新だったことは別。混ぜると、1 度の通信失敗で出ていた
    /// バッジが消え、オフラインで「最新です」と嘘をつく。
    #[test]
    fn 更新チェックは届かなかったことを最新と混ぜない() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let check = |outcome| TaskResult::UpdateCheck {
            outcome,
            announce: true,
        };

        let effects = ws.accept(check(crate::task::UpdateCheck::Newer(Box::new(
            crate::modal::update::tests::info(),
        ))));
        apply(&mut ws, &mut svc, effects);

        let effects = ws.accept(check(crate::task::UpdateCheck::Unreachable));
        apply(&mut ws, &mut svc, effects);
        assert!(ws.chrome.update.is_some(), "届かないだけでバッジが消えた");
        let status = crate::render::status_line(&ws).to_string();
        assert!(status.contains("could not reach GitHub"), "{status}");

        let effects = ws.accept(check(crate::task::UpdateCheck::UpToDate));
        apply(&mut ws, &mut svc, effects);
        assert!(ws.chrome.update.is_none());
        assert!(
            crate::render::status_line(&ws)
                .to_string()
                .contains("up to date")
        );
    }

    /// モーダルを閉じたあとに届いた失敗も伝える。隠す導線が用意されている以上、
    /// 黙って消えると差し替えが済んだのか落ちたのか分からなくなる。
    #[test]
    fn 更新の失敗はモーダルを閉じていても伝わる() {
        let mut ws = Workspace::for_test();
        let effects = ws.accept(TaskResult::UpdateProgress(
            crate::task::UpdateStage::Failed("no route to host".into()),
        ));
        let [Effect::Status(crate::workspace::StatusLevel::Error, text)] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(text, "no route to host");
        assert!(!ws.relaunch);
    }

    /// 演出は「動いている理由」に数えられる。切ってあれば起動直後から Idle。
    #[test]
    fn 起動演出の有無がフレームを流す理由になる() {
        let mut ws = Workspace::for_test();
        assert_eq!(liveness(&ws, false), Liveness::Idle, "切ってあるのに動く");

        ws.entrance = crate::entrance::Entrance::new(true);
        assert_eq!(liveness(&ws, false), Liveness::Active);
        ws.entrance.skip();
        assert_eq!(liveness(&ws, false), Liveness::Idle);
    }

    /// 取り消せない git 操作は必ず確認を挟む。
    #[test]
    fn メニューからのmergeは確認を通ってからタスクになる() {
        let repo = crate::testing::TestRepo::new();
        let feature = repo.worktree("feature/x");
        repo.commit_in(&feature, "b.txt", "bravo\n", "add bravo");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        let index = ws
            .panels
            .worktree
            .list()
            .iter()
            .position(|w| w.path == feature)
            .expect("linked worktree が一覧に無い");
        apply(&mut ws, &mut svc, vec![Effect::SelectWorktree(index)]);
        crate::testing::pump(&mut ws, &mut svc);

        menu_pick(&mut ws, &mut svc, crate::command::CommandId::MergeToMain);
        let Some(Modal::Confirm(confirm)) = ws.modals.last() else {
            panic!("確認が出ていない: {:?}", ws.modals);
        };
        assert!(
            confirm.question.contains("feature/x"),
            "{}",
            confirm.question
        );

        on_key(&mut ws, &mut svc, key(KeyCode::Char('n')));
        assert!(ws.modals.is_empty());
        assert!(svc.try_recv().is_none(), "n でタスクが飛んだ");

        menu_pick(&mut ws, &mut svc, crate::command::CommandId::MergeToMain);
        on_key(&mut ws, &mut svc, key(KeyCode::Char('y')));
        crate::testing::pump(&mut ws, &mut svc);
        let status = ws.chrome.status.as_ref().expect("結果が出ていない");
        assert_eq!(status.level, crate::workspace::StatusLevel::Success);
        assert!(
            repo.git(&["log", "--oneline", "main"])
                .contains("add bravo"),
            "main に入っていない"
        );
    }

    #[test]
    fn git操作の結果は文言と一覧の取り直しになる() {
        let mut ws = Workspace::for_test();
        let effects = ws.accept(TaskResult::GitDone(Ok("Merged.".into())));
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::Status(crate::workspace::StatusLevel::Success, text),
                    Effect::Spawn(Task::ListWorktrees)
                ] if text == "Merged."
            ),
            "{effects:?}"
        );

        let effects = ws.accept(TaskResult::GitDone(Err("no upstream".into())));
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Status(crate::workspace::StatusLevel::Error, _)]
            ),
            "失敗で一覧を取り直している: {effects:?}"
        );
    }

    #[test]
    fn pr取り込みに失敗しても入力は残る() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        run_command(
            &mut ws,
            &mut svc,
            crate::command::CommandId::ReviewPullRequest,
        );
        type_text(&mut ws, &mut svc, "12345");
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));

        let effects = ws.accept(TaskResult::PrIntake(Err(
            "Pull request #12345 not found.".into()
        )));
        apply(&mut ws, &mut svc, effects);

        let Some(Modal::PrInput(prompt)) = ws.modals.last() else {
            panic!("入力が閉じた: {:?}", ws.modals);
        };
        assert_eq!(prompt.input.text(), "12345");
    }

    #[test]
    fn pr取り込みが成功すると閉じてその_worktreeへ移る() {
        let repo = crate::testing::TestRepo::new();
        let fetched = repo.worktree("pr-9");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        ws.modals.push(Modal::PrInput(Default::default()));

        let effects = ws.accept(TaskResult::PrIntake(Ok((9, fetched.clone()))));
        apply(&mut ws, &mut svc, effects);
        crate::testing::pump(&mut ws, &mut svc);

        assert!(ws.modals.is_empty(), "入力が開いたまま");
        assert_eq!(
            ws.panels.worktree.selected().map(|w| w.path.clone()),
            Some(fetched)
        );
        assert_eq!(ws.focus, Focus::Explorer);
    }

    #[test]
    fn リモートブランチを選ぶと_worktreeができる() {
        let repo = crate::testing::TestRepo::new();
        repo.remote_branch("feature/remote-only");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);

        run_command(&mut ws, &mut svc, crate::command::CommandId::SwitchBranch);
        crate::testing::pump(&mut ws, &mut svc);
        assert!(matches!(ws.modals.last(), Some(Modal::BranchPicker(_))));

        type_text(&mut ws, &mut svc, "remote-only");
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);

        assert!(
            ws.panels
                .worktree
                .list()
                .iter()
                .any(|w| w.branch == "feature/remote-only"),
            "{:?}",
            ws.panels.worktree.list()
        );
    }

    #[test]
    fn prune_は消えたworktreeを数えて確認してから消す() {
        let repo = crate::testing::TestRepo::new();
        let gone = repo.worktree("feature/gone");
        std::fs::remove_dir_all(&gone).unwrap();
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);

        run_command(&mut ws, &mut svc, crate::command::CommandId::PruneWorktrees);
        crate::testing::pump(&mut ws, &mut svc);
        let Some(Modal::Confirm(confirm)) = ws.modals.last() else {
            panic!("確認が出ていない: {:?}", ws.modals);
        };
        assert!(
            confirm.question.contains("feature-gone"),
            "{}",
            confirm.question
        );

        on_key(&mut ws, &mut svc, key(KeyCode::Char('y')));
        crate::testing::pump(&mut ws, &mut svc);
        assert!(
            repo.git(&["worktree", "list"]).lines().count() == 1,
            "{}",
            repo.git(&["worktree", "list"])
        );
    }

    #[test]
    fn cherry_pickは選んだコミットを今のworktreeへ積む() {
        let repo = crate::testing::TestRepo::new();
        let source = repo.worktree("feature/source");
        repo.commit_in(
            &source,
            "b.txt",
            "bravo
",
            "add bravo",
        );
        let here = repo.worktree("feature/here");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        let index = ws
            .panels
            .worktree
            .list()
            .iter()
            .position(|w| w.path == here)
            .unwrap();
        apply(&mut ws, &mut svc, vec![Effect::SelectWorktree(index)]);
        crate::testing::pump(&mut ws, &mut svc);

        run_command(&mut ws, &mut svc, crate::command::CommandId::CherryPick);
        crate::testing::pump(&mut ws, &mut svc);

        // 取り出し元は tab で回る。bravo を持つブランチに当たるまで送る。
        let source_title = |ws: &Workspace| match ws.modals.last() {
            Some(Modal::CherryPick(picker)) => crate::modal::commits::title(picker),
            other => panic!("cherry-pick が開いていない: {other:?}"),
        };
        for _ in 0..ws.panels.worktree.list().len() {
            if source_title(&ws).contains("feature/source") {
                break;
            }
            on_key(&mut ws, &mut svc, key(KeyCode::Tab));
            crate::testing::pump(&mut ws, &mut svc);
        }
        assert!(
            source_title(&ws).contains("feature/source"),
            "取り出し元に届かない"
        );

        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);
        assert!(here.join("b.txt").exists(), "cherry-pick が届いていない");
        assert_eq!(
            ws.chrome.status.as_ref().map(|s| s.level),
            Some(crate::workspace::StatusLevel::Success)
        );
    }

    fn run_command(
        ws: &mut Workspace,
        svc: &mut Services<TaskResult>,
        id: crate::command::CommandId,
    ) {
        let effects = crate::command::execute(ws, id);
        apply(ws, svc, effects);
    }

    fn type_text(ws: &mut Workspace, svc: &mut Services<TaskResult>, text: &str) {
        for c in text.chars() {
            on_key(ws, svc, key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn 書いたコメントはスレッドと一覧に出て起動し直しても残る() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbravo\ncharlie\n").unwrap();

        let mut ws = Workspace::for_test();
        ws.repo.root = dir.path().to_path_buf();
        let mut svc = Services::new();
        crate::testing::select_only_worktree(&mut ws, &mut svc, dir.path());
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        ws.sync_layout(&l);

        let effects = ws
            .panels
            .viewer
            .open(std::path::Path::new("a.txt"), Some(2), None, false);
        apply(&mut ws, &mut svc, effects);
        crate::testing::pump(&mut ws, &mut svc);
        ws.focus = Focus::Viewer;

        on_key(&mut ws, &mut svc, key(KeyCode::Char('c')));
        assert!(
            matches!(
                ws.modals.as_slice(),
                [crate::modal::Modal::CommentEditor(_)]
            ),
            "{:?}",
            ws.modals
        );
        type_text(&mut ws, &mut svc, "off by one");
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);

        assert!(ws.modals.is_empty(), "確定でモーダルは閉じる");
        assert_eq!(ws.review.comments().len(), 1);
        assert_eq!(ws.review.comments()[0].line_start, 2);

        let rendered = crate::panels::viewer::render::body(
            &ws.panels.viewer,
            &ws.review,
            &ws.theme,
            ws.config.ui.icon_set(),
            80,
            20,
        );
        let text: Vec<String> = rendered
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect();
        assert!(
            text.iter().any(|l| l.contains("off by one")),
            "インラインスレッドが出ない: {text:?}"
        );

        ws.focus = Focus::Explorer;
        on_key(&mut ws, &mut svc, key(KeyCode::Char('c')));
        let listed: Vec<String> = crate::panels::explorer::render::bottom_lines(&ws, 10)
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect();
        assert!(
            listed.iter().any(|l| l.contains("a.txt:L2")),
            "Comments ペインに出ない: {listed:?}"
        );

        // 起動し直しても DB に残っている。
        let mut fresh = Workspace::for_test();
        fresh.repo.root = dir.path().to_path_buf();
        let mut svc = Services::new();
        apply(&mut fresh, &mut svc, vec![Effect::Spawn(Task::LoadReview)]);
        crate::testing::pump(&mut fresh, &mut svc);
        assert_eq!(fresh.review.comments().len(), 1);
        assert_eq!(fresh.review.comments()[0].body, "off by one");
    }

    /// F10 からメニューを歩いて Enter するまでを、実キーで 1 本に通す。
    /// メニュー・パレット・キーは同じ実行口へ落ちるので、ここが通れば表と実装は繋がっている。
    #[test]
    fn f10から矢印とenterでコマンドが実行口に届く() {
        let mut ws = Workspace::for_test();
        press(&mut ws, &[key(KeyCode::F(10))]);
        assert_eq!(ws.chrome.menu, crate::menu::MenuBar::Bar { index: 0 });

        press(&mut ws, &[key(KeyCode::Right), key(KeyCode::Down)]);
        assert_eq!(
            ws.chrome.menu,
            crate::menu::MenuBar::Open {
                index: 1,
                selected: 0
            },
            "Worktree メニューの先頭"
        );

        press(&mut ws, &[key(KeyCode::Enter)]);
        assert_eq!(
            ws.chrome.menu,
            crate::menu::MenuBar::Closed,
            "実行の前に閉じる"
        );
        let [Modal::Prompt(prompt)] = ws.modals.as_slice() else {
            panic!("{:?}", ws.modals);
        };
        assert_eq!(prompt.title, "New worktree branch");
    }

    /// パレットで打ってから Enter するまで。メニューと同じ Effect::Command に落ちる。
    #[test]
    fn パレットのあいまい入力からenterで同じ実行口に届く() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        on_key(
            &mut ws,
            &mut svc,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert!(matches!(ws.modals.as_slice(), [Modal::Palette(_)]));

        type_text(&mut ws, &mut svc, "quit");
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        assert!(ws.modals.is_empty());
        assert!(ws.should_quit);
    }

    /// ライブプレビューは確定しない。Esc で開いた時点のテーマに戻る。
    #[test]
    fn テーマピッカーは移動でプレビューしescで戻す() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let before = ws.appearance.name.clone();
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::Command(crate::command::CommandId::SwitchTheme)],
        );
        assert!(matches!(ws.modals.as_slice(), [Modal::ThemePicker(_)]));

        on_key(&mut ws, &mut svc, key(KeyCode::Down));
        assert_ne!(ws.appearance.name, before, "移動でプレビューが乗る");
        assert_eq!(ws.theme.name, ws.appearance.name);

        on_key(&mut ws, &mut svc, key(KeyCode::Esc));
        assert!(ws.modals.is_empty());
        assert_eq!(ws.appearance.name, before);
        assert_eq!(ws.theme.name, before);
    }

    /// 検索を打ってから結果の行でファイルが開くまで。svc の往復を含む。
    #[test]
    fn grep検索は結果からファイルを開く() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("a.txt"),
            "alpha
needle here
",
        )
        .unwrap();

        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        crate::testing::select_only_worktree(&mut ws, &mut svc, dir.path());
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::Command(crate::command::CommandId::SearchFullText)],
        );
        type_text(&mut ws, &mut svc, "needle");

        assert!(!tick_modals(&mut ws, &mut svc), "締切の前は何も投げない");
        std::thread::sleep(Duration::from_millis(220));
        assert!(tick_modals(&mut ws, &mut svc));
        crate::testing::pump(&mut ws, &mut svc);

        // 木は ファイル → 一致 の 2 行。下に降りて Enter で開く。
        on_key(&mut ws, &mut svc, key(KeyCode::Down));
        on_key(&mut ws, &mut svc, key(KeyCode::Down));
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);

        assert!(ws.modals.is_empty());
        assert_eq!(ws.panels.viewer.active_path(), Some("a.txt"));
        assert_eq!(ws.focus, Focus::Viewer);
    }

    #[test]
    fn watchの行き先は種類で分かれる() {
        use conductor_svc::watch::WatchEvent;
        let mut ws = Workspace::for_test();
        assert!(matches!(
            watch_effects(&mut ws, WatchEvent::RefreshRequested).as_slice(),
            [Effect::Spawn(Task::LoadReview)]
        ));
        assert!(watch_effects(&mut ws, WatchEvent::ConfigChanged).is_empty());
    }

    /// パネルが消費しない Action が global の解釈へ落ちる配線。ターミナルの中では
    /// new_claude_code は fires_in_terminal ではないので PTY のものになる。
    #[test]
    fn ctrl_nはパネルからは新規セッションターミナルからはptyへ() {
        let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        for focus in [Focus::Explorer, Focus::Worktree, Focus::Viewer] {
            let mut ws = Workspace::for_test();
            ws.focus = focus;
            let Routed::Action(action) = route(&mut ws, ctrl_n) else {
                panic!("{focus:?} で ctrl+n が Action にならない");
            };
            let effects = ws
                .dispatch(action)
                .unwrap_or_else(|| global_effects(&ws, action));
            assert!(
                matches!(
                    effects.as_slice(),
                    [Effect::Command(crate::command::CommandId::NewClaudeCode)]
                ),
                "{focus:?}: {effects:?}"
            );
            assert!(matches!(
                crate::command::execute(&mut ws, crate::command::CommandId::NewClaudeCode)
                    .as_slice(),
                [Effect::NewSession(SessionKind::ClaudeCode)]
            ));
        }

        let mut ws = Workspace::for_test();
        ws.focus = Focus::TerminalClaude;
        assert!(matches!(route(&mut ws, ctrl_n), Routed::ForwardToPty(_)));
    }
}
