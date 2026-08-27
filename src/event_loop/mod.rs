//! メインイベントループ: プロセス開始からユーザが終了するまでの
//! draw → poll → handle サイクル。
//!
//! ループ 1 周分の各フェーズは [phases]、反復をまたぐ状態とその初期化は
//! [state] にある。定期タイマーと外部イベント源のポーリングは
//! [crate::event_loop_timers]。ここに残っているのは周回そのものだけ。

mod phases;
mod state;

use std::io;
use std::time::Duration;

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use state::LoopState;

/// terminal パネルにフォーカスしているときの tick 間隔 (PTY の応答性のため ~120fps)。
const TICK_RATE_TERMINAL: Duration = Duration::from_millis(8);
/// ユーザ入力の直後、スクロールを滑らかにするための tick 間隔 (~60fps)。
const TICK_RATE_ACTIVE: Duration = Duration::from_millis(16);
/// terminal 以外のパネルがアイドル状態のときの tick 間隔 (CPU 使用量を抑える)。
const TICK_RATE_IDLE: Duration = Duration::from_millis(500);
/// 最後の入力イベントのあと、active の tick 間隔を使い続ける時間。
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
/// 装飾アニメーション更新の固定間隔 (~10fps)。メインの tick 間隔とは独立している。
const DECORATION_TICK_INTERVAL: Duration = Duration::from_millis(100);
/// 「Claude が待機中」通知の呼吸パルスの間隔 (~12fps)。セッションが待機している
/// 間、装飾や PTY のアクティビティとは独立に再描画を駆動する。これにより
/// ユーザが他の場所にフォーカスしていてもパルスが呼吸し続ける。
const PULSE_TICK_INTERVAL: Duration = Duration::from_millis(80);
/// フォーカスしていない terminal パネルを更新する間隔 (~2fps)。バックグラウンドの
/// PTY 出力の可視性と CPU 使用量のバランスを取る。
const UNFOCUSED_TERMINAL_REFRESH: Duration = Duration::from_millis(500);

/// ユーザが終了するまで draw → poll → handle サイクルを回す。
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
        phases::run_background_work(app, &mut loop_state);

        if app.should_quit {
            // 実行中の解析は revidere の子プロセス。ここで止めないと、メイン
            // ループが終わったあと誰も結果を読まないまま走り続ける。
            app.shutdown_revidere();
            return Ok(());
        }
    }
}
