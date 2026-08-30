//! Reflow トランスクリプトビュー: 無限スクロールバックモード中に Claude の PTY パネルへ
//! 重ねて表示する、読み取り専用・折り返し表示のセッションログビューア。

pub mod input;
pub mod key;
pub mod log;
pub mod render;

use crate::app::{App, StatusLevel};

/// reflow トランスクリプトビューの入場アニメーション。
///
/// アニメーション開始時刻の Instant を保持し、reflow_view::render がフレームカウンタに
/// 依存せず経過時間から進捗を計算できるようにする。アニメーションするのは入場遷移だけで、
/// 初回の build_lines の遅延を隠す目的。ビューを閉じるときはアニメーションなしで即座に
/// ライブ PTY へ戻し、プロンプトへの復帰を瞬時に感じさせる。
pub struct Sweep {
    pub start: std::time::Instant,
}

/// reflow トランスクリプトビューの状態。
///
/// active が true のとき、このビューは Claude の PTY パネルに重なって表示され、
/// Claude Code のセッションログをスクロール可能な折り返し Markdown として描画する。
/// 幅が変わると cached_lines を全面的に再構築し、それ以外は毎フレームキャッシュを
/// 再利用する。
#[derive(Default)]
pub struct ReflowView {
    /// reflow ビューが現在 Claude の PTY パネルに重なって表示されているかどうか。
    pub active: bool,
    /// セッションログをバックグラウンドスレッドでまだパース中かどうか。true の間は
    /// トランスクリプトの代わりに「Loading…」というプレースホルダを表示し、
    /// poll_reflow_load がエントリを受け取った時点でクリアされる。
    pub loading: bool,
    /// セッションファイルからパース・正規化したログエントリ。
    ///
    /// Rc で包んでいるのは、build_lines がハンドルを安価に clone（refcount の
    /// インクリメントのみ）でき、cache.render を呼ぶ前に self への借用を解放できる
    /// ようにするため。リサイズのたびに全エントリの文字列をディープコピーせずに済む。
    pub entries: std::rc::Rc<Vec<log::LogEntry>>,
    /// 垂直スクロールオフセット — 先頭からスキップする描画済み行数。
    pub scroll: usize,
    /// cached_lines の総行数（各描画のあとに同期される）。
    pub total_lines: usize,
    /// 直近描画時のパネル内側の幅 — reflow のためのサイズ変化検知に使う。
    pub last_width: u16,
    /// 最下部への追従状態: 最新ターンに追従している間は true、読み手が上へスクロール
    /// して離れると false になる。
    ///
    /// このフラグが reflow 時にビューポートをどう扱うかを決める。追従中は毎フレーム
    /// 最下部に再固定するのでリサイズ後も最新出力を表示し続け、離脱中は代わりに同じ
    /// 論理行へ戻す（[input::scroll_after_reflow] 参照）。この区別が
    /// ないと必ずどちらかが壊れる — 無条件に固定すると読み手の位置を失い、無条件に
    /// アンカーすると幅の狭いパネルで行が増えたときに最新行がビューポートの外に出て
    /// しまう。
    ///
    /// 「開いた直後は固定する」というフラグも兼ねている。ビューは追従状態で開くので、
    /// 初回描画はこのフラグだけで最新ターンに着地し、別途ワンショットの状態を持つ
    /// 必要がない。
    pub follow: bool,
    /// 離脱中に描画される「最新ターンへジャンプ」バッジの画面上の矩形。画面に出ていない
    /// ときは None。描画側が毎フレーム記録し、クリックハンドラが参照する。
    /// [crate::app::App::reflow_jump_to_latest] 参照。
    pub jump_hit: Option<ratatui::layout::Rect>,
    /// 幅に合わせて事前に折り返し描画した行。last_width が変わるか needs_rebuild
    /// が立ったときだけ再構築する。
    pub cached_lines: Vec<ratatui::text::Line<'static>>,
    /// cached_lines の各行に対応する1エントリ。どこ由来か（スクロールアンカー）と、
    /// ガター用グリフのために未書き込みのままにするセルの位置を持つ。
    pub line_meta: Vec<render::LineMeta>,
    /// 幅の変化以外で cached_lines を無効化する必要があるときに立てる —
    /// 現状では expand トグルのみ。
    pub needs_rebuild: bool,
    /// 直前のフレームでオーバーレイがパネルを覆っていたかどうか。オーバーレイが閉じた
    /// ときに強制的にハード再描画するために使う。このビューが意図的に未書き込みのまま
    /// にしているセル（LineMeta::skip_col 参照）はオーバーレイが描いた内容を保持
    /// してしまい、自分のバッファ同士しか比較しない ratatui の diff では再描画されない。
    pub last_overlay_active: bool,
    /// 直近描画時のパネル内側の高さ — ページスクロールのサイズ算出に使う。
    pub last_inner_height: u16,
    /// セッションごとの Markdown 描画キャッシュ。
    ///
    /// App::markdown_cache とは別に持つことで、共有キャッシュを reflow 用のキーで
    /// 汚さず、新しいセッションを開いたとき（open_reflow が ReflowView 全体を
    /// 差し替える）に自動的に無効化されるようにしている。
    pub cache: crate::ui::markdown::MarkdownCache,
    /// 進行中の入場・退場スイープアニメーション。アイドル時は None。
    ///
    /// Option<Sweep> のデフォルトは None なので、Sweep 自体は Default を
    /// 実装する必要がない — Option<T>: Default は T: Default の制約なしに常に
    /// None になる。
    pub sweep: Option<Sweep>,
    /// Conductor 独自の ctrl+o 相当のトグル。true のとき、tool_use/tool_result
    /// ブロックを Claude のデフォルトである折りたたみ表示ではなく、常に展開して
    /// 描画する。ブロック単位ではなくグローバルなトグル。切り替えると
    /// cached_lines の再構築を強制する（render.rs の幅変化チェックだけでは
    /// 検知できないため）。
    pub expanded: bool,
}

impl App {
    // Reflow トランスクリプトビュー

    /// アクティブな Claude パネルのセッションに対して reflow トランスクリプトビューを開く。
    ///
    /// 現在表示している Claude パネルを支えるセッションのログを、pin されている
    /// claude_session_id から解決し（/clear によるローテーションを追うのは
    /// claude_sessions::current_session_log を参照）、その .jsonl を読み込んで
    /// パースしオーバーレイを有効化する。id がログに解決できない場合や、ログに
    /// 表示可能なターンが1つもない場合はステータスフラッシュで理由を示してビューは
    /// 非アクティブのままにする。他のセッションの履歴で代替することは決してない。
    pub fn open_reflow(&mut self) {
        // トランスクリプトの参照元は、現在表示している Claude パネルを支えるセッション
        // そのものであり、pin されているセッション id だけで特定する
        // （PtySession::claude_session_id を参照）。
        //
        // ここをディレクトリ単位の基準に広げてはいけない。1つの Claude プロジェクト
        // ディレクトリには、そのワークツリーで過去に走った全セッションのログが入っている
        // — 兄弟の Conductor パネル（CC:1, CC:2, …）、それより前の実行、素の claude
        // 起動など。そのため「一番新しいログ」や「このセッションの最終ターンに続けて
        // 最初のターンが始まっているログ」という基準では、別の会話を指してしまうことが
        // ある。後者は実際にこのビューが別セッションの履歴を表示していた原因で、後から
        // 始まったログを /clear ローテーションの続きとみなしていたが、これは pin
        // されたセッションがターンを書き続けている間しか成立しない。メインエージェントが
        // 停止していてサブエージェントだけが動いているパネルは、自分のセッションログに
        // 何も書き込まない（サブエージェントのターンは <session-id>/subagents/*.jsonl
        // へ書かれる）ため最終ターンが止まって見え、同じワークツリーで後から始まった
        // どのセッションもこの判定を通過してしまい、サブエージェントが動いている間ずっと
        // ビューを乗っ取っていた。
        //
        // 例外は /clear によるログのローテーションだけ。/clear は書き込み先
        // を新しい session id の .jsonl に移すので、pin した id を据え置くと
        // clear 前の会話を出したまま clear 後が一切出なくなる。
        //
        // pin は SessionStart フック (cc_hook) が更新する。フックはパネル
        // 自身の Claude プロセスの中で走るので、これは推測ではなく事実 — 同一
        // ワークツリーで複数パネルが同時に /clear しても取り違えない。
        // フックが沈黙した環境ではここから claude_sessions::rotation の
        // 推測にフォールバックするが、そちらも「先頭が /clear コマンドで、
        // 前セッションの最終書き込み以降に始まり、他パネルが pin していない」
        // ログしか後続と認めない。新規に起動しただけの claude はこの形に
        // ならないので、上に書いた乗っ取りは起きない。
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

        // ログのパースはバックグラウンドスレッドで行う。load_session は .jsonl
        // 全体を読み込んで JSON パースするため、大きなセッション（5MB 以上）だと
        // 60fps ループを数フレームぶんブロックしてしまう。ビューは「Loading…」
        // プレースホルダを表示して即座に有効化し、poll_reflow_load がエントリの
        // 到着後に差し替える。
        // ハード再描画を1回強制する: パネルは現在ライブ PTY を表示しており、
        // このビューがガターグリフの後に未書き込みのまま残すセルは、そのままだと
        // 古い内容がいつまでも残ってしまう。
        self.terminal.needs_clear = true;

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
            // 最新ターンで開き、読み手が上にスクロールするまでそこに留まる —
            // これが初回描画を固定する仕組みでもある。
            follow: true,
            jump_hit: None,
            cached_lines: Vec::new(),
            line_meta: Vec::new(),
            needs_rebuild: false,
            last_overlay_active: false,
            last_inner_height: 0,
            cache: crate::ui::markdown::MarkdownCache::new(),
            // 入場遷移を開始する: ボーダーが TRANSITION_DURATION_MS かけてアクセント色から
            // 補色へ滑らかに変化し、初回のロード + build_lines の遅延を隠す。
            sweep: Some(Sweep {
                start: std::time::Instant::now(),
            }),
            // 常に折りたたんだ状態で開く。Claude Code 自身のデフォルト（ctrl+o を
            // 押していない）のトランスクリプト表示に合わせている。
            expanded: false,
        };
    }

    /// バックグラウンドで完了したセッションログのパース結果を reflow ビューへ反映する。
    ///
    /// パース中にビューが閉じられていた場合は結果を破棄する（古くなっているため）。
    /// ログが空だった場合はステータスフラッシュとともにビューを閉じる — これは
    /// ビューを有効化する前に旧来の同期パスが出していたのと同じ結果である。
    pub fn poll_reflow_load(&mut self) {
        let Some(entries) = self.bg.reflow_load.poll() else {
            return;
        };
        if !self.reflow.active {
            return; // ロード中にビューが閉じられた場合は、古い結果を捨てる。
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
        // プレースホルダにはスクロールする対象がなかったため、読み手がまだ離脱している
        // はずがない — トランスクリプトは最新ターンに固定された状態で届く。
        self.reflow.follow = true;
        // ロードが遅いと入場スイープはすでに終わっている可能性がある。トランスクリプトが
        // 「Loading…」プレースホルダを即座に置き換えるよう再描画する。
        self.request_redraw();
    }

    /// トランスクリプトを最新ターンへジャンプさせ、追従を再開する。
    ///
    /// 「最新へ戻る」唯一の入口であり、G/End キーと離脱中バッジのクリックの
    /// 両方から共有される。そのためどちらの経路でもビューは同じ状態（最下部かつ
    /// 追従中）になり、次のリサイズでもその位置に留まる。
    pub fn reflow_jump_to_latest(&mut self) {
        self.reflow.scroll = input::bottom_scroll(
            self.reflow.total_lines,
            self.reflow.last_inner_height as usize,
        );
        self.reflow.follow = true;
        // キーハンドラでの1ステップごとのクリアと同じ理由: 全行が動くため、端末が
        // カウントより幅広く描くグリフがあると、ratatui 自身の diff では見えない
        // 残像が残ってしまう。
        self.terminal.needs_clear = true;
    }

    /// reflow トランスクリプトビューを終了し、ライブ PTY 表示へ戻る。
    pub fn close_reflow(&mut self) {
        self.reflow.active = false;
        self.reflow.sweep = None;
        // 実行中のバックグラウンドログパースをキャンセルし、ビューが閉じたあとに
        // 古い結果が届いたり、次回オープン時に紛れ込んだりしないようにする。
        self.bg.reflow_load.clear();
        // Claude のスクロールバックをリセットし、ライブの末尾を即座に表示する。
        self.terminal.claude.scroll = 0;
        // 次のフレームで PTY の新しいスナップショットを強制する。reflow ビューが
        // 表示されている間 PTY パネルは何も描画していなかったため、cache_claude には
        // スクロールバック前のフレームが残っている。閉じた直後に新しい出力が来なければ
        // （Claude がプロンプトで待機中など）このキャッシュが残り続け、入力欄が
        // 再表示されない。キャッシュをクリアして dirty を立てることでライブの末尾を
        // 即座に再構築する。
        self.terminal.claude.cache = Default::default();
        self.terminal.claude.dirty = true;
    }

    /// reflow トランスクリプトビューを終了し、即座にライブ PTY へ戻る。
    ///
    /// キーバインド・スクロールの呼び出し元のために close_reflow とは別の入口として
    /// 残しているが、退場アニメーションは存在しない。表示は同一フレームでライブの
    /// 末尾に切り替わるため、プロンプトへの復帰が瞬時に感じられる。
    pub fn request_close_reflow(&mut self) {
        self.close_reflow();
    }
}
