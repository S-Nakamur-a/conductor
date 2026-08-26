//! メインイベントループ ([crate::event_loop::run_loop]) 向けの、周期タイマーの
//! 処理と外部イベントソースのポーリング。
//!
//! ループ本体から切り出したのは [crate::event_loop] が読める大きさを超えないように
//! するためだけで、挙動は変えていない。1 周ごとの「バックグラウンド処理」を
//! 2 つの関数として括り出したもの。

use std::time::{Duration, Instant};

use crate::app::App;

/// この周回で発火時刻に達した周期タイマーをすべて実行する。git と worktree の
/// ポーリング、装飾・鼓動の再描画周期、PTY の後始末、Claude Code の
/// 待機状態、統計の更新、ccusage、アップデート確認。
pub(crate) fn run_due_timers(
    app: &mut App,
    timers: &mut crate::timer::TimerRegistry,
    file_watcher: &mut Option<crate::file_watcher::FileWatcher>,
    current_watch_paths: &mut Vec<std::path::PathBuf>,
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
            "unfocused_terminal" => {
                app.terminal.drop_inactive_caches(app.focus);
                app.dirty.mark(crate::app::DirtyPanels::TERMINAL);
            }
            // I/O の重いタイマー。入力中はスクロールが固まるので飛ばす。
            "worktree_poll" if !input_active => {
                if app.refresh_worktrees() {
                    app.dirty.mark(
                        crate::app::DirtyPanels::WORKTREE | crate::app::DirtyPanels::EXPLORER,
                    );
                }
                // 監視対象のパス集合が変わったらファイル監視を作り直す (git init で
                // 最初の worktree ができた、worktree が追加・削除された、など)。
                // これが無いと古い集合を監視し続けて新しいファイルを取りこぼす。
                let desired = app.watch_paths();
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
                    app.dirty
                        .mark(crate::app::DirtyPanels::EXPLORER | crate::app::DirtyPanels::VIEWER);
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
/// デバウンス済みまたは即時の効果を app へ反映する。
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
        while let Some(crate::file_watcher::FsEvent::Changed(path)) = watcher.poll() {
            // 索引の作り直しは自前の静穏時間 (3 秒) で数えるので、下の FS_DEBOUNCE
            // とは別に、1 件ずつそのまま渡す。
            let tree_root = app.selected_worktree_path();
            app.code_nav.semantic.note_change(&path, &tree_root);
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

    // 意味索引の作り直し。静穏時間の計測と子プロセスの見張りは sheaf 側にあり、
    // ここでやるのはチャネルを覗くのと時刻の比較だけ (索引の読み込みとパースは
    // ワーカースレッドにある)。
    tick_semantic_regeneration(app);

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

/// 意味索引の作り直しを 1 周進める。
///
/// 成果は選択中の worktree ではなく索引そのものに乗るので、生成が終わったら
/// 読み直す。生成が向いていたツリーと今の選択が違えば `Slot` が取り込みを拒み、
/// [`crate::app::App::start_semantic_index_load`] が正しい向きで読み直す。
fn tick_semantic_regeneration(app: &mut App) {
    let repo_root = app.repo.path.clone();
    let tree_root = app.selected_worktree_path();
    // 索引ルートの列挙も内容の鍵もツリーを歩くので、UI スレッドではやらない。
    // 要るときだけ背景に回して、届くまでは Loading のまま持ち越す。
    if app.code_nav.semantic.needs_survey(&tree_root).is_some() {
        app.start_semantic_index_load();
    }
    // 読んでいるファイルの索引ルートに索引が無ければ、ここで作りに行かせる。
    // 索引ルートは実在するリポジトリで 109 本になるので、まとめては作らない。
    if let Some(rel) = app.viewer_state.content.current_file.clone() {
        let reading =
            app.code_nav
                .semantic
                .note_open(std::path::Path::new(&rel), &repo_root, &tree_root);
        // 索引がこのファイルを説明できないと、黙って構文層に落ちる。言わないと
        // 「ジャンプが甘い」としか見えないので、そのファイルを開いたときに 1 度だけ出す。
        // 内容が動いただけなら作りに行っている (Building) ので、ここには来ない。
        if reading == crate::semantic_index::Reading::Stale {
            app.set_status(
                "Code index does not cover this file — Repo ▸ Rebuild Code Index".to_string(),
                crate::app::StatusLevel::Warning,
            );
        }
    }
    let Some(finished) = app
        .code_nav
        .semantic
        .tick_regeneration(&repo_root, &tree_root)
    else {
        return;
    };
    let manual = finished.manual;
    match finished.outcome {
        // 1 世代が作るのは索引ルート 1 本ぶんで、画面が引くのは全ルートを畳んだもの。
        // 受け取った Store をそのまま入れると、他のルートの索引が黙って落ちる。
        // 成果物はもうディスクに置かれているので、読み直しに任せる。
        crate::semantic_index::Regenerated::Ready { documents } => {
            log::info!("semantic index regenerated: {documents} documents");
            // 索引が無い状態から埋まったときだけ知らせる。作り直しは編集が
            // 収まるたびに走るので、毎回出すと status がそれで埋まる。
            // 手で頼まれたときは必ず知らせる。押した本人が結果を待っている。
            if manual || app.code_nav.semantic.store(&tree_root).is_none() {
                let unit = if documents == 1 { "file" } else { "files" };
                app.set_status(
                    format!("Code index ready ({documents} {unit})"),
                    crate::app::StatusLevel::Success,
                );
            }
            app.start_semantic_index_load();
        }
        // 待機に戻すのは sheaf 側がやる。ここで作り直しを起こすと二重に走る。
        crate::semantic_index::Regenerated::Busy => {}
        crate::semantic_index::Regenerated::Failed(why) => {
            log::warn!("semantic index regeneration failed: {why}");
            if manual {
                app.set_status(
                    format!("Could not rebuild the code index: {why}"),
                    crate::app::StatusLevel::Error,
                );
            }
        }
        crate::semantic_index::Regenerated::Unavailable(why) => {
            log::info!("semantic index disabled: {why}");
        }
    }
}
