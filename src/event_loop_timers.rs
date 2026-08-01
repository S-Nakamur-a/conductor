//! メインイベントループ ([`crate::event_loop::run_loop`]) 向けの、周期タイマーの
//! 処理と外部イベントソースのポーリング。
//!
//! ループ本体から切り出したのは [`crate::event_loop`] が読める大きさを超えないように
//! するためだけで、挙動は変えていない。1 周ごとの「バックグラウンド処理」を
//! 2 つの関数として括り出したもの。

use std::time::{Duration, Instant};

use crate::app::App;
use crate::event_loop::watch_paths_for;

/// この周回で発火時刻に達した周期タイマーをすべて実行する。git と worktree の
/// ポーリング、装飾・鼓動・rich グローの再描画周期、PTY の後始末、Claude Code の
/// 待機状態、統計の更新、ccusage、アップデート確認。
pub(crate) fn run_due_timers(
    app: &mut App,
    timers: &mut crate::timer::TimerRegistry,
    file_watcher: &mut Option<crate::file_watcher::FileWatcher>,
    current_watch_paths: &mut Vec<std::path::PathBuf>,
    rich_active: bool,
    input_active: bool,
    ccusage_poll_secs: u64,
) {
    for name in timers.check_due() {
        match name {
            // 静かなモード: 装飾を進めるのは worktree パネルにフォーカスがあるときだけ。
            // レビューや端末作業の最中に視界の端で動き続けることがないようにする。
            // それ以外のときはその場で止まり、フォーカスが戻ったら再開する。
            "decoration" if app.focus == crate::app::Focus::Worktree => {
                let left_w = app.layout.cache.columns[0].width;
                let panel_h = app.layout.cache.main_area.height;
                let list_h = (app.worktrees.len() as u16 + 2).max(5);
                let detail_h = (1 + app.worktree_mgr.local_branches.len() as u16 + 2).min(8);
                let deco_h = panel_h.saturating_sub(list_h + detail_h);
                if app.tick_decoration(left_w.saturating_sub(2), deco_h) {
                    app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
                }
            }
            // どこかのセッションが待機している限り、フォーカスや装飾のアニメーション
            // 有無にかかわらず通知バーの明滅を動かす。
            "pulse" if !app.terminal.cc_waiting_worktrees.is_empty() => {
                app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
            }
            // パーティモードのアニメーション (虹色の枠、シンタックス、紙吹雪) を動かす。
            "pulse" if app.party_mode => {
                app.dirty.mark_all();
            }
            // rich モードのグラデーション枠を約 30fps で安定して動かす。この効果は
            // フレーム全体への後処理なので、進めるには全面の再描画が必要。PTY の
            // ラスタはキャッシュされたまま (`dirty_claude` / `dirty_shell` で制御)
            // なので、これはウィジェットの安い再描画で済む。
            "rich_glow" if rich_active => {
                app.dirty.mark_all();
            }
            "unfocused_terminal" => {
                match app.focus {
                    crate::app::Focus::TerminalClaude => {
                        app.terminal.cache_shell = Default::default();
                    }
                    crate::app::Focus::TerminalShell => {
                        app.terminal.cache_claude = Default::default();
                    }
                    _ => {
                        app.terminal.cache_claude = Default::default();
                        app.terminal.cache_shell = Default::default();
                    }
                }
                app.dirty.mark(crate::app::DirtyPanels::TERMINAL);
            }
            // I/O の重いタイマー。入力中はスクロールが固まるので飛ばす。
            "worktree_poll" if !input_active => {
                if app.refresh_worktrees() {
                    app.dirty.mark(
                        crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::EXPLORER,
                    );
                }
                // 監視対象のパス集合が変わったらファイル監視を作り直す (`git init` で
                // 最初の worktree ができた、worktree が追加・削除された、など)。
                // これが無いと古い集合を監視し続けて新しいファイルを取りこぼす。
                let desired = watch_paths_for(app);
                if desired != *current_watch_paths {
                    match crate::file_watcher::FileWatcher::new(&desired) {
                        Ok(w) => {
                            *current_watch_paths = desired;
                            *file_watcher = Some(w);
                        }
                        Err(e) => {
                            // 監視が丸ごと無くなる形に静かに劣化させるのではなく、
                            // 前の監視 (古いパス集合に対してはまだ有効) を残す。
                            // 次のポーリングで再試行する。
                            log::warn!("file watcher rebuild failed: {e}");
                            app.set_status(
                                format!("File watcher rebuild failed ({e})"),
                                crate::app::StatusLevel::Warning,
                            );
                        }
                    }
                }
                // 定期的な保険: ファイルツリーを歩き直して、監視イベントを取りこぼしても
                // 新規作成されたファイルが現れるようにする。安い処理 (子は遅延読み込み)
                // で、変化があったときだけ再描画する。
                if app.refresh_viewer() {
                    app.dirty.mark(
                        crate::app::DirtyPanels::EXPLORER | crate::app::DirtyPanels::VIEWER,
                    );
                }
                app.check_diff_viewer_staleness();
            }
            "pty_cleanup" if !input_active && app.cleanup_dead_sessions() => {
                app.dirty
                    .mark(crate::app::DirtyPanels::TERMINAL | crate::app::DirtyPanels::WORKTREE);
            }
            "cc_waiting" if !input_active => {
                if app.check_cc_waiting_state() {
                    app.dirty.mark(
                        crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL,
                    );
                }
                app.flush_deferred_prompts();
            }
            "stats_refresh" if !input_active => {
                if let Some(store) = &app.review_store {
                    let new_stats = store.get_today_stats().ok();
                    if new_stats != app.stats.today {
                        app.stats.today = new_stats;
                        app.dirty.mark(crate::app::DirtyPanels::WORKTREE);
                    }
                }
            }
            "ccusage" => {
                let max_age = ccusage_poll_secs;
                app.bg.ccusage.start(move |tx| {
                    let info = crate::ccusage_cache::read_if_fresh(max_age)
                        .or_else(crate::ccusage_cache::fetch_and_cache);
                    if let Some(info) = info {
                        let _ = tx.send(info);
                    }
                });
            }
            "update_check" => {
                app.bg.update_check.start(|tx| {
                    let _ = tx.send(crate::update_checker::check_for_update());
                });
            }
            _ => {}
        }
    }
}

/// ループに入力を供給する外部イベントソース (ファイル監視、設定ファイル監視、
/// Claude Code の状態通知ソケット、MCP のリフレッシュパイプ) をすべてポーリングし、
/// デバウンス済みまたは即時の効果を `app` へ反映する。
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_watchers(
    app: &mut App,
    file_watcher: &Option<crate::file_watcher::FileWatcher>,
    fs_pending: &mut bool,
    fs_first_seen: &mut Option<Instant>,
    config_watcher: &Option<crate::config_watcher::ConfigWatcher>,
    cfg_pending: &mut bool,
    cfg_first_seen: &mut Option<Instant>,
    cc_notify: &Option<crate::cc_notify::CcNotifyListener>,
    refresh_pipe: &Option<crate::refresh_pipe::RefreshPipe>,
) {
    // ファイルシステムのイベント 1 件ごとに高コストな git 操作が走らないよう、
    // ファイル監視によるリフレッシュはデバウンスする。
    const FS_DEBOUNCE: Duration = Duration::from_millis(500);
    // 設定ファイル変更用の別のデバウンス。FS_DEBOUNCE より短く、独立させてあるので
    // worktree ポーリングによる作り直しでリセットされない。
    const CONFIG_DEBOUNCE: Duration = Duration::from_millis(300);

    // ファイルシステムの変更イベント (デバウンスあり)。
    if let Some(watcher) = file_watcher {
        while watcher.poll().is_some() {
            if !*fs_pending {
                *fs_first_seen = Some(Instant::now());
            }
            *fs_pending = true;
        }
        if *fs_pending
            && let Some(t) = *fs_first_seen
            && t.elapsed() >= FS_DEBOUNCE
        {
            *fs_pending = false;
            *fs_first_seen = None;
            app.refresh_worktrees();
            app.refresh_viewer();
            app.refresh_diff();
            if !app.bg.symbol_index.is_running() {
                app.start_symbol_index_build();
            }
            app.dirty.mark_all();
        }
    }

    // 設定ファイルの変更イベント (デバウンスあり)。FS のイベントより短いのは、
    // 2 つのイベント列が独立しているから。worktree ポーリングによる作り直しが
    // 設定側のデバウンスタイマーをリセットしてはいけない。
    if let Some(watcher) = config_watcher {
        while watcher.poll().is_some() {
            if !*cfg_pending {
                *cfg_first_seen = Some(Instant::now());
            }
            *cfg_pending = true;
        }
        if *cfg_pending
            && let Some(t) = *cfg_first_seen
            && t.elapsed() >= CONFIG_DEBOUNCE
        {
            *cfg_pending = false;
            *cfg_first_seen = None;
            app.reload_appearance_config();
        }
    }

    // Claude Code の状態通知。
    if let Some(cc_notify) = cc_notify {
        while let Some(event) = cc_notify.poll() {
            app.handle_cc_notify(event);
            app.dirty
                .mark(crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::TERMINAL);
        }
    }

    // MCP のリフレッシュパイプ。MCP がパイプへ書いたらレビューコメントを読み直す。
    if let Some(refresh_pipe) = refresh_pipe
        && refresh_pipe.poll().is_some()
    {
        // 余分なイベントを吸い出す (連続した書き込みをまとめる)。
        while refresh_pipe.poll().is_some() {}
        app.refresh_reviews();
        app.dirty.mark_all();
        log::debug!("refresh_pipe: reloaded reviews from MCP trigger");
    }
}
