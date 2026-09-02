//! メインループ。待つ → 入力を捌く → svc の結果を消費 → 描く、の 1 周を回す。

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

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
    let mut dirty = true;

    apply(ws, svc, vec![Effect::Spawn(Task::ListWorktrees)]);

    loop {
        // 区画は描く前に決める。PTY のリサイズが描画の副産物になると、
        // 子プロセスは 1 フレーム古い幅に描いたものを送ってくる。
        let size = terminal.size()?;
        let last_layout = layout(ws, Rect::new(0, 0, size.width, size.height));
        ws.sync_layout(&last_layout);
        ws.panels.viewer.refresh_highlight(&ws.config);

        // 描くのが先。起動直後に既にキーが溜まっていると、捌いてから描く並びでは
        // 1 フレームも出さないまま終了しうる。
        if dirty {
            terminal.draw(|frame| render(frame, ws, &last_layout))?;
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
        dirty |= ws.panels.terminal.took_output();
        ws.panels.terminal.nudge();
        dirty |= run_timers(ws, svc, &mut worktree_poll, &mut pty_cleanup);

        if ws.should_quit {
            return Ok(());
        }
    }
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

/// watcher からの合図の行き先。MCP のレビュー更新だけがパネルの外に効く。
fn watch_effects(ws: &mut Workspace, watch: conductor_svc::watch::WatchEvent) -> Vec<Effect> {
    match watch {
        conductor_svc::watch::WatchEvent::RefreshRequested => {
            vec![Effect::Spawn(Task::LoadReview)]
        }
        watch => ws.panels.terminal.on_watch(&watch),
    }
}

fn run_timers(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    worktree_poll: &mut Timer,
    pty_cleanup: &mut Timer,
) -> bool {
    let now = Instant::now();
    let mut dirty = false;
    if worktree_poll.due(now) {
        apply(ws, svc, vec![Effect::Spawn(Task::ListWorktrees)]);
    }
    if pty_cleanup.due(now) {
        dirty |= ws.panels.terminal.cleanup_dead();
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

/// クリックした区画へフォーカスを移し、その区画に行を渡す。ホイールは
/// フォーカスを動かさない — 覗くだけでキーの行き先が変わると事故になる。
fn on_mouse(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    layout: &Layout,
    mouse: MouseEvent,
) {
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
        MouseEventKind::ScrollDown => scroll_region(ws, region, 3),
        MouseEventKind::ScrollUp => scroll_region(ws, region, -3),
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(focus) = focus_for(region) else {
                return;
            };
            apply(ws, svc, vec![Effect::Focus(focus)]);
            let effects = match focus {
                Focus::Explorer => ws.panels.explorer.click(mouse.row, &ws.review),
                Focus::Viewer => {
                    let Workspace { panels, review, .. } = ws;
                    panels.viewer.click(
                        mouse.column,
                        mouse.row,
                        mouse.modifiers.contains(KeyModifiers::SHIFT),
                        review,
                    )
                }
                _ => Vec::new(),
            };
            apply(ws, svc, effects);
        }
        _ => {}
    }
}

fn scroll_region(ws: &mut Workspace, region: Region, delta: isize) {
    match region {
        Region::ExplorerTree | Region::ExplorerChanges => {
            let Workspace { panels, review, .. } = ws;
            panels.explorer.scroll(region, delta, review)
        }
        Region::Viewer => ws.panels.viewer.scroll_lines(delta),
        _ => {}
    }
}

fn focus_for(region: Region) -> Option<Focus> {
    match region {
        Region::WorktreeStrip => Some(Focus::Worktree),
        Region::ExplorerTree | Region::ExplorerChanges => Some(Focus::Explorer),
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
