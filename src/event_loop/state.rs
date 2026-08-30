//! イベントループが反復をまたいで持ち回る状態と、その初期化。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use super::{
    ACTIVITY_TIMEOUT, DECORATION_TICK_INTERVAL, PULSE_TICK_INTERVAL, UNFOCUSED_TERMINAL_REFRESH,
};
use crate::app::{App, Focus};
use crate::timer;

/// 画面領域の持ち主が変わったことを検出するための鍵。
///
/// (最大化, エディタ, リフロー, explorer幅, viewer幅, ターミナル分割, explorer分割)。
/// このどれかが変わると、ある領域の描画主体が別のパネルに移る。ratatui のセル差分は
/// その受け渡しを見られないので、明け渡された縁に前の持ち主のグリフが残ってしまう
/// (復帰した Explorer にエディタのコード片が見える、リサイズしたパネルの旧い縁に
/// 文字が残る、など)。変化を検出したらハードクリアで画面を取り直す。
type LayoutKey = (Option<Focus>, bool, bool, u16, u16, u16, u16);

/// ファイル監視 / config 監視のイベントのデバウンス状態。
///
/// 実際のデバウンス間隔は [crate::event_loop_timers::poll_watchers] 側にある。
#[derive(Default)]
pub(super) struct DebounceState {
    pub pending: bool,
    pub first_seen: Option<Instant>,
}

/// 外部からのイベント源。どれも生成に失敗しうるので、失敗したら機能を落として続ける。
pub(super) struct EventSources {
    /// worktree 配下のファイル変更監視。監視対象は worktree の増減に応じて作り直す。
    pub file_watcher: Option<crate::file_watcher::FileWatcher>,
    /// 現在監視しているパス。file_watcher の作り直しの判断に使う。
    pub watch_paths: Vec<PathBuf>,
    /// conductor の config ファイル専用の監視。
    ///
    /// worktree 用と分けてあるのは、worktree 用が worktree の増減のたびに
    /// 作り直されるから — 同じインスタンスを共有すると config の監視が切れる。
    pub config_watcher: Option<crate::config_watcher::ConfigWatcher>,
    /// Claude Code の状態通知を受けるソケット (即時配送)。
    pub cc_notify: Option<crate::cc_notify::CcNotifyListener>,
    /// MCP からのレビュー再読み込み要求を受ける名前付きパイプ。
    pub refresh_pipe: Option<crate::refresh_pipe::RefreshPipe>,
}

/// イベントループの反復をまたいで生きる状態。
pub(super) struct LoopState {
    pub sources: EventSources,
    pub timers: timer::TimerRegistry,
    pub fs_debounce: DebounceState,
    pub cfg_debounce: DebounceState,
    /// 直近のフレームの描画領域。マウス座標の解決に使う。
    pub last_frame_area: Rect,
    /// 直近に PTY へ伝えたサイズ。変化したときだけリサイズを送る。
    pub last_claude_size: (u16, u16),
    pub last_shell_size: (u16, u16),
    pub first_frame_done: bool,
    pub last_layout_key: Option<LayoutKey>,
    /// 最後にユーザー入力があった時刻。アクティブ / アイドルの tick 速度を切り替える。
    pub last_input_time: Instant,
    /// ccusage のポーリング間隔 (秒)。config から起動時に読む。
    pub ccusage_poll_secs: u64,
}

impl LoopState {
    /// 監視・タイマー・起動時の先読みを仕掛けて、ループの初期状態を作る。
    pub fn setup(app: &mut App) -> Self {
        let sources = Self::open_event_sources(app);
        let timers = Self::register_timers(app);

        // 最初のフレームが描かれるように再描画を要求しておく。
        app.request_redraw();

        Self {
            sources,
            timers,
            fs_debounce: DebounceState::default(),
            cfg_debounce: DebounceState::default(),
            last_frame_area: Rect::default(),
            last_claude_size: (0, 0),
            last_shell_size: (0, 0),
            first_frame_done: false,
            last_layout_key: None,
            last_input_time: Instant::now() - ACTIVITY_TIMEOUT,
            ccusage_poll_secs: app.config.ccusage.poll_interval_secs,
        }
    }

    fn open_event_sources(app: &mut App) -> EventSources {
        // 監視対象は後で (worktree_poll タイマーの中で) 変化に応じて作り直す —
        // ユーザーが素のフォルダで git init したり worktree を増減させたりしても
        // 新しいファイルが見えるように。
        let watch_paths = app.watch_paths();
        let file_watcher = match crate::file_watcher::FileWatcher::new(&watch_paths) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("file watcher setup failed: {e}");
                app.set_status(
                    format!("File watcher unavailable — auto-refresh degraded ({e})"),
                    crate::app::StatusLevel::Warning,
                );
                None
            }
        };

        EventSources {
            file_watcher,
            watch_paths,
            config_watcher: crate::config_watcher::ConfigWatcher::new(
                &crate::config::config_file_path(),
            )
            .ok(),
            cc_notify: crate::cc_notify::CcNotifyListener::new(&app.repo.path).ok(),
            refresh_pipe: crate::refresh_pipe::RefreshPipe::new(&app.repo.path).ok(),
        }
    }

    fn register_timers(app: &mut App) -> timer::TimerRegistry {
        let mut timers = timer::TimerRegistry::new();
        timers.register("worktree_poll", Duration::from_secs(3));
        timers.register("pty_cleanup", Duration::from_secs(10));
        timers.register("cc_waiting", Duration::from_secs(5));
        timers.register("stats_refresh", Duration::from_secs(30));
        timers.register("decoration", DECORATION_TICK_INTERVAL);
        timers.register("unfocused_terminal", UNFOCUSED_TERMINAL_REFRESH);
        timers.register("pulse", PULSE_TICK_INTERVAL);

        Self::bootstrap_ccusage(app, &mut timers);
        Self::bootstrap_update_check(app, &mut timers);
        timers
    }

    /// ccusage は複数の Conductor で npx ccusage を重複起動しないよう
    /// グローバルなファイルキャッシュを使う。起動時はキャッシュの中身をそのまま出し、
    /// 鮮度の確認は即座にスケジュールする。
    fn bootstrap_ccusage(app: &mut App, timers: &mut timer::TimerRegistry) {
        if !app.config.ccusage.enabled {
            return;
        }
        if let Some(info) = crate::ccusage_cache::read_any() {
            app.stats.ccusage = Some(info);
        }
        timers.register_immediate(
            "ccusage",
            Duration::from_secs(app.config.ccusage.poll_interval_secs),
        );
    }

    /// 更新チェックはキャッシュからバッジを即出しつつ、最新のリリース情報は必ず
    /// バックグラウンドで取り直す (キャッシュが古くて新版を見落とさないように)。
    fn bootstrap_update_check(app: &mut App, timers: &mut timer::TimerRegistry) {
        if !app.config.updates.check_on_startup {
            return;
        }
        timers.register(
            "update_check",
            Duration::from_secs(app.config.updates.check_interval_secs),
        );

        use crate::update_checker;
        if let Some(cached) = update_checker::read_cache()
            && update_checker::is_newer(&cached.latest_version, update_checker::current_version())
        {
            app.update.info = Some(cached);
        }
        app.bg.update_check.start(|tx| {
            let _ = tx.send(update_checker::check_for_update());
        });
    }

    /// 領域の持ち主が変わったかを判定し、変わっていたらハードクリアを予約する。
    pub fn note_layout_change(&mut self, app: &mut App) {
        let key = (
            app.expanded_panel,
            app.editor.is_some(),
            app.reflow.active,
            app.config.layout.explorer_width_pct,
            app.config.layout.viewer_width_pct,
            app.layout.terminal_split_pct,
            app.config.layout.explorer_split_pct,
        );
        if self.last_layout_key == Some(key) {
            return;
        }
        // 初回はクリア不要 — まだ前の持ち主がいない。
        if self.last_layout_key.is_some() {
            app.terminal.needs_clear = true;
        }
        self.last_layout_key = Some(key);
    }

    /// ユーザーがいま操作中か (重い I/O タイマーを遅らせる判断に使う)。
    pub fn input_active(&self) -> bool {
        self.last_input_time.elapsed() < ACTIVITY_TIMEOUT
    }
}
