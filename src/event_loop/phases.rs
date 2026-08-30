//! メインループ 1 周分を構成する各フェーズ。
//!
//! 並び順は「入力からピクセルまで」の遅延を最小にするために決まっている:
//! 待つ → 入力を捌く → 描く → 遅くてよい後始末。

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyEventKind, poll as crossterm_poll, read as crossterm_read};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::state::LoopState;
use super::{
    ACTIVITY_TIMEOUT, DECORATION_TICK_INTERVAL, PULSE_TICK_INTERVAL, TICK_RATE_ACTIVE,
    TICK_RATE_IDLE, TICK_RATE_TERMINAL,
};
use crate::app::App;
use crate::event::{handle_key_event, handle_mouse_event, handle_paste_event};
use crate::ui::layout::render_ui;

/// フレームを描く前にイベントを捌き続ける上限。
///
/// 高速スクロール中の入力飢餓を防ぐ (トラックパッドの慣性は 100 個超の
/// イベントを出しうる)。この予算を超えた分は次の反復に回して、中間フレームが
/// 滑らかに出るようにする。
const MAX_DRAIN: Duration = Duration::from_millis(8);

/// ステータスメッセージが消えるまでのフレーム数。
const STATUS_FADE_TICKS: u64 = 180;

/// 「いま画面が動いている理由」— tick 速度と再描画の要否を決める材料。
///
/// 1 周の頭で 1 度だけ求めて、フェーズ間で使い回す (decoration の設定文字列の
/// パースや PTY の通知フラグの取得は、1 周に 1 回しかやってはいけない)。
pub(super) struct FrameSignals {
    /// 装飾アニメーションが動いているか。
    pub decoration_active: bool,
    /// PTY から新しい出力が届いたか。
    pub pty_dirty: bool,
}

impl FrameSignals {
    /// 1 周の頭で状態を採取し、PTY 出力があれば再描画を要求する。
    pub fn take(app: &mut App) -> Self {
        let decoration_active =
            crate::worktree::decoration::DecorationMode::from_str(&app.config.general.decoration)
                .has_animation();
        let pty_dirty = app.terminal.pty_manager.take_output_notify();

        if pty_dirty {
            app.terminal.claude.dirty = true;
            app.terminal.shell.dirty = true;
            if let Some(editor) = app.editor.as_mut() {
                editor.dirty = true;
            }
            app.request_redraw();
        }

        Self {
            decoration_active,
            pty_dirty,
        }
    }
}

/// 次にイベントを待つ時間を決める。
///
/// 描くものがあるなら待たない。無いときは「いちばん速い理由」を上から選ぶ:
/// ターミナル系のフォーカス > 進行中の操作 > 直前の入力 > フォーカス遷移 >
/// 待機パルス > 装飾 > アイドル。
pub(super) fn next_tick(app: &App, loop_state: &LoopState, signals: &FrameSignals) -> Duration {
    if app.needs_redraw || signals.pty_dirty {
        return Duration::ZERO;
    }
    match app.focus {
        f if f.is_pty() => TICK_RATE_TERMINAL,
        _ if app.update.is_active() => TICK_RATE_ACTIVE,
        _ if !app.worktree_mgr.pending_worktrees.is_empty() => TICK_RATE_ACTIVE,
        _ if app.panel_number_overlay.is_visible() => TICK_RATE_ACTIVE,
        _ if loop_state.last_input_time.elapsed() < ACTIVITY_TIMEOUT => TICK_RATE_ACTIVE,
        // フォーカスのボーダーが遷移中はフレームを流し続ける。
        _ if app.has_active_transition() => TICK_RATE_ACTIVE,
        _ if !app.terminal.cc_waiting_worktrees.is_empty() => PULSE_TICK_INTERVAL,
        // 解析中はストリップのスピナーが回る。数分続くものなので、worktree の
        // 作成 (数秒) と同じ 60fps では回さない。
        _ if !app.revidere.runs.is_empty() => PULSE_TICK_INTERVAL,
        _ if signals.decoration_active => DECORATION_TICK_INTERVAL,
        _ => TICK_RATE_IDLE,
    }
}

/// 溜まっているイベントを [MAX_DRAIN] の予算まで捌く。
///
/// まとめて捌くのは、高速スクロールで 1 イベント 1 フレームにならないようにするため。
pub(super) fn drain_events(
    app: &mut App,
    loop_state: &mut LoopState,
    tick: Duration,
) -> Result<()> {
    if !crossterm_poll(tick)? {
        return Ok(());
    }
    let deadline = Instant::now() + MAX_DRAIN;
    loop {
        let event = crossterm_read()?;
        handle_event(app, loop_state, event);
        app.request_redraw();
        // イベントが尽きたか、1 フレーム分の予算を使い切ったら抜ける。
        if Instant::now() >= deadline || !crossterm_poll(Duration::ZERO)? {
            return Ok(());
        }
    }
}

fn handle_event(app: &mut App, loop_state: &mut LoopState, event: Event) {
    match event {
        // オートリピート (押しっぱなし) は押下と同じに扱う。j/k/上下を押し続けて
        // スクロールやナビゲーションが継続するように。Repeat が届くのは kitty
        // キーボードプロトコル下だけで、無い端末はオートリピートを Press の連続
        // として送ってくる。
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            log::debug!(
                "key: code={:?} mods={:?} kind={:?}",
                key.code,
                key.modifiers,
                key.kind
            );
            loop_state.last_input_time = Instant::now();
            // D7(a): キー入力があった = マウスはもう能動的な入力デバイスではない。
            // crossterm はマウスが端末ウィンドウから出たことを報告しないので、
            // これが無いとアンダーラインや行 / チップ / タブのハイライトが、
            // ユーザーがキーボードに移ったあともずっと点いたままになる。
            //
            // 消すのはポインタ由来のハイライトだけ。ホバー *ポップアップ* は
            // 下の handle_key_event の担当で、pin されたモーダル (キーで操作)
            // と一時的なもの (任意のキーで消え、Esc は握り潰す) を区別するのに
            // スタックが要る。
            app.clear_pointer_hover();
            handle_key_event(app, key);
        }
        Event::Mouse(mouse) => {
            loop_state.last_input_time = Instant::now();
            handle_mouse_event(app, mouse, loop_state.last_frame_area);
        }
        Event::Paste(data) => {
            loop_state.last_input_time = Instant::now();
            handle_paste_event(app, data);
        }
        Event::Resize(_, _) => {
            // ウィンドウのリサイズは全パネルの境界を作り直すので、旧い縁の内容が
            // 残らないようハードクリアする (境界ドラッグと同じ種類のズレ)。
            app.terminal.needs_clear = true;
        }
        // D7(b): crossterm が確実に報告してくれて、かつマウスが我々の描画面から
        // 出たと言い切れる唯一のケース — 端末ウィンドウ自体がフォーカスを失った
        // (ユーザーが alt-tab した等)。
        Event::FocusLost => app.clear_all_hover(),
        _ => {}
    }
}

/// 時間で動くもの・進行中のものがあるあいだ、再描画を要求し続ける。
///
/// どれも「イベントが来ないと止まってしまう」種類のもので、アイドルの tick 速度に
/// 落ちると固まって見える。
pub(super) fn mark_continuous_dirty(app: &mut App) {
    let overlay_animating = app.update.is_active()
        || app.overlays.grep_search.running
        || app.overlays.grep_search.debounce_deadline.is_some()
        || app.panel_number_overlay.is_visible()
        // フォーカスのボーダー遷移は時間ベース。
        || app.has_active_transition()
        // ハードクリアが予約されていれば全面再描画になる。
        || app.terminal.needs_clear;
    if overlay_animating {
        app.request_redraw();
    }
    // worktree 作成中と revidere の解析中は、どちらもストリップの上で
    // スピナーが回る。イベントが来ないと止まって見える。
    if !app.worktree_mgr.pending_worktrees.is_empty() || !app.revidere.runs.is_empty() {
        app.request_redraw();
    }
    // リフローのスイープ演出: 進行中は毎フレーム再描画を要求する。
    // (focus==TerminalClaude なので tick は既に 8ms、追加の起床要因は不要)
    if app.reflow.sweep.is_some() {
        app.request_redraw();
    }
}

/// フレームを 1 枚、不可分に描く。
///
/// 同期出力 (端末モード 2026) で clear + draw を囲み、端末が 1 ショットで
/// 反映するようにしている。無いと、8ms 間隔の連続フレーム (スクロールバック中)
/// が途中まで適用されて実画面と ratatui のセルバッファがズレ、以降そのセルが
/// 二度と再描画されない (スクロールバックの「にじみ」)。この機能が無い端末は
/// マーカーを無視するだけ。
pub(super) fn render_frame(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    loop_state: &mut LoopState,
) -> Result<()> {
    if !app.terminal.needs_clear && !app.needs_redraw {
        return Ok(());
    }
    let _ = execute!(io::stdout(), BeginSynchronizedUpdate);

    if app.terminal.needs_clear {
        terminal.clear()?;
        app.terminal.needs_clear = false;
    }
    if app.needs_redraw {
        app.ui_tick = app.ui_tick.wrapping_add(1);
        expire_status_message(app);
        app.panel_number_overlay.expire_if_due();

        terminal.draw(|frame| {
            loop_state.last_frame_area = frame.area();
            render_ui(frame, app);
        })?;
        app.needs_redraw = false;
    }

    let _ = execute!(io::stdout(), EndSynchronizedUpdate);
    Ok(())
}

fn expire_status_message(app: &mut App) {
    let expired = app
        .status_message
        .as_ref()
        .is_some_and(|msg| app.ui_tick.wrapping_sub(msg.created_at_tick) >= STATUS_FADE_TICKS);
    if expired {
        app.status_message = None;
    }
}

/// 遅くてよい後始末。描画のあとに置いてあるので、ここが重くても入力の
/// 応答性には効かない。
pub(super) fn run_background_work(app: &mut App, loop_state: &mut LoopState) {
    // PTY のサイズをキャッシュ済みレイアウトに合わせる。
    app.sync_pty_sizes(
        &mut loop_state.last_claude_size,
        &mut loop_state.last_shell_size,
    );

    if !loop_state.first_frame_done {
        loop_state.first_frame_done = true;
        app.perform_auto_resume();
    }

    // 定期タイマー (git のポーリング、装飾、ターミナルの再描画など)。
    // ユーザーが操作中 (スクロール中など) は重い I/O タイマーを遅らせて、
    // スクロールの途中で固まらないようにする。
    let input_active = loop_state.input_active();
    let sources = &mut loop_state.sources;
    crate::event_loop_timers::run_due_timers(
        app,
        &mut loop_state.timers,
        &mut sources.file_watcher,
        &mut sources.watch_paths,
        input_active,
        loop_state.ccusage_poll_secs,
    );
    crate::event_loop_timers::poll_watchers(
        app,
        &sources.file_watcher,
        &mut loop_state.fs_debounce.pending,
        &mut loop_state.fs_debounce.first_seen,
        &sources.config_watcher,
        &mut loop_state.cfg_debounce.pending,
        &mut loop_state.cfg_debounce.first_seen,
        &sources.cc_notify,
        &sources.refresh_pipe,
    );

    app.poll_all_background_ops();

    // 自動ホバー: マウスがシンボル上で規定時間止まったらポップアップを出し、
    // 猶予時間と無効化を管理する。
    app.tick_hover();
    // ジャンプ用アンダーライン (D8/D9): ポップアップとは別の、より速い
    // (150ms・猶予なし) デバウンス。tick_underline_hover を参照。
    app.tick_underline_hover();

    if app.overlays.active == crate::overlay::ActiveOverlay::GrepSearch {
        let root = app.explorer.root().to_path_buf();
        if app.overlays.grep_search.check_debounce(&root) {
            app.request_redraw();
        }
    }
    if !app.terminal.deferred_prompts.is_empty() {
        app.flush_deferred_prompts();
    }
    app.terminal.pty_manager.nudge_alt_screen_sessions();
}
