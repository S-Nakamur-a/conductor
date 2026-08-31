//! Reflow トランスクリプトビュー: 無限スクロールバックモード中に Claude の PTY パネルへ
//! 重ねて表示する、読み取り専用・折り返し表示のセッションログビューア。

pub mod input;
pub mod key;
pub mod log;
pub mod render;

use crate::app::{App, StatusLevel};

/// 入場アニメーションの開始時刻。退場側は無く、閉じるときは即座にライブ PTY へ戻す。
pub struct Sweep {
    pub start: std::time::Instant,
}

/// Claude の PTY パネルに重ねる、読み取り専用のセッションログビュー。
#[derive(Default)]
pub struct ReflowView {
    pub active: bool,
    pub loading: bool,
    /// Rc なのは、build_lines が clone して self の借用を先に解放できるようにするため。
    /// リサイズのたびに全エントリをディープコピーせずに済む。
    pub entries: std::rc::Rc<Vec<log::LogEntry>>,
    /// 先頭からスキップする描画済み行数。エントリ数ではない。
    pub scroll: usize,
    pub total_lines: usize,
    /// 直近描画時のパネル内側の幅。変化がリフローの契機になる。
    pub last_width: u16,
    /// 最新ターンに追従しているか。上へスクロールすると外れる。
    ///
    /// リサイズ時の扱いがこれで決まる。無条件に最下部へ固定すると読み手の位置を失い、
    /// 無条件に論理行へアンカーすると狭いパネルで最新行が画面外へ出る。
    pub follow: bool,
    /// 「最新ターンへジャンプ」バッジの矩形。描画側が毎フレーム記録し、クリックが読む。
    pub jump_hit: Option<ratatui::layout::Rect>,
    pub cached_lines: Vec<ratatui::text::Line<'static>>,
    /// 各行の由来エントリと、ガターのために未書き込みで残すセルの位置。
    pub line_meta: Vec<render::LineMeta>,
    /// 幅の変化以外での無効化。現状は expand トグルのみ。
    pub needs_rebuild: bool,
    /// オーバーレイが閉じた直後にハード再描画するために使う。未書き込みで残すセル
    /// ([render::LineMeta::skip_col]) はオーバーレイの内容を保持してしまい、自分の
    /// バッファ同士しか比較しない ratatui の diff では消えない。
    pub last_overlay_active: bool,
    /// 直近描画時の内側の高さ。ページスクロールの幅に使う。
    pub last_inner_height: u16,
    /// App::markdown_cache と分けているのは、共有キャッシュを汚さず、セッションを
    /// 開き直したときに丸ごと捨てられるようにするため。
    pub cache: crate::ui::markdown::MarkdownCache,
    pub sweep: Option<Sweep>,
    /// tool_use/tool_result を常に展開するか (Claude の ctrl+o 相当)。ブロック単位
    /// ではなくグローバル。幅変化チェックには掛からないので needs_rebuild が要る。
    pub expanded: bool,
}

impl App {
    // Reflow トランスクリプトビュー

    /// アクティブな Claude パネルのトランスクリプトを開く。
    ///
    /// 解決できない、または表示できるターンが無いときは理由をステータスに出して
    /// 開かない。他のセッションの履歴で代替することは決してない。
    pub fn open_reflow(&mut self) {
        // 参照先は pin された session id だけで特定する。ディレクトリ単位に広げると
        // 同じワークツリーで走った別セッションのログを掴む — サブエージェント実行中は
        // 自分のログへの書き込みが止まるので、「最新のログ」も「続きに見えるログ」も
        // 当てにならない。例外は /clear のローテーションだけで、pin は cc_hook が
        // 更新する。フックが沈黙した環境では claude_sessions::rotation が推測する。
        let Some(idx) = self.terminal.claude.active_session else {
            self.set_status(
                "No Claude session for this panel; transcript unavailable".to_string(),
                StatusLevel::Warning,
            );
            return;
        };
        let Some((working_dir, session_id, spawned_at)) =
            self.terminal.pty_manager.claude_session_ref(idx)
        else {
            self.set_status(
                "No Claude session for this panel; transcript unavailable".to_string(),
                StatusLevel::Warning,
            );
            return;
        };
        let claimed = self.terminal.pty_manager.other_claude_session_ids(idx);

        let Some(path) = crate::claude_sessions::current_session_log(
            &working_dir,
            &session_id,
            spawned_at,
            &claimed,
        ) else {
            self.set_status(
                format!("No session log on disk for {session_id}"),
                StatusLevel::Warning,
            );
            return;
        };

        // ガターグリフの後に未書き込みで残すセルに、ライブ PTY の内容が残り続ける。
        self.terminal.needs_clear = true;

        // 5MB 級の .jsonl だと 60fps ループが数フレーム止まるのでバックグラウンドへ。
        self.bg.reflow_load.start(move |tx| {
            let _ = tx.send(log::load_session(&path));
        });

        self.reflow = ReflowView {
            active: true,
            loading: true,
            entries: std::rc::Rc::new(Vec::new()),
            scroll: 0,
            total_lines: 0,
            last_width: 0, // 初回描画で行の全面再構築を強制する。
            // 最新ターンで開く。これが初回描画の着地点も決める。
            follow: true,
            jump_hit: None,
            cached_lines: Vec::new(),
            line_meta: Vec::new(),
            needs_rebuild: false,
            last_overlay_active: false,
            last_inner_height: 0,
            cache: crate::ui::markdown::MarkdownCache::new(),
            sweep: Some(Sweep {
                start: std::time::Instant::now(),
            }),
            // Claude Code 自身のデフォルトに合わせて折りたたみで開く。
            expanded: false,
        };
    }

    /// バックグラウンドのパース結果を反映する。ビューが閉じていれば捨てる。
    pub fn poll_reflow_load(&mut self) {
        let Some(entries) = self.bg.reflow_load.poll() else {
            return;
        };
        if !self.reflow.active {
            return;
        }
        if entries.is_empty() {
            self.close_reflow();
            self.set_status(
                "Session log is empty or unreadable".to_string(),
                StatusLevel::Info,
            );
            return;
        }
        self.reflow.entries = std::rc::Rc::new(entries);
        self.reflow.loading = false;
        self.reflow.last_width = 0; // 次の描画で行の全面再構築を強制する。
        // プレースホルダはスクロールできないので、読み手が離脱している可能性はない。
        self.reflow.follow = true;
        // 入場スイープが先に終わっている場合があるので、明示的に描き直す。
        self.request_redraw();
    }

    /// 最新ターンへ飛び、追従を再開する。G/End とバッジのクリックが共有する唯一の入口。
    pub fn reflow_jump_to_latest(&mut self) {
        self.reflow.scroll = input::bottom_scroll(
            self.reflow.total_lines,
            self.reflow.last_inner_height as usize,
        );
        self.reflow.follow = true;
        // 全行が動くので、幅を過小申告するグリフの残像が ratatui の diff に映らない。
        self.terminal.needs_clear = true;
    }

    /// ビューを閉じ、ライブ PTY 表示へ戻す。
    pub fn close_reflow(&mut self) {
        self.reflow.active = false;
        self.reflow.sweep = None;
        // 閉じたあとに古い結果が届かないようにする。
        self.bg.reflow_load.clear();
        self.terminal.claude.scroll = 0;
        // 表示中 PTY パネルは何も描いていないので、キャッシュにはスクロールバック前の
        // フレームが残っている。Claude が待機中だと新しい出力が来ず、そのまま残る。
        self.terminal.claude.cache = Default::default();
        self.terminal.claude.dirty = true;
    }
}
