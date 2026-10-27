//! メインループ。待つ → 入力を捌く → svc の結果を消費 → 描く、の 1 周を回す。

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use conductor_svc::watch::FileWatcher;
use conductor_svc::{EventKind, Services};

use crate::effect::{Effect, apply};
use crate::layout::{Layout, Region, layout};
use crate::liveness::{Liveness, liveness};
use crate::panels::explorer::ExplorerPanel;
use crate::render::render;
use crate::route::{Routed, global_effects, route};
use crate::select;
use crate::task::{Task, TaskResult};
use crate::timer::{
    CONFIG_DEBOUNCE, Debounce, FLEET_POLL, FS_DEBOUNCE, GIT_DEBOUNCE, PTY_CLEANUP, Timer,
    WORKTREE_POLL,
};
use crate::workspace::{Focus, StatusLevel, Workspace};

/// フレームを描く前にイベントを捌き続ける上限。高速スクロール中に中間フレームが
/// 出なくなるのを防ぐ (トラックパッドの慣性は 100 個超のイベントを出しうる)。
const MAX_DRAIN: Duration = Duration::from_millis(8);
/// 最後の入力のあと Active の tick を使い続ける時間。
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
/// フラッシュメッセージが消えるまで。
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

/// watcher の合図が静まるのを待つ締切。
struct Debounces {
    fs: Debounce,
    git: Debounce,
    config: Debounce,
}

/// 監視先は worktree 一覧が届いてから決まり、リポジトリ切り替えでも変わる。
#[derive(Default)]
struct FileWatch {
    watcher: Option<FileWatcher>,
    watching: Vec<PathBuf>,
}

impl FileWatch {
    /// 監視先が変わっていたら作り直す。描き直しが要るなら true。
    fn sync(&mut self, ws: &mut Workspace, svc: &mut Services<TaskResult>) -> bool {
        let desired = watch_paths(ws);
        if desired == self.watching {
            return false;
        }
        match FileWatcher::new(&desired, svc.sender()) {
            Ok(watcher) => {
                self.watching = desired;
                self.watcher = Some(watcher);
                false
            }
            // 前の監視を残す。古いパス集合にはまだ効くので、丸ごと失うよりましなため。
            Err(e) => {
                log::warn!("file watcher rebuild failed: {e:#}");
                let status = Effect::Status(
                    StatusLevel::Warning,
                    format!("File watcher rebuild failed ({e})"),
                );
                apply(ws, svc, vec![status]);
                true
            }
        }
    }
}

/// 監視するのは各 worktree。一覧が空のときリポジトリ自身にするのは、git でない
/// ディレクトリでも Explorer が追随するため。
fn watch_paths(ws: &Workspace) -> Vec<PathBuf> {
    let worktrees = ws.panels.worktree.list();
    if worktrees.is_empty() {
        return vec![ws.repo.root.clone()];
    }
    worktrees.iter().map(|w| w.path.clone()).collect()
}

pub fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
) -> Result<()> {
    let mut last_input = Instant::now();
    let mut worktree_poll = Timer::new(WORKTREE_POLL, last_input);
    let mut pty_cleanup = Timer::new(PTY_CLEANUP, last_input);
    let mut fleet_poll = Timer::new(FLEET_POLL, last_input);
    let update_interval = Duration::from_secs(ws.config.updates.check_interval_secs);
    let mut update_poll = ws
        .config
        .updates
        .check_on_startup
        .then(|| Timer::new(update_interval, last_input));
    let mut dirty = true;
    let mut debounces = Debounces {
        fs: Debounce::new(FS_DEBOUNCE),
        git: Debounce::new(GIT_DEBOUNCE),
        config: Debounce::new(CONFIG_DEBOUNCE),
    };
    let mut file_watch = FileWatch::default();

    let mut startup = vec![
        Effect::Spawn(Task::ListWorktrees),
        Effect::Spawn(Task::LoadGrabState),
        Effect::OpenFleet,
    ];
    if update_poll.is_some() {
        startup.push(Effect::Spawn(Task::CheckForUpdate {
            max_age: update_interval,
            announce: false,
        }));
    }
    apply(ws, svc, startup);
    file_watch.sync(ws, svc);

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
            ws.fx.start_pending();
            let mut clipboard = None;
            terminal.draw(|frame| {
                render(frame, ws, &last_layout);
                if ws.chrome.copy_requested {
                    clipboard = selected_text(ws, &last_layout, frame.buffer_mut())
                        .filter(|text| !text.is_empty());
                }
            })?;
            let _ = execute!(io::stdout(), EndSynchronizedUpdate);
            ws.chrome.copy_requested = false;
            if let Some(text) = clipboard {
                let mut stdout = io::stdout();
                let _ = stdout
                    .write_all(&select::osc52(&text))
                    .and_then(|_| stdout.flush());
                select::system_copy(&text);
                apply(
                    ws,
                    svc,
                    vec![Effect::Status(
                        StatusLevel::Info,
                        "Copied to clipboard".into(),
                    )],
                );
            }
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
                EventKind::Watch(watch) => watch_effects(ws, watch, &mut debounces, Instant::now()),
            };
            apply(ws, svc, effects);
            dirty = true;
        }

        dirty |= expire_status(ws);
        dirty |= tick_modals(ws, svc);
        dirty |= tick_index(ws, svc);
        dirty |= ws.tick_viewer();
        dirty |= tick_editor(ws, svc);
        dirty |= tick_fs_refresh(ws, svc, &mut debounces.fs, Instant::now());
        dirty |= tick_git_refresh(ws, svc, &mut debounces.git, Instant::now());
        dirty |= tick_config_reload(ws, svc, &mut debounces.config, Instant::now());
        dirty |= ws.panels.terminal.took_output();
        ws.panels.terminal.flush_deferred();
        dirty |= run_timers(
            ws,
            svc,
            &mut worktree_poll,
            &mut file_watch,
            &mut pty_cleanup,
            update_poll.as_mut().map(|timer| (timer, update_interval)),
        );
        poll_fleet(ws, svc, &mut fleet_poll, Instant::now());
        poll_fleet_changes(ws, svc, Instant::now());
        dirty |= ws.fx.tick();

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
fn watch_effects(
    ws: &mut Workspace,
    watch: conductor_svc::watch::WatchEvent,
    debounces: &mut Debounces,
    now: Instant,
) -> Vec<Effect> {
    use conductor_svc::watch::WatchEvent;
    match watch {
        WatchEvent::RefreshRequested => vec![Effect::Spawn(Task::LoadReview)],
        WatchEvent::UiRequest { cwd, request } => crate::remote::effects(ws, &cwd, request),
        // 索引の作り直しは自前の静穏時間で数えるので、1 件ずつそのまま渡す。
        WatchEvent::FsChanged(path) => {
            crate::index::note_change(ws, &path);
            debounces.fs.touch(now);
            Vec::new()
        }
        WatchEvent::GitChanged => {
            debounces.git.touch(now);
            Vec::new()
        }
        WatchEvent::ConfigChanged => {
            debounces.config.touch(now);
            Vec::new()
        }
        watch => ws.panels.terminal.on_watch(&watch),
    }
}

/// 猶予が明けていたら Explorer を投げ直す。何か投げたら true。
fn tick_fs_refresh(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    debounce: &mut Debounce,
    now: Instant,
) -> bool {
    tick_explorer(ws, svc, debounce, now, ExplorerPanel::refresh)
}

fn tick_git_refresh(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    debounce: &mut Debounce,
    now: Instant,
) -> bool {
    tick_explorer(ws, svc, debounce, now, ExplorerPanel::refresh_git)
}

fn tick_explorer(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    debounce: &mut Debounce,
    now: Instant,
    refresh: fn(&mut ExplorerPanel) -> Vec<Effect>,
) -> bool {
    if !debounce.fire(now) {
        return false;
    }
    let effects = refresh(&mut ws.panels.explorer);
    apply(ws, svc, effects);
    true
}

/// 猶予が明けていたら設定を読み直す。反映するものがあれば true。
fn tick_config_reload(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    debounce: &mut Debounce,
    now: Instant,
) -> bool {
    if !debounce.fire(now) {
        return false;
    }
    let effects = ws.reload_config();
    if effects.is_empty() {
        return false;
    }
    apply(ws, svc, effects);
    true
}

fn run_timers(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    worktree_poll: &mut Timer,
    file_watch: &mut FileWatch,
    pty_cleanup: &mut Timer,
    update_poll: Option<(&mut Timer, Duration)>,
) -> bool {
    let now = Instant::now();
    let mut dirty = false;
    if worktree_poll.due(now) {
        if !ws.panels.worktree.is_listing() {
            apply(ws, svc, vec![Effect::Spawn(Task::ListWorktrees)]);
        }
        // 一覧と同じ周期で見直す。作り直しに失敗しても次の周回で試し直せる。
        dirty |= file_watch.sync(ws, svc);
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

fn poll_fleet(ws: &mut Workspace, svc: &mut Services<TaskResult>, timer: &mut Timer, now: Instant) {
    if !timer.due(now) || ws.panels.fleet.is_polling() {
        return;
    }
    let task = fleet_status_task(ws);
    apply(ws, svc, vec![Effect::Spawn(task)]);
}

fn poll_fleet_changes(ws: &mut Workspace, svc: &mut Services<TaskResult>, now: Instant) {
    if !ws.chrome.fleet_open {
        return;
    }
    let fleet = &ws.panels.fleet;
    if let Some(dir) = fleet.changes.wanted(fleet.selected_dir(), now) {
        apply(ws, svc, vec![Effect::Spawn(Task::FleetChanges { dir })]);
    }
}

/// この窓で開いた repo は、セッションが無くても木を引く。
fn fleet_status_task(ws: &Workspace) -> Task {
    let sessions = ws.panels.terminal.own_claude_sessions();
    let dirs = sessions.iter().map(|s| s.working_dir.clone()).collect();
    let own = sessions
        .into_iter()
        .filter_map(|s| Some((s.working_dir, s.claude_session_id?)))
        .collect();
    let mut roots = ws.panels.fleet.kept_roots();
    roots.extend(ws.repo.known.iter().cloned());
    Task::FleetStatus {
        known: ws.panels.fleet.known_transcripts(),
        own,
        dirs,
        roots,
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
    ws.fx.skip();
    loop {
        match crossterm::event::read()? {
            // オートリピートは押下と同じに扱う。Repeat が届くのは kitty プロトコル下
            // だけで、無い端末は Press の連続として送ってくる。
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                on_key(ws, svc, key);
            }
            Event::Mouse(mouse) => on_mouse(ws, svc, layout, mouse),
            Event::Paste(data) => on_paste(ws, svc, data),
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
    ws.chrome.selection = None;
    let effects = match route(ws, key) {
        Routed::Effects(effects) => effects,
        Routed::Action(action) => ws
            .dispatch(action)
            .unwrap_or_else(|| global_effects(ws, action)),
        Routed::ForwardToPty(key) => {
            if let Some(session) = ws.pty_session() {
                ws.panels.terminal.forward_key(key, &session);
            }
            Vec::new()
        }
        Routed::Ignored => Vec::new(),
    };
    apply(ws, svc, effects);
}

/// 貼り付け 1 つ分。macOS の端末は IME で確定したマルチバイト文字をキーではなく
/// bracketed paste で届けるので、これを捨てると日本語が 1 文字ずつしか入らない。
/// キーと同じ順で最前面のモーダルに優先権を渡す (裏の PTY へ流れると入力欄から消える)。
///
/// ドロップされたファイルは端末が落とした位置を教えないので、フォーカスに関係なく Claude Code へ流す。
pub fn on_paste(ws: &mut Workspace, svc: &mut Services<TaskResult>, data: String) {
    if let Some(modal) = ws.modals.last_mut() {
        modal.paste(&data);
        return;
    }
    let typing_into_fleet = matches!(ws.focus, Focus::FleetSession | Focus::FleetShell);
    if dropped_file(&data).is_some() && !typing_into_fleet {
        if ws.chrome.fleet_open {
            apply(
                ws,
                svc,
                vec![Effect::Status(
                    StatusLevel::Warning,
                    "Leave the session list to drop a file into Claude.".into(),
                )],
            );
            return;
        }
        if ws.panels.terminal.paste_to_claude(&data) {
            apply(ws, svc, vec![Effect::Focus(Focus::TerminalClaude)]);
            return;
        }
    }
    if let Some(session) = ws.pty_session() {
        ws.panels.terminal.paste(&data, &session);
    }
}

/// 貼り付けがファイルのドロップなら、そのパス。
///
/// 端末はドロップしたパスの後ろに空白を 1 つ足す — Cmd+V で貼ったパスには無いので、
/// これが両者を分ける印。空白を含むパスは iTerm2 や Terminal.app が `\ ` に、
/// Alacritty が `'…'` に包むので、存在を確かめる前にほどく。
fn dropped_file(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim_end_matches([' ', '\n', '\r']);
    if trimmed.len() == text.len() || trimmed.contains('\n') {
        return None;
    }
    let unquoted = trimmed
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(trimmed);
    let mut path = String::with_capacity(unquoted.len());
    let mut chars = unquoted.chars();
    while let Some(c) = chars.next() {
        path.push(if c == '\\' { chars.next()? } else { c });
    }
    let path = PathBuf::from(path);
    (path.is_absolute() && path.is_file()).then_some(path)
}

fn selected_text(ws: &Workspace, layout: &Layout, buffer: &Buffer) -> Option<String> {
    let selection = ws.chrome.selection.as_ref()?;
    let area = select::area(layout, selection.region)?;
    Some(select::text(buffer, selection, area))
}

/// クリックした区画へフォーカスを移し、その区画に行を渡す。ホイールは
/// フォーカスを動かさない — 覗くだけでキーの行き先が変わると事故になる。
fn on_mouse(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    layout: &Layout,
    mouse: MouseEvent,
) {
    // 下区画の見出しは境界の行にあるが、文字が並んでいる範囲だけは一覧の切り替えに使う。
    // 残りの桁はそのまま境界のドラッグへ渡す — 枠を掴んで高さを変える操作を潰さない。
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && let Some(effects) = bottom_title_click(ws, layout, mouse)
    {
        apply(ws, svc, effects);
        return;
    }
    if let Some(effects) = drag_divider(ws, layout, mouse) {
        apply(ws, svc, effects);
        return;
    }
    if drag_select(ws, layout, mouse) {
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
            let root = ws.panels.viewer.root().to_path_buf();
            let (panels, _, ctx) = ws.split(&root);
            if region == Region::Viewer {
                panels.viewer.pointer_moved(mouse.column, mouse.row, &ctx);
            } else {
                panels.viewer.note_pointer(None);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // 開いているポップアップが先。区画に渡すと裏の一覧が反応してしまう。
            if let Some(crate::modal::Modal::Popup(popup)) = ws.modals.last_mut() {
                let effects = popup.click(mouse.column, mouse.row, layout.area);
                apply(ws, svc, effects);
                return;
            }
            let columns = viewer_code_columns(ws, region, mouse);
            ws.chrome.selection = select::area(layout, region).map(|_| {
                let selection = select::Selection::begin(region, mouse.column, mouse.row);
                // None (ガター等) はそのまま区画まるごと — 行番号ごと持っていきたい選択もある。
                columns.map_or(selection, |c| selection.within_columns(c))
            });
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
                && let Some(effects) = ws.panels.terminal.tab_click(focus, mouse.column, tabs)
            {
                apply(ws, svc, effects);
                return;
            }
            // 「最新へ」チップは PTY の中身より上にある。
            let chip = match focus {
                Focus::TerminalClaude => layout.rect(region).map(|rect| {
                    let session = ws.panels.terminal.visible_claude().map(str::to_string);
                    (crate::panels::terminal::render::content_area(rect), session)
                }),
                Focus::FleetSession => layout.rect(region).map(|rect| {
                    (
                        crate::panels::terminal::render::editor_area(rect),
                        ws.fleet_session(),
                    )
                }),
                _ => None,
            };
            if let Some((area, session)) = chip
                && ws.panels.terminal.transcript_click(
                    area,
                    session.as_deref(),
                    mouse.column,
                    mouse.row,
                )
            {
                return;
            }
            let effects = match focus {
                Focus::Explorer => ws
                    .panels
                    .explorer
                    .click(mouse.column, mouse.row, &ws.review),
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
                Focus::Fleet => fleet_row_click(ws, layout, mouse),
                _ => Vec::new(),
            };
            apply(ws, svc, effects);
        }
        _ => {}
    }
}

fn fleet_row_click(ws: &mut Workspace, layout: &Layout, mouse: MouseEvent) -> Vec<Effect> {
    use crate::panels::fleet::{ListPress, render};
    if !ws.modals.is_empty() {
        return Vec::new();
    }
    let Some(rect) = layout.rect(Region::FleetList) else {
        return Vec::new();
    };
    let Some(composed) = render::compose(ws, rect) else {
        return Vec::new();
    };
    let (x, y) = (mouse.column, mouse.row);
    let press = ListPress {
        hit: render::slot_at(&composed, x, y),
        button: render::button_hit(&composed, x, y),
        in_button_columns: render::in_button_columns(&composed, x),
    };
    ws.panels.fleet.click_list((x, y), press)
}

/// Viewer の region のときだけ code_columns で桁を絞る。
fn viewer_code_columns(
    ws: &mut Workspace,
    region: Region,
    mouse: MouseEvent,
) -> Option<(u16, u16)> {
    if region != Region::Viewer {
        return None;
    }
    let root = ws.panels.viewer.root().to_path_buf();
    let (panels, _, ctx) = ws.split(&root);
    panels.viewer.code_columns(mouse.column, mouse.row, &ctx)
}

fn drag_select(ws: &mut Workspace, layout: &Layout, mouse: MouseEvent) -> bool {
    let Some(selection) = ws.chrome.selection.as_mut() else {
        return false;
    };
    match mouse.kind {
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(area) = select::area(layout, selection.region) {
                selection.extend(mouse.column, mouse.row, area);
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if selection.is_empty() {
                ws.chrome.selection = None;
            } else {
                ws.chrome.copy_requested = true;
            }
            true
        }
        _ => false,
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

/// 下区画の見出しを押したら、何を出すかを選ぶポップアップを開く。見出しの外なら None。
fn bottom_title_click(ws: &Workspace, layout: &Layout, mouse: MouseEvent) -> Option<Vec<Effect>> {
    let rect = layout.rect(Region::ExplorerChanges)?;
    if mouse.row != rect.y {
        return None;
    }
    let title = crate::panels::explorer::render::bottom_title(&ws.panels.explorer, ws);
    let zone = rect.x + 1..rect.x + 1 + crate::strip::width_of(&title);
    if !zone.contains(&mouse.column) {
        return None;
    }
    let explorer = &ws.panels.explorer;
    let current = match (explorer.bottom(), explorer.changes.listing()) {
        (crate::panels::explorer::BottomView::Comments, _) => 2,
        (_, crate::panels::explorer::git_changes::Listing::Log) => 1,
        _ => 0,
    };
    Some(vec![Effect::PushModal(crate::modal::Modal::Popup(
        crate::modal::popup::Popup::new("Show", listings(), Some((rect.x, rect.y + 1)), current),
    ))])
}

fn listings() -> crate::modal::popup::Choices {
    use crate::command::CommandId;
    vec![
        ("Changes", Effect::Command(CommandId::ShowDiffList)),
        ("Commit Log", Effect::Command(CommandId::ShowCommitLog)),
        ("Comments", Effect::Command(CommandId::ShowCommentList)),
    ]
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
    ws.chrome.selection = None;
    match region {
        Region::ExplorerTree | Region::ExplorerChanges => {
            let Workspace { panels, review, .. } = ws;
            panels.explorer.scroll(region, delta, review)
        }
        Region::Viewer => ws.panels.viewer.scroll_lines(delta),
        // 端末はホイールの行き先が中身次第 (子プロセス / トランスクリプト /
        // スクロールバック) なので、判断はパネルが持つ。
        Region::TerminalClaude | Region::TerminalShell | Region::Editor | Region::FleetSession => {
            let Some(rect) = layout.rect(region) else {
                return;
            };
            let fleet_session = ws.fleet_session();
            let effects = ws.panels.terminal.wheel(
                region,
                rect,
                mouse.column,
                mouse.row,
                delta,
                fleet_session,
            );
            apply(ws, svc, effects);
        }
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
        Region::FleetList => Some(Focus::Fleet),
        Region::FleetSession => Some(Focus::FleetSession),
        Region::FleetChanges => Some(Focus::FleetShell),
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn test_debounces() -> Debounces {
        Debounces {
            fs: Debounce::new(FS_DEBOUNCE),
            git: Debounce::new(GIT_DEBOUNCE),
            config: Debounce::new(CONFIG_DEBOUNCE),
        }
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
        ws.modals
            .push(Modal::Prompt(crate::modal::Prompt::single("t", |_| {
                Vec::new()
            })));
        on_paste(&mut ws, &mut Services::new(), "日本語".into());
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
        on_paste(&mut ws, &mut Services::new(), "日本語".into());
        assert!(ws.modals.is_empty());
        assert!(ws.chrome.status.is_none());
    }

    #[test]
    fn ドロップは末尾の空白と存在で見分ける() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("shot.png");
        let spaced = dir.path().join("Screenshot 2026-09-08 at 18.02.11.png");
        std::fs::write(&plain, b"").unwrap();
        std::fs::write(&spaced, b"").unwrap();
        let escaped = spaced.display().to_string().replace(' ', "\\ ");

        assert_eq!(
            dropped_file(&format!("{} ", plain.display())),
            Some(plain.clone())
        );
        assert_eq!(dropped_file(&format!("{escaped} ")), Some(spaced.clone()));
        assert_eq!(
            dropped_file(&format!("'{}' ", spaced.display())),
            Some(spaced.clone())
        );
        assert_eq!(
            dropped_file(&plain.display().to_string()),
            None,
            "Cmd+V で貼ったパスには末尾の空白が無い"
        );
        assert_eq!(
            dropped_file(&format!("{} ", dir.path().join("nope.png").display())),
            None
        );
        assert_eq!(
            dropped_file(&format!("{}\n{} ", plain.display(), plain.display())),
            None
        );
    }

    #[test]
    fn ドロップしたファイルはフォーカスがどこでもclaude_codeへ行く() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let dir = tempfile::tempdir().unwrap();
        ws.config.general.shell = "/bin/sh".into();
        ws.repo.root = dir.path().to_path_buf();
        ws.panels
            .terminal
            .follow_worktree(Some(dir.path().to_path_buf()));
        let file = dir.path().join("shot.png");
        std::fs::write(&file, b"").unwrap();

        ws.focus = Focus::Viewer;
        on_paste(&mut ws, &mut svc, format!("{} ", file.display()));
        assert_eq!(
            ws.focus,
            Focus::Viewer,
            "Claude Code のセッションが無ければ今まで通り捨てる"
        );

        ws.panels.terminal.show_shell_as_claude_for_test(dir.path());
        on_paste(&mut ws, &mut svc, format!("{} ", file.display()));
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn 止めて終了はセッションがあるときだけ確かめてから止めて閉じる() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let id = crate::command::CommandId::QuitAndStopSessions;
        assert!(matches!(
            crate::command::execute(&mut ws, id).as_slice(),
            [Effect::Quit]
        ));

        ws.panels.terminal.show_shell_as_claude_for_test(dir.path());
        let effects = crate::command::execute(&mut ws, id);
        assert!(matches!(
            effects.as_slice(),
            [Effect::PushModal(Modal::Confirm(_))]
        ));
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::StopSessions(None), Effect::Quit],
        );
        assert_eq!(ws.panels.terminal.session_count(), 0);
        assert!(ws.should_quit);
    }

    #[test]
    fn 消したworktreeのセッションだけを止める() {
        let (kept, deleted) = (
            tempfile::TempDir::new().unwrap(),
            tempfile::TempDir::new().unwrap(),
        );
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.panels
            .terminal
            .show_shell_as_claude_for_test(kept.path());
        ws.panels
            .terminal
            .show_shell_as_claude_for_test(deleted.path());

        let stop = Effect::StopSessions(Some(deleted.path().to_path_buf()));
        apply(&mut ws, &mut svc, vec![stop]);

        assert_eq!(ws.panels.terminal.session_count(), 1);
        assert!(!ws.panels.terminal.has_claude_session(deleted.path()));
    }

    #[test]
    fn 本文から始めたドラッグは行番号を外した文字列を書き出す() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "alpha\nbravo\n").unwrap();
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Viewer;
        let effects = ws.panels.viewer.set_root(dir.path().to_path_buf());
        apply(&mut ws, &mut svc, effects);
        let effects = ws
            .panels
            .viewer
            .open(std::path::Path::new("a.rs"), None, None, false);
        apply(&mut ws, &mut svc, effects);
        crate::testing::pump(&mut ws, &mut svc);

        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        ws.panels.viewer.sync_layout(&l);
        let viewer = l.rect(Region::Viewer).unwrap();
        let body_y = viewer.y + 2;
        let at = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let drag_from = |ws: &mut Workspace, svc: &mut Services<TaskResult>, x| {
            on_mouse(
                ws,
                svc,
                &l,
                at(MouseEventKind::Down(MouseButton::Left), x, body_y),
            );
            on_mouse(
                ws,
                svc,
                &l,
                at(
                    MouseEventKind::Drag(MouseButton::Left),
                    viewer.right() - 2,
                    body_y + 1,
                ),
            );
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal
                .draw(|frame| crate::render::render(frame, ws, &l))
                .unwrap();
            selected_text(ws, &l, terminal.backend().buffer()).unwrap()
        };

        // 枠の内側から、行番号 1 桁 + 畳みの印 + 仕切り " │ " を越えた先が本文。
        let code = drag_from(&mut ws, &mut svc, viewer.x + 1 + 5);
        assert_eq!(code, "alpha\nbravo");

        let gutter = drag_from(&mut ws, &mut svc, viewer.x + 1);
        assert!(
            gutter.contains('\u{2502}'),
            "ガターから押せば今までどおり: {gutter}"
        );
    }

    #[test]
    fn 一覧の右を押すと映しているセッションへ入る() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let dir = tempfile::tempdir().unwrap();
        ws.panels.terminal.show_shell_as_claude_for_test(dir.path());
        let id = ws.panels.terminal.visible_claude().unwrap().to_string();
        let own = crate::panels::fleet::OwnSession {
            panel_id: id,
            working_dir: dir.path().to_path_buf(),
            claude_session_id: None,
        };
        ws.apply_fleet(Default::default(), vec![own]);
        let l = layout(&ws, Rect::new(0, 0, 160, 40));
        let right = l.rect(Region::FleetSession).unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: right.x + 5,
            row: right.y + 5,
            modifiers: KeyModifiers::NONE,
        };
        on_mouse(&mut ws, &mut svc, &l, click);
        assert_eq!(ws.focus, Focus::FleetSession);
    }

    #[test]
    fn 一覧の右のホイールはフォーカスを動かさずにそのセッションのログを開きに行く() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let dir = tempfile::tempdir().unwrap();
        ws.panels.terminal.show_shell_as_claude_for_test(dir.path());
        let own = crate::panels::fleet::OwnSession {
            panel_id: ws.panels.terminal.visible_claude().unwrap().to_string(),
            working_dir: dir.path().to_path_buf(),
            claude_session_id: None,
        };
        ws.apply_fleet(Default::default(), vec![own]);
        let focus = ws.focus;
        let l = layout(&ws, Rect::new(0, 0, 160, 40));
        let right = l.rect(Region::FleetSession).unwrap();
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: right.x + 5,
            row: right.y + 5,
            modifiers: KeyModifiers::NONE,
        };
        on_mouse(&mut ws, &mut svc, &l, wheel);

        assert_eq!(ws.focus, focus);
        let status = ws.chrome.status.as_ref().map(|s| s.text.as_str());
        assert_eq!(
            status,
            Some("this panel has not reported its session id yet"),
            "右に映したセッションのログを開きに行っていない"
        );
    }

    #[test]
    fn 区画の中でドラッグすると選択が枠の内側に収まり離すと書き出す番になる() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        let l = layout(&ws, Rect::new(0, 0, 120, 40));
        let viewer = l.rect(Region::Viewer).unwrap();
        let at = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        let down = at(
            MouseEventKind::Down(MouseButton::Left),
            viewer.x + 3,
            viewer.y + 3,
        );
        on_mouse(&mut ws, &mut svc, &l, down);
        let up_in_place = at(
            MouseEventKind::Up(MouseButton::Left),
            viewer.x + 3,
            viewer.y + 3,
        );
        on_mouse(&mut ws, &mut svc, &l, up_in_place);
        assert_eq!(
            ws.chrome.selection, None,
            "動かさずに離したクリックは選ばない"
        );
        assert!(!ws.chrome.copy_requested);

        on_mouse(&mut ws, &mut svc, &l, down);
        let outside = at(
            MouseEventKind::Drag(MouseButton::Left),
            0,
            viewer.bottom() + 5,
        );
        on_mouse(&mut ws, &mut svc, &l, outside);
        let selection = ws.chrome.selection.expect("ドラッグで選択が始まる");
        assert_eq!(selection.region, Region::Viewer);
        assert!(selection.contains(viewer.x + 1, viewer.bottom() - 2));
        assert!(
            !selection.contains(viewer.x + 2, viewer.bottom() - 2),
            "先端は枠の内側の左端で止まる"
        );
        assert!(
            !selection.contains(viewer.x + 1, viewer.bottom() - 1),
            "先端は枠の内側の下端で止まる"
        );

        on_mouse(
            &mut ws,
            &mut svc,
            &l,
            at(MouseEventKind::Up(MouseButton::Left), 0, 0),
        );
        assert!(ws.chrome.copy_requested);
        assert!(ws.chrome.selection.is_some(), "書き出すまで選択は残る");

        press(&mut ws, &[key(KeyCode::Char('j'))]);
        assert_eq!(ws.chrome.selection, None, "キーを押したら消える");
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

        crate::testing::wait_for(|| tick_editor(&mut ws, &mut svc));
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

        ws.fx
            .play(crate::fx::Kind::assemble(), crate::fx::Target::Panels);
        assert_eq!(liveness(&ws, false), Liveness::Active);
        ws.fx.skip();
        assert_eq!(liveness(&ws, false), Liveness::Idle);

        // 数分続く周回も同じ理由で Active。
        let viewer = crate::fx::Target::Region(Region::Viewer);
        ws.fx.play(crate::fx::Kind::Orbit, viewer);
        assert_eq!(liveness(&ws, false), Liveness::Active);
        ws.fx.stop(&crate::fx::Kind::Orbit, viewer);
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

    fn intaken(
        worktree: &std::path::Path,
        branch: &str,
    ) -> Box<conductor_core::pr_intake::IntakenPr> {
        Box::new(conductor_core::pr_intake::IntakenPr {
            pr_number: 9,
            worktree_path: worktree.to_path_buf(),
            meta: conductor_core::pr_intake::FetchedPr {
                branch: branch.into(),
                title: String::new(),
                base_ref: "main".into(),
                head_ref: branch.into(),
                url: String::new(),
                head_owner_login: None,
            },
            pulled: None,
        })
    }

    #[test]
    fn pr取り込みが成功すると閉じてその_worktreeへ移る() {
        let repo = crate::testing::TestRepo::new();
        let fetched = repo.worktree("feature/pr-head");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        ws.modals.push(Modal::PrInput(Default::default()));

        let effects = ws.accept(TaskResult::PrIntake(Ok(intaken(
            &fetched,
            "feature/pr-head",
        ))));
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::Spawn(crate::task::Task::ListWorktrees),
                    Effect::PopModal,
                    Effect::Focus(Focus::Explorer),
                ]
            ),
            "{effects:?}"
        );
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
    fn 追いつかせた取り込みの帯は_git_の答えだけを言う() {
        let repo = crate::testing::TestRepo::new();
        let fetched = repo.worktree("feature/pr-head");
        let (mut ws, _svc) = crate::testing::workspace_for(&repo);
        let mut pr = intaken(&fetched, "feature/pr-head");
        pr.pulled = Some("fast-forwarded".into());

        let effects = ws.accept(TaskResult::PrIntake(Ok(pr)));
        let band: Vec<&str> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Status(_, text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(band, ["PR #9: fast-forwarded"], "{effects:?}");
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
    fn リモートブランチは作る直前に_fetch_して最新で切る() {
        let repo = crate::testing::TestRepo::new();
        repo.remote_branch("feature/remote-only");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        run_command(&mut ws, &mut svc, crate::command::CommandId::SwitchBranch);
        crate::testing::pump(&mut ws, &mut svc);

        repo.commit_in(&repo.root(), "b.txt", "beta\n", "second");
        let tip = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["push", "origin", "main"]);
        let origin = repo.origin();
        repo.git(&[
            "-C",
            origin.to_str().unwrap(),
            "update-ref",
            "refs/heads/feature/remote-only",
            &tip,
        ]);

        type_text(&mut ws, &mut svc, "remote-only");
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);

        let created = ws
            .panels
            .worktree
            .list()
            .iter()
            .find(|w| w.branch == "feature/remote-only")
            .unwrap_or_else(|| panic!("{:?}", ws.panels.worktree.list()));
        assert_eq!(created.head_oid.as_deref(), Some(tip.as_str()));
        let status = ws.chrome.status.as_ref().expect("status");
        assert_eq!(status.level, StatusLevel::Success, "{}", status.text);
    }

    #[test]
    fn ベースが取れなくてもリモートブランチは最新で作られ_画面に警告が出る() {
        let repo = crate::testing::TestRepo::new();
        repo.remote_branch("feature/remote-only");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        ws.repo.main_branch = "gone".into();
        run_command(&mut ws, &mut svc, crate::command::CommandId::SwitchBranch);
        crate::testing::pump(&mut ws, &mut svc);

        repo.commit_in(&repo.root(), "b.txt", "beta\n", "second");
        let tip = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["push", "origin", "main"]);
        let origin = repo.origin();
        repo.git(&[
            "-C",
            origin.to_str().unwrap(),
            "update-ref",
            "refs/heads/feature/remote-only",
            &tip,
        ]);

        type_text(&mut ws, &mut svc, "remote-only");
        on_key(&mut ws, &mut svc, key(KeyCode::Enter));
        crate::testing::pump(&mut ws, &mut svc);

        let created = ws
            .panels
            .worktree
            .list()
            .iter()
            .find(|w| w.branch == "feature/remote-only")
            .unwrap_or_else(|| panic!("{:?}", ws.panels.worktree.list()));
        assert_eq!(created.head_oid.as_deref(), Some(tip.as_str()));
        let status = ws.chrome.status.as_ref().expect("status");
        assert_eq!(status.level, StatusLevel::Warning, "{}", status.text);
        assert!(status.text.contains("gone"), "{}", status.text);
        assert!(!status.text.contains('\n'), "{}", status.text);
    }

    #[test]
    fn 新しいworktreeは作る直前にoriginを取り直して最新のベースから切る() {
        let repo = crate::testing::TestRepo::new();
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        repo.commit_in(&repo.root(), "b.txt", "beta\n", "second");
        let tip = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["push", "origin", "main"]);
        // 手元だけ 1 つ前に戻し、origin が先に進んだことを知らない状態にする。
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        repo.git(&["reset", "-q", "--hard", "HEAD~1"]);

        apply(
            &mut ws,
            &mut svc,
            vec![Effect::Spawn(Task::CreateWorktree {
                branch: "topic".into(),
            })],
        );
        crate::testing::pump(&mut ws, &mut svc);

        let created = ws
            .panels
            .worktree
            .list()
            .iter()
            .find(|w| w.branch == "topic")
            .unwrap_or_else(|| panic!("{:?}", ws.panels.worktree.list()));
        assert_eq!(created.head_oid.as_deref(), Some(tip.as_str()));
    }

    #[test]
    fn originが無いリポジトリでは_worktree作成に警告が出ない() {
        let repo = crate::testing::TestRepo::new();
        repo.git(&["remote", "remove", "origin"]);
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);

        apply(
            &mut ws,
            &mut svc,
            vec![Effect::Spawn(Task::CreateWorktree {
                branch: "topic".into(),
            })],
        );
        crate::testing::pump(&mut ws, &mut svc);

        let status = ws.chrome.status.as_ref().expect("status");
        assert_eq!(status.level, StatusLevel::Success, "{}", status.text);
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
        let listed: Vec<String> = crate::panels::explorer::render::bottom_lines(&ws, 60, 10)
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
        assert_eq!(prompt.title, "New worktree");
        assert_eq!(prompt.mode().label, "Branch name");
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
        let mut debounces = test_debounces();
        let now = Instant::now();
        assert!(matches!(
            watch_effects(&mut ws, WatchEvent::RefreshRequested, &mut debounces, now).as_slice(),
            [Effect::Spawn(Task::LoadReview)]
        ));

        assert!(watch_effects(&mut ws, WatchEvent::ConfigChanged, &mut debounces, now).is_empty());
        assert!(
            debounces.config.fire(now + CONFIG_DEBOUNCE),
            "ConfigChanged が設定の猶予を触っていない"
        );

        assert!(watch_effects(&mut ws, WatchEvent::GitChanged, &mut debounces, now).is_empty());
        assert!(
            !debounces.fs.fire(now + FS_DEBOUNCE),
            "GitChanged がファイル変更の猶予 (ツリーの走査) を触っている"
        );
        assert!(
            debounces.git.fire(now + GIT_DEBOUNCE),
            "GitChanged が git の猶予を触っていない"
        );
    }

    #[test]
    fn fs変更は猶予が明けるまでexplorerを投げない() {
        use conductor_svc::watch::WatchEvent;
        let dir = tempfile::TempDir::new().unwrap();
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        crate::testing::select_only_worktree(&mut ws, &mut svc, dir.path());
        crate::testing::pump(&mut ws, &mut svc);

        let mut debounces = test_debounces();
        let start = Instant::now();
        watch_effects(
            &mut ws,
            WatchEvent::FsChanged(dir.path().join("a.txt")),
            &mut debounces,
            start,
        );
        assert!(
            !tick_fs_refresh(&mut ws, &mut svc, &mut debounces.fs, start),
            "猶予の前に投げた"
        );

        assert!(
            tick_fs_refresh(&mut ws, &mut svc, &mut debounces.fs, start + FS_DEBOUNCE),
            "猶予明けで投げていない"
        );
    }

    #[test]
    fn 監視先はworktreeの一覧から決まる() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        assert_eq!(watch_paths(&ws), vec![ws.repo.root.clone()]);

        let dir = tempfile::TempDir::new().unwrap();
        crate::testing::select_only_worktree(&mut ws, &mut svc, dir.path());
        assert_eq!(watch_paths(&ws), vec![dir.path().to_path_buf()]);
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

    #[test]
    fn 一覧の木はこの窓で開いた_repo_もセッションが無くても引く() {
        let mut ws = Workspace::for_test();
        ws.repo.known.push(PathBuf::from("/r/opened"));
        let Task::FleetStatus { roots, .. } = fleet_status_task(&ws) else {
            panic!("not a fleet status task");
        };
        assert!(roots.contains(&PathBuf::from("/r/opened")), "{roots:?}");
    }

    fn fleet_with_tree() -> (Workspace, Services<TaskResult>, Layout) {
        fleet_listing(Vec::new(), 40)
    }

    fn fleet_listing(
        live: Vec<conductor_core::claude_sessions::registry::LiveSession>,
        height: u16,
    ) -> (Workspace, Services<TaskResult>, Layout) {
        use conductor_core::git_engine::{Head, WorktreeHead};
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let worktree = |path: &str, branch: &str| WorktreeHead {
            path: PathBuf::from(path),
            head: Head::Branch(branch.into()),
        };
        ws.apply_fleet(
            crate::panels::fleet::FleetStatus {
                live,
                trees: vec![crate::panels::fleet::RepoTree {
                    root: PathBuf::from("/r/app"),
                    name: "app".into(),
                    worktrees: vec![
                        worktree("/r/app", "main"),
                        worktree("/r/app-b", "feature/b"),
                    ],
                }],
                ..Default::default()
            },
            Vec::new(),
        );
        let l = layout(&ws, Rect::new(0, 0, 160, height));
        (ws, svc, l)
    }

    fn click_line(
        ws: &mut Workspace,
        svc: &mut Services<TaskResult>,
        l: &Layout,
        want: impl Fn(&crate::panels::fleet::render::Slot) -> bool,
        column: impl Fn(Rect) -> u16,
    ) {
        let rect = l.rect(Region::FleetList).unwrap();
        let composed = crate::panels::fleet::render::compose(ws, rect).unwrap();
        let index = composed.slots.iter().position(want).unwrap() - composed.offset;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: column(composed.body),
            row: composed.body.y + u16::try_from(index).unwrap(),
            modifiers: KeyModifiers::NONE,
        };
        on_mouse(ws, svc, l, click);
    }

    #[test]
    fn 低い窓では変更の区画を出さず一覧が全部取る() {
        let (_, _, low) = fleet_listing(Vec::new(), 22);
        assert_eq!(low.rect(Region::FleetChanges), None);
        assert_eq!(low.rect(Region::FleetList).unwrap().height, low.main.height);
        let (_, _, tall) = fleet_listing(Vec::new(), 40);
        let list = tall.rect(Region::FleetList).unwrap();
        let changes = tall.rect(Region::FleetChanges).unwrap();
        assert_eq!(list.height + changes.height, tall.main.height);
        assert_eq!(changes.y, list.bottom());
    }

    #[test]
    fn 変更の区画で縮んだ一覧でも_plus_は押した行の起動メニューを開く() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_listing(Vec::new(), 24);
        assert!(l.rect(Region::FleetChanges).is_some());
        click_button(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Worktree(w) if w.label == "feature/b"),
        );
        assert!(matches!(ws.modals.last(), Some(Modal::Popup(_))));
    }

    #[test]
    fn 変更の区画を押してもキーボードは動かない() {
        for focus in [Focus::Fleet, Focus::FleetSession] {
            let (mut ws, mut svc, l) = fleet_with_tree();
            ws.focus = focus;
            let rect = l.rect(Region::FleetChanges).unwrap();
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 3,
                row: rect.y + 2,
                modifiers: KeyModifiers::NONE,
            };
            on_mouse(&mut ws, &mut svc, &l, click);
            assert_eq!(ws.focus, focus);
        }
    }

    fn drawn_changes(ws: &mut Workspace, l: &Layout) -> String {
        ws.sync_layout(l);
        let rect = l.rect(Region::FleetChanges).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(l.area.width, l.area.height)).unwrap();
        terminal
            .draw(|frame| crate::panels::fleet::render::lower(frame, rect, ws))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn 選び直した行の結果が届くまで前の行の一覧を出さず_loading_を出す() {
        use conductor_core::diff_state::{DiffSource, DiffState};
        let (mut ws, _svc, l) = fleet_with_tree();
        let mut diff = DiffState::new(DiffSource::working_tree("main"));
        diff.error = Some("app-main-only".into());
        ws.accept(TaskResult::FleetChanges {
            dir: PathBuf::from("/r/app"),
            diff: Box::new(diff),
            status: Default::default(),
        });
        assert!(drawn_changes(&mut ws, &l).contains("app-main-only"));
        ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::NavigateDown);
        assert_eq!(
            ws.panels.fleet.selected_dir(),
            Some(PathBuf::from("/r/app-b"))
        );
        let after = drawn_changes(&mut ws, &l);
        assert!(!after.contains("app-main-only"), "{after}");
        assert!(after.contains("loading"), "{after}");
        assert_eq!(
            crate::panels::fleet::render::lower_title(&ws),
            " Git Changes "
        );
    }

    #[test]
    fn 一覧を閉じている間は変更を読まない() {
        let (mut ws, mut svc, _) = fleet_with_tree();
        ws.chrome.fleet_open = false;
        poll_fleet_changes(&mut ws, &mut svc, Instant::now());
        assert_eq!(svc.in_flight(), 0);
        ws.chrome.fleet_open = true;
        poll_fleet_changes(&mut ws, &mut svc, Instant::now());
        assert_eq!(svc.in_flight(), 1);
    }

    #[test]
    fn 投げ中に重ねて変更を読まない() {
        let (mut ws, mut svc, _) = fleet_with_tree();
        poll_fleet_changes(&mut ws, &mut svc, Instant::now());
        poll_fleet_changes(&mut ws, &mut svc, Instant::now());
        assert_eq!(svc.in_flight(), 1, "投げ中に 2 本目を投げた");
    }

    #[test]
    fn 据えた結果のファイルと増減が区画と題に出る() {
        use conductor_core::diff_state::{DiffSource, DiffState, FileDiff, FileStatus};
        let (mut ws, _svc, l) = fleet_with_tree();
        let mut diff = DiffState::new(DiffSource::working_tree("main"));
        diff.files.push(FileDiff {
            path: "src/lib.rs".into(),
            status: FileStatus::Modified,
            added_lines: 7,
            deleted_lines: 2,
            hunks: Vec::new(),
        });
        diff.rebuild_display_list();
        ws.accept(TaskResult::FleetChanges {
            dir: PathBuf::from("/r/app"),
            diff: Box::new(diff),
            status: Default::default(),
        });
        let drawn = drawn_changes(&mut ws, &l);
        assert!(drawn.contains("lib.rs"), "{drawn}");
        assert!(drawn.contains("+7"), "{drawn}");
        assert_eq!(
            crate::panels::fleet::render::lower_title(&ws),
            " Git Changes (1) +7 -2 "
        );
    }

    #[test]
    fn 選択が戻った後に届いた前の行の結果は戻った行の一覧を消さない() {
        use conductor_core::diff_state::{DiffSource, DiffState};
        use conductor_core::keymap::Action;
        let land = |ws: &mut Workspace, dir: &str, marker: &str| {
            let mut diff = DiffState::new(DiffSource::working_tree("main"));
            diff.error = Some(marker.into());
            let effects = ws.accept(TaskResult::FleetChanges {
                dir: PathBuf::from(dir),
                diff: Box::new(diff),
                status: Default::default(),
            });
            assert!(effects.is_empty(), "読み直すたびに帯へ出る: {effects:?}");
        };
        let (mut ws, _svc, l) = fleet_with_tree();
        ws.dispatch_to(Focus::Fleet, Action::NavigateDown);
        land(&mut ws, "/r/app-b", "b-only");
        ws.dispatch_to(Focus::Fleet, Action::NavigateUp);
        ws.dispatch_to(Focus::Fleet, Action::NavigateDown);
        land(&mut ws, "/r/app", "a-only");
        let drawn = drawn_changes(&mut ws, &l);
        assert!(drawn.contains("b-only"), "{drawn}");
    }

    struct ShellFleet {
        ws: Workspace,
        svc: Services<TaskResult>,
        l: Layout,
        _tmp: tempfile::TempDir,
        main: PathBuf,
        branch: PathBuf,
    }

    /// 一覧の木は worker が canonical にして返すので、木の dir も canonical にしておく。
    fn shell_fleet(height: u16) -> ShellFleet {
        use conductor_core::git_engine::{Head, WorktreeHead};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let (main, branch) = (root.join("app"), root.join("app-b"));
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&branch).unwrap();
        let mut config = conductor_core::config::Config::default();
        config.general.shell = "/bin/sh".into();
        let mut ws = Workspace::for_test_with(config);
        let mut svc = Services::new();
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let worktree = |path: &PathBuf, branch: &str| WorktreeHead {
            path: path.clone(),
            head: Head::Branch(branch.into()),
        };
        ws.apply_fleet(
            crate::panels::fleet::FleetStatus {
                trees: vec![crate::panels::fleet::RepoTree {
                    root: main.clone(),
                    name: "app".into(),
                    worktrees: vec![worktree(&main, "main"), worktree(&branch, "feature/b")],
                }],
                ..Default::default()
            },
            Vec::new(),
        );
        let l = layout(&ws, Rect::new(0, 0, 160, height));
        ws.sync_layout(&l);
        ShellFleet {
            ws,
            svc,
            l,
            _tmp: tmp,
            main,
            branch,
        }
    }

    fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn ctrl_t() -> KeyEvent {
        chord(KeyCode::Char('t'), KeyModifiers::CONTROL)
    }

    fn ctrl_g() -> KeyEvent {
        chord(KeyCode::Char('g'), KeyModifiers::CONTROL)
    }

    fn shells_in(ws: &Workspace, dir: &std::path::Path) -> usize {
        ws.panels
            .terminal
            .own_ptys()
            .iter()
            .filter(|p| p.kind == SessionKind::Shell && p.working_dir == dir)
            .count()
    }

    #[test]
    fn 一覧の_ctrl_t_は選んだ_worktree_でシェルを起こして入り本画面の_worktree_は動かさない() {
        let mut f = shell_fleet(40);
        f.ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::NavigateDown);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &f.branch), 1);
        assert_eq!(f.ws.focus, Focus::FleetShell);
        assert!(f.ws.fleet_shell().is_some());
        assert_eq!(f.ws.pty_session(), f.ws.fleet_shell());
        assert_eq!(
            f.ws.panels.terminal.session_in(Focus::TerminalShell),
            None,
            "本画面のシェル区画に別の worktree のシェルが出た"
        );
    }

    #[test]
    fn ctrl_t_は何度押してもシェル1本に入る() {
        let mut f = shell_fleet(40);
        for _ in 0..2 {
            on_key(&mut f.ws, &mut f.svc, ctrl_t());
            assert_eq!(f.ws.focus, Focus::FleetShell);
            on_key(
                &mut f.ws,
                &mut f.svc,
                chord(KeyCode::Esc, KeyModifiers::CONTROL),
            );
            assert_eq!(f.ws.focus, Focus::Fleet);
            assert_eq!(
                f.ws.panels.fleet.lower(),
                crate::panels::fleet::Lower::Shell,
                "一覧へ戻っても左下はシェルのまま"
            );
        }
        assert_eq!(shells_in(&f.ws, &f.main), 1);
    }

    #[test]
    fn シェルの中の文字とctrl_tはシェルへ届く() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        for k in [
            key(KeyCode::Char('a')),
            key(KeyCode::Esc),
            key(KeyCode::Tab),
            ctrl_t(),
        ] {
            assert!(
                matches!(route(&mut f.ws, k), Routed::ForwardToPty(_)),
                "{k:?}"
            );
        }
    }

    #[test]
    fn ctrl_g_はシェルの中からでも一覧からでも左下を_git_changes_にして一覧へ戻る() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        for _ in 0..2 {
            on_key(&mut f.ws, &mut f.svc, ctrl_g());
            assert_eq!(f.ws.focus, Focus::Fleet);
            assert_eq!(
                f.ws.panels.fleet.lower(),
                crate::panels::fleet::Lower::Changes
            );
            assert!(f.ws.modals.is_empty(), "一覧の ctrl+g が全文検索を開いた");
        }
        assert_eq!(
            shells_in(&f.ws, &f.main),
            1,
            "Git Changes へ戻すとシェルが止まった"
        );
    }

    #[test]
    fn シェルの中の_ctrl_g_はファイルを開きに行かない() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        let effects =
            f.ws.dispatch(conductor_core::keymap::Action::OpenFileFromTerminal)
                .unwrap();
        assert!(
            matches!(effects.as_slice(), [Effect::Focus(Focus::Fleet)]),
            "{effects:?}"
        );
    }

    #[test]
    fn シェルの中のスクロールは右の_claude_の読みビューを開かない() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        for action in [
            conductor_core::keymap::Action::ScrollbackUp,
            conductor_core::keymap::Action::ScrollbackTop,
        ] {
            assert_eq!(f.ws.dispatch(action), Some(Vec::new()));
        }
        assert_eq!(f.ws.focus, Focus::FleetShell);
    }

    #[test]
    fn シェルから一覧を開き直して閉じても見えないシェルに入らない() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        apply(&mut f.ws, &mut f.svc, vec![Effect::OpenFleet]);
        let effects =
            f.ws.dispatch(conductor_core::keymap::Action::ExitSubPanel)
                .unwrap();
        apply(&mut f.ws, &mut f.svc, effects);
        assert!(!f.ws.chrome.fleet_open);
        assert_eq!(f.ws.focus, Focus::Explorer);
        assert_eq!(f.ws.pty_session(), None);
    }

    #[test]
    fn 打っていたシェルが閉じたら同じ_dir_の別のシェルへは打たせず一覧へ戻す() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        let first = f.ws.pty_session().unwrap();
        let config = f.ws.config.clone();
        f.ws.panels
            .terminal
            .launch_shell_in(&f.main, &config)
            .unwrap();
        f.ws.panels.terminal.close_sessions(&[first]);
        assert_eq!(f.ws.pty_session(), None);
        f.ws.sync_layout(&f.l);
        assert_eq!(f.ws.focus, Focus::Fleet);
    }

    #[test]
    fn 打っている間に読み直しで選んだ行が動いたら一覧へ戻す() {
        use conductor_core::git_engine::{Head, WorktreeHead};
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        f.ws.apply_fleet(
            crate::panels::fleet::FleetStatus {
                trees: vec![crate::panels::fleet::RepoTree {
                    root: f.branch.clone(),
                    name: "other".into(),
                    worktrees: vec![WorktreeHead {
                        path: f.branch.clone(),
                        head: Head::Branch("feature/b".into()),
                    }],
                }],
                ..Default::default()
            },
            Vec::new(),
        );
        assert_eq!(f.ws.panels.fleet.selected_dir(), Some(f.branch.clone()));
        f.ws.sync_layout(&f.l);
        assert_eq!(f.ws.focus, Focus::Fleet);
    }

    #[test]
    fn 窓を縮めて左下が消えたら一覧へ戻り_ctrl_t_は起こさない() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        let low = layout(&f.ws, Rect::new(0, 0, 160, 22));
        f.ws.sync_layout(&low);
        assert_eq!(f.ws.focus, Focus::Fleet);
        f.ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::NavigateDown);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &f.branch), 0);
        assert_eq!(f.ws.focus, Focus::Fleet);
        assert!(matches!(
            f.ws.chrome.status.as_ref().map(|s| s.level),
            Some(StatusLevel::Warning)
        ));
    }

    #[test]
    fn 消す点検中の_worktree_にはシェルを起こさない() {
        let mut f = shell_fleet(40);
        f.ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::NavigateDown);
        f.ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::StopSession);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &f.branch), 0);
        assert_eq!(f.ws.focus, Focus::Fleet);
    }

    fn branch_inspection(f: &ShellFleet) -> crate::panels::fleet::Inspection {
        crate::panels::fleet::Inspection {
            repo: f.main.clone(),
            path: f.branch.clone(),
            label: "feature/b".into(),
            check: conductor_core::git_engine::DeletionCheck {
                recorded: f.branch.clone(),
                branch: Some("feature/b".into()),
                keeps_branch: false,
                head: None,
                base: None,
                unmerged: Some(0),
                dirty: Some(0),
            },
            claudes: Vec::new(),
            panels: Vec::new(),
            own_claudes: 0,
            shells: 0,
            in_window_repo: false,
        }
    }

    #[test]
    fn 削除を確定した_worktree_には消えるまでシェルを起こさない() {
        let mut f = shell_fleet(40);
        f.ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::NavigateDown);
        let inspection = branch_inspection(&f);
        // 消す worker は走らせない。結果が届くまでの区間だけを見る。
        crate::effect::delete_fleet_worktree(&mut f.ws, inspection);
        f.ws.panels.fleet.forget_in_flight();
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &f.branch), 0);
        assert_eq!(f.ws.focus, Focus::Fleet);
        f.ws.accept(TaskResult::FleetWorktreeDeleted {
            path: f.branch.clone(),
            in_window_repo: false,
            outcome: Err("failed".into()),
        });
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &f.branch), 1, "消せなかった後も起こせない");
    }

    #[test]
    fn exit_したシェルは片付く前でも映さず_ctrl_t_で新しく起こす() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        let first = f.ws.pty_session().unwrap();
        for c in "exit\r".chars() {
            let code = if c == '\r' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            };
            f.ws.panels.terminal.forward_key(key(code), &first);
        }
        crate::testing::wait_for(|| f.ws.fleet_shell().is_none());
        assert!(
            f.ws.panels.terminal.has_session(&first),
            "片付いた後を見ている"
        );
        f.ws.sync_layout(&f.l);
        assert_eq!(f.ws.focus, Focus::Fleet);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        let second = f.ws.pty_session().unwrap();
        assert_ne!(second, first);
    }

    #[test]
    fn 木に載らない外の端末の行にはシェルを起こさない() {
        use conductor_core::claude_sessions::registry::{LiveSession, Owner, Status};
        let mut f = shell_fleet(40);
        let outside = f.main.join("sub");
        std::fs::create_dir_all(&outside).unwrap();
        f.ws.apply_fleet(
            crate::panels::fleet::FleetStatus {
                live: vec![LiveSession {
                    pid: 9,
                    cwd: outside.clone(),
                    session_id: "s9".into(),
                    status: Status::Idle,
                    status_since: None,
                    owner: Owner::Elsewhere,
                    started: std::time::SystemTime::UNIX_EPOCH,
                }],
                ..Default::default()
            },
            Vec::new(),
        );
        assert_eq!(f.ws.panels.fleet.selected_dir(), Some(outside.clone()));
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &outside), 0);
        assert_eq!(f.ws.focus, Focus::Fleet);
        assert_eq!(
            f.ws.panels.fleet.lower(),
            crate::panels::fleet::Lower::Changes
        );
    }

    #[test]
    fn 右の_claude_に入力中の_ctrl_g_は左下もフォーカスも変えない() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        f.ws.focus = Focus::FleetSession;
        let effects =
            f.ws.dispatch(conductor_core::keymap::Action::OpenFileFromTerminal)
                .unwrap_or_default();
        apply(&mut f.ws, &mut f.svc, effects);
        assert_eq!(f.ws.focus, Focus::FleetSession);
        assert_eq!(
            f.ws.panels.fleet.lower(),
            crate::panels::fleet::Lower::Shell
        );
    }

    #[test]
    fn 消えた_worktree_にはシェルを起こさずモードもフォーカスも変えない() {
        let (mut ws, mut svc, l) = fleet_with_tree();
        ws.sync_layout(&l);
        on_key(&mut ws, &mut svc, ctrl_t());
        assert_eq!(ws.panels.terminal.own_ptys().len(), 0);
        assert_eq!(ws.focus, Focus::Fleet);
        assert_eq!(
            ws.panels.fleet.lower(),
            crate::panels::fleet::Lower::Changes
        );
        assert!(matches!(
            ws.chrome.status.as_ref().map(|s| s.level),
            Some(StatusLevel::Warning)
        ));
    }

    #[test]
    fn 本画面と同じ_worktree_に起こしたシェルは綴りを揃えて本画面のシェル区画にも映る() {
        let mut f = shell_fleet(40);
        let spelled = f._tmp.path().join("app");
        assert_ne!(
            spelled, f.main,
            "tempdir が canonical だと綴りの差を確かめられない"
        );
        f.ws.panels.terminal.follow_worktree(Some(spelled.clone()));
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(shells_in(&f.ws, &spelled), 1);
        assert_eq!(
            f.ws.panels.terminal.session_in(Focus::TerminalShell),
            f.ws.fleet_shell()
        );
        on_key(
            &mut f.ws,
            &mut f.svc,
            chord(KeyCode::Esc, KeyModifiers::CONTROL),
        );
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        assert_eq!(
            shells_in(&f.ws, &spelled),
            1,
            "綴りの違いで 2 本目を起こした"
        );
    }

    #[test]
    fn 左下のシェルを押すと入り_シェルが無ければ入らない() {
        let mut f = shell_fleet(40);
        let rect = f.l.rect(Region::FleetChanges).unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 3,
            row: rect.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        on_key(&mut f.ws, &mut f.svc, ctrl_g());
        on_mouse(&mut f.ws, &mut f.svc, &f.l, click);
        assert_eq!(f.ws.focus, Focus::Fleet, "Git Changes を押して入った");
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        on_key(
            &mut f.ws,
            &mut f.svc,
            chord(KeyCode::Esc, KeyModifiers::CONTROL),
        );
        on_mouse(&mut f.ws, &mut f.svc, &f.l, click);
        assert_eq!(f.ws.focus, Focus::FleetShell);
    }

    #[test]
    fn 左下のシェルに落としたファイルはシェルへ貼る() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        f.ws.chrome.status = None;
        let dropped = f.main.join("dropped.png");
        std::fs::write(&dropped, b"").unwrap();
        on_paste(&mut f.ws, &mut f.svc, format!("{} ", dropped.display()));
        assert!(f.ws.chrome.status.is_none(), "{:?}", f.ws.chrome.status);
        assert_eq!(f.ws.focus, Focus::FleetShell);
    }

    #[test]
    fn 一覧を閉じたら左下のシェルは打ち先にならない() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        f.ws.chrome.fleet_open = false;
        assert_eq!(f.ws.fleet_shell(), None);
        assert_eq!(f.ws.pty_session(), None);
    }

    #[test]
    fn 左下の題と案内はフォーカスとシェルの有無で変わる() {
        let mut f = shell_fleet(40);
        on_key(&mut f.ws, &mut f.svc, ctrl_t());
        let title = crate::panels::fleet::render::lower_title(&f.ws);
        assert!(
            title.contains("入力中") && title.contains("ctrl+g"),
            "{title}"
        );
        on_key(
            &mut f.ws,
            &mut f.svc,
            chord(KeyCode::Esc, KeyModifiers::CONTROL),
        );
        let title = crate::panels::fleet::render::lower_title(&f.ws);
        assert!(title.contains("ctrl+t で入る"), "{title}");
        f.ws.dispatch_to(Focus::Fleet, conductor_core::keymap::Action::NavigateDown);
        let drawn = drawn_changes(&mut f.ws, &f.l);
        assert!(drawn.contains("シェルなし"), "{drawn}");
    }

    fn click_button(
        ws: &mut Workspace,
        svc: &mut Services<TaskResult>,
        l: &Layout,
        want: impl Fn(&crate::panels::fleet::render::Slot) -> bool,
    ) {
        click_line(ws, svc, l, want, |body| body.right() - 1);
    }

    fn click_text(
        ws: &mut Workspace,
        svc: &mut Services<TaskResult>,
        l: &Layout,
        want: impl Fn(&crate::panels::fleet::render::Slot) -> bool,
    ) {
        click_line(ws, svc, l, want, |body| body.x + 1);
    }

    fn fleet_with_sessions(height: u16) -> (Workspace, Services<TaskResult>, Layout) {
        use conductor_core::claude_sessions::registry::{LiveSession, Owner, Status, Waiting};
        let session = |pid: u32, status: Status| LiveSession {
            pid,
            cwd: PathBuf::from("/r/app"),
            session_id: format!("s{pid}"),
            status,
            status_since: None,
            owner: Owner::Conductor,
            started: std::time::SystemTime::UNIX_EPOCH,
        };
        let mut live = vec![session(7, Status::Waiting(Waiting::Question))];
        live.extend((8..20).map(|pid| session(pid, Status::Working)));
        fleet_listing(live, height)
    }

    fn selected_key(ws: &Workspace) -> Option<(crate::panels::fleet::RowKey, bool)> {
        ws.panels.fleet.selected_row().map(|r| (r.key, r.pinned))
    }

    #[test]
    fn worktree_の行の文字を押すとその_worktree_が選ばれる() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_tree();
        assert_eq!(ws.panels.fleet.selected_worktree().unwrap().label, "main");
        click_text(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Worktree(w) if w.label == "feature/b"),
        );
        assert_eq!(
            ws.panels.fleet.selected_worktree().unwrap().label,
            "feature/b"
        );
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn セッションの行を押すとそのセッションが選ばれ_入力待ちの箱は枠を押しても選ばれる() {
        use crate::panels::fleet::RowKey;
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_sessions(60);
        assert_ne!(selected_key(&ws), Some((RowKey::Pid(9), false)));
        click_text(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Session(r) if r.key == RowKey::Pid(9) && !r.pinned),
        );
        assert_eq!(selected_key(&ws), Some((RowKey::Pid(9), false)));

        // 箱の最初の行は上の枠線。
        click_text(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Session(r) if r.pinned),
        );
        assert_eq!(selected_key(&ws), Some((RowKey::Pid(7), true)));
    }

    #[test]
    fn 見出しと区画の枠を押しても選択は動かない() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_sessions(60);
        let before = selected_key(&ws);
        assert!(before.is_some());
        click_text(&mut ws, &mut svc, &l, |slot| matches!(slot, Slot::Repo(_)));
        click_text(&mut ws, &mut svc, &l, |slot| matches!(slot, Slot::Other));
        click_line(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Session(r) if !r.pinned),
            |body| body.x - 1,
        );
        assert_eq!(selected_key(&ws), before);
    }

    #[test]
    fn 確認が開いている間は行を押しても選択は動かない() {
        use crate::panels::fleet::RowKey;
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_sessions(60);
        let before = selected_key(&ws);
        assert_ne!(before, Some((RowKey::Pid(9), false)));
        ws.modals
            .push(Modal::Confirm(crate::modal::Confirm::ask("x", Vec::new())));
        click_text(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Session(r) if r.key == RowKey::Pid(9)),
        );
        assert_eq!(selected_key(&ws), before);
    }

    #[test]
    fn 右で打っている間に一覧の行を押すと選択が動き一覧へ戻る() {
        use crate::panels::fleet::RowKey;
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_sessions(60);
        ws.focus = Focus::FleetSession;
        click_text(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Session(r) if r.key == RowKey::Pid(9) && !r.pinned),
        );
        assert_eq!(selected_key(&ws), Some((RowKey::Pid(9), false)));
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn 下までスクロールした一覧でも押した見た目の行が選ばれる() {
        use crate::panels::fleet::Item;
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_sessions(20);
        let bottom = ws
            .panels
            .fleet
            .items()
            .into_iter()
            .filter_map(|item| match item {
                Item::Session(row) => Some(row),
                Item::Worktree(_) => None,
            })
            .next_back()
            .unwrap();
        let bottom_key = bottom.key.clone();
        ws.panels.fleet.click_list(
            (1, 0),
            crate::panels::fleet::ListPress {
                hit: Some(&Slot::Session(bottom)),
                button: None,
                in_button_columns: false,
            },
        );
        let rect = l.rect(Region::FleetList).unwrap();
        let composed = crate::panels::fleet::render::compose(&ws, rect).unwrap();
        assert!(composed.offset > 0, "スクロールしていること");
        let (line, row) = composed
            .slots
            .iter()
            .enumerate()
            .skip(composed.offset)
            .find_map(|(i, slot)| match slot {
                Slot::Session(r) if r.key != bottom_key => Some((i, r.clone())),
                _ => None,
            })
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: composed.body.x + 1,
            row: composed.body.y + u16::try_from(line - composed.offset).unwrap(),
            modifiers: KeyModifiers::NONE,
        };
        assert!(click.row < composed.body.bottom(), "見えている行を押すこと");
        on_mouse(&mut ws, &mut svc, &l, click);
        assert!(ws.panels.fleet.is_selected(&row));
        let after = crate::panels::fleet::render::compose(&ws, rect).unwrap();
        assert_eq!(
            after.offset,
            line.saturating_sub(usize::from(composed.body.height) - 2),
            "表示位置は j / k と同じく選んだ行から決まる"
        );
    }

    #[test]
    fn 一度目で一覧が送られ二度目の下にボタンが来てもボタンは開かない() {
        use crate::panels::fleet::render::{Slot, compose, slot_at};
        let mut slid_under = 0;
        for (height, y_off) in (16..64).flat_map(|h| (0..h).map(move |y| (h, y))) {
            let (mut ws, mut svc, l) = fleet_with_sessions(height);
            let rect = l.rect(Region::FleetList).unwrap();
            let body = compose(&ws, rect).unwrap().body;
            let (x, y) = (body.right() - 1, body.y + y_off);
            if y >= body.bottom() {
                continue;
            }
            let press = |ws: &mut Workspace, svc: &mut Services<TaskResult>| {
                let click = |kind| MouseEvent {
                    kind,
                    column: x,
                    row: y,
                    modifiers: KeyModifiers::NONE,
                };
                on_mouse(ws, svc, &l, click(MouseEventKind::Down(MouseButton::Left)));
                on_mouse(ws, svc, &l, click(MouseEventKind::Up(MouseButton::Left)));
            };
            let first = compose(&ws, rect).unwrap();
            if !matches!(slot_at(&first, x, y), Some(Slot::Session(_))) {
                continue;
            }
            press(&mut ws, &mut svc);
            let second = compose(&ws, rect).unwrap();
            if matches!(
                slot_at(&second, x, y),
                Some(Slot::Worktree(_) | Slot::Repo(Some(_)))
            ) {
                slid_under += 1;
            }
            press(&mut ws, &mut svc);
            assert!(ws.modals.is_empty(), "y {y}: {:?}", ws.modals.len());
        }
        assert!(slid_under > 0, "二度目の下にボタンが来る行があること");
    }

    #[test]
    fn worktree_の文字を押した直後に同じ行の_plus_を押しても起動メニューが開く() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_tree();
        let feature_b = |slot: &Slot| matches!(slot, Slot::Worktree(w) if w.label == "feature/b");
        click_text(&mut ws, &mut svc, &l, feature_b);
        ws.focus = Focus::FleetSession;
        click_button(&mut ws, &mut svc, &l, feature_b);
        assert!(matches!(ws.modals.last(), Some(Modal::Popup(_))));
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn 描いた行ごとにその行の当たりがある() {
        use crate::panels::fleet::RowKey;
        use crate::panels::fleet::render::Slot;
        let (ws, _, l) = fleet_with_sessions(60);
        let composed =
            crate::panels::fleet::render::compose(&ws, l.rect(Region::FleetList).unwrap()).unwrap();
        assert_eq!(composed.lines.len(), composed.slots.len());
        let text = |i: usize| composed.lines[i].to_string();
        for (i, slot) in composed.slots.iter().enumerate() {
            match slot {
                Slot::Other => assert!(text(i).starts_with("入力待ち"), "{i}: {}", text(i)),
                Slot::Repo(_) => assert!(text(i).starts_with("app"), "{i}: {}", text(i)),
                Slot::Worktree(w) => assert!(text(i).contains(&w.label), "{i}: {}", text(i)),
                Slot::Session(r) if r.pinned => {
                    assert_eq!(r.key, RowKey::Pid(7));
                    assert!(text(i).starts_with(['┌', '│', '└']), "{i}: {}", text(i));
                }
                Slot::Session(_) => assert!(!text(i).starts_with(['┌', '│', '└'])),
            }
        }
    }

    #[test]
    fn 本文の上と下の外を押しても選択は動かない() {
        let (mut ws, mut svc, l) = fleet_with_sessions(20);
        let before = selected_key(&ws);
        let rect = l.rect(Region::FleetList).unwrap();
        let body = crate::panels::fleet::render::compose(&ws, rect)
            .unwrap()
            .body;
        for row in [body.y - 1, body.bottom()] {
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: body.x + 1,
                row,
                modifiers: KeyModifiers::NONE,
            };
            on_mouse(&mut ws, &mut svc, &l, click);
            assert_eq!(selected_key(&ws), before, "row {row}");
        }
    }

    #[test]
    fn セッション行の右端を押しても選ぶ() {
        use crate::panels::fleet::RowKey;
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_sessions(60);
        assert_ne!(selected_key(&ws), Some((RowKey::Pid(9), false)));
        click_button(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Session(r) if r.key == RowKey::Pid(9) && !r.pinned),
        );
        assert_eq!(selected_key(&ws), Some((RowKey::Pid(9), false)));
        assert!(ws.modals.is_empty());
    }

    #[test]
    fn worktree_の_plus_を押すと押した行の起動メニューが開き_選択は動かない() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_tree();
        assert_eq!(ws.panels.fleet.selected_worktree().unwrap().label, "main");
        click_button(
            &mut ws,
            &mut svc,
            &l,
            |slot| matches!(slot, Slot::Worktree(w) if w.label == "feature/b"),
        );
        let Some(Modal::Popup(mut popup)) = ws.modals.pop() else {
            panic!("起動メニューが開く");
        };
        let one = popup.update(
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
            &ws.ctx(),
        );
        assert!(matches!(
            one.last(),
            Some(Effect::NewClaudeIn { dir, chrome: false }) if dir == &PathBuf::from("/r/app-b")
        ));
        assert_eq!(ws.panels.fleet.selected_worktree().unwrap().label, "main");
    }

    #[test]
    fn repo_の印を押すと_smart_worktree_の説明を聞く() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_tree();
        click_button(&mut ws, &mut svc, &l, |slot| {
            matches!(slot, Slot::Repo(Some(_)))
        });
        assert!(matches!(ws.modals.last(), Some(Modal::Prompt(_))));
        assert_eq!(
            ws.panels.fleet.take_smart_target(),
            Some(PathBuf::from("/r/app"))
        );
    }

    #[test]
    fn 確認が開いている間はボタンを押しても何も積まない() {
        use crate::panels::fleet::render::Slot;
        let (mut ws, mut svc, l) = fleet_with_tree();
        ws.modals
            .push(Modal::Confirm(crate::modal::Confirm::ask("x", Vec::new())));
        click_button(&mut ws, &mut svc, &l, |slot| {
            matches!(slot, Slot::Worktree(_))
        });
        assert_eq!(ws.modals.len(), 1);
    }
}
