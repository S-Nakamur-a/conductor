//! Main event loop: the draw → poll → handle cycle, from process start until
//! the user quits.
//!
//! ループ 1 周分の各フェーズは [`phases`]、反復をまたぐ状態とその初期化は
//! [`state`] にある。定期タイマーと外部イベント源のポーリングは
//! [`crate::event_loop_timers`]。ここに残っているのは周回そのものだけ。

mod phases;
mod state;

use std::io;
use std::time::Duration;

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use state::LoopState;

/// Tick rate when terminal panels are focused (~120fps for responsive PTY).
const TICK_RATE_TERMINAL: Duration = Duration::from_millis(8);
/// Tick rate right after user input for responsive scrolling (~60fps).
const TICK_RATE_ACTIVE: Duration = Duration::from_millis(16);
/// Tick rate when non-terminal panels are idle (low CPU usage).
const TICK_RATE_IDLE: Duration = Duration::from_millis(500);
/// How long to keep using the active tick rate after the last input event.
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
/// Fixed interval for decoration animation updates (~10fps), independent of main tick rate.
const DECORATION_TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Interval for the "Claude is waiting" notification breathing pulse (~12fps).
/// Drives redraws while a session waits, independent of decoration/PTY activity,
/// so the pulse keeps breathing even when the user is focused elsewhere.
const PULSE_TICK_INTERVAL: Duration = Duration::from_millis(80);
/// Interval for refreshing unfocused terminal panels (~2fps).
/// Balances visibility of background PTY output with CPU usage.
const UNFOCUSED_TERMINAL_REFRESH: Duration = Duration::from_millis(500);
/// Redraw cadence for rich-mode gradient borders (~30fps). The rotating focus
/// gradient and waiting glow (`ui::rich`) derive their phase from wall-clock
/// time but only advance when the frame is redrawn; without a dedicated cadence
/// the gradient stutters at the idle/decoration tick rate. Only armed in rich
/// mode (and never overriding the faster terminal/active rates), so the cost is
/// a steady 30fps repaint while rich effects are visible.
const RICH_REFRESH_INTERVAL: Duration = Duration::from_millis(33);

/// Paths the file watcher should monitor: every worktree's path, or — when
/// there are no worktrees (e.g. a plain non-git directory) — the repo path
/// itself, so the Explorer still auto-refreshes on file changes there.
pub(crate) fn watch_paths_for(app: &App) -> Vec<std::path::PathBuf> {
    if app.worktrees.is_empty() {
        vec![app.repo.path.clone()]
    } else {
        app.worktrees.iter().map(|w| w.path.clone()).collect()
    }
}

/// Drive the draw → poll → handle cycle until the user quits.
///
/// 1 周の並びは「入力からピクセルまで」の遅延で決まっている: イベントを待って
/// 溜まった分をまとめて捌き、すぐ描き、遅くてよい仕事は最後に回す。
pub(crate) fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut loop_state = LoopState::setup(app);

    loop {
        let signals = phases::FrameSignals::take(app);
        let tick = phases::next_tick(app, &loop_state, &signals);
        phases::drain_events(app, &mut loop_state, tick)?;

        // 埋め込みエディタは TUI のサスペンドではなくパネル内 PTY として動くので、
        // その終了は毎周ここで検出する: 子プロセスが消えていたらパネルを畳み、
        // Explorer/Viewer のレイアウトを戻し、編集されたファイルを読み直す。
        if app.poll_editor_exit() {
            app.dirty.mark_all();
        }

        loop_state.note_layout_change(app);
        phases::mark_continuous_dirty(app);
        phases::render_frame(terminal, app, &mut loop_state)?;
        phases::run_background_work(app, &mut loop_state, &signals);

        if app.should_quit {
            // 生成中のウォークスルーはヘッドレスの `claude` 子プロセス。
            // これが無いと、メインループが止まったあと誰もポーリングしないまま
            // 孤児として動き続け (API 課金も続き) てしまう。
            app.shutdown_walkthrough_generation();
            return Ok(());
        }
    }
}
