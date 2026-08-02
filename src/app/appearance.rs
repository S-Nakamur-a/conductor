//! 外観設定（テーマ、シンタックスハイライト、diff表示モード、レイアウト比率）の
//! ランタイムでのテーマ切り替えと、設定ファイルのライブリロード。

use super::App;
use crate::config;

impl App {
    /// 現在のconfigからsyntectのハイライトテーマを組み直す。
    ///
    /// 解決結果が前回と同じなら何もしない。変わっていれば、テーマを差し替えた
    /// うえでハイライト済みspanを持つキャッシュをすべて無効化する。
    ///
    /// 「無効化」まで含めてここでやるのが肝。span色はキャッシュに焼き込まれる
    /// ので、テーマだけ差し替えても画面には古い配色が残り続ける。テーマを
    /// 触る経路(テーマピッカー・OSC11自動判定・configライブリロード)は必ず
    /// ここを通す。
    fn rebuild_syntect_theme(&mut self) {
        let new_id = config::syntax_theme_id(&self.config);
        if new_id == self.highlight.theme_id {
            return;
        }

        self.highlight.theme = config::syntect_theme_for(&self.config, &self.highlight.themes);
        self.highlight.theme_id = new_id;
        // Viewerのハイライトキャッシュのキーに混ざる。これを進めないと、
        // ファイル内容が同じままなのでrehighlight_viewerがキャッシュヒットで
        // 素通りしてしまう。
        self.highlight.generation = self.highlight.generation.wrapping_add(1);

        // レビューコメント内のコードブロックが新しいsyntectテーマを反映
        // できるよう、Markdownキャッシュをクリアする。このキャッシュは
        // UIのカラーパレットだけを指紋にしているので、シンタックスのみの
        // 変更ではそうしないと古いハイライトのspanが残ってしまう。
        self.markdown_cache.clear();

        // 次の描画でreflowトランスクリプトを完全に再構築させ、Markdown
        // のspanが新しいテーマ色とsyntectパレットを反映するようにする。
        // last_width=0にすることで、パネル幅が変わったかどうかに関わらず
        // 次のフレームでbuild_linesが必ず実行される。
        self.reflow.last_width = 0;
        self.reflow.cache.clear();
    }

    /// アクティブなUIテーマをランタイムで切り替える。
    ///
    /// UIパレットとsyntectのシンタックスハイライトテーマの両方を切り替える。
    /// 以前はUIパレットだけを組み直していたので、テーマを変えてもViewerの
    /// コードの配色が変わらなかった。
    ///
    /// persistがtrueのとき、選択は設定ファイル (~/.config/conductor/config.toml)
    /// に書き込まれ、再起動後も残る。書き込み失敗は致命的ではない: ログに
    /// 残し、警告フラッシュとして表示する。
    pub fn set_theme(&mut self, name: &str, persist: bool) {
        self.theme = super::build_theme(name, self.theme_sel.high_contrast);
        self.theme_sel.name = name.to_string();
        self.config.ui.theme = Some(name.to_string());

        // syntectテーマはconfigの現在値から解決されるので、ui.themeを書いた
        // 後に呼ぶ。
        self.rebuild_syntect_theme();
        // 開いているファイルを新しいテーマで塗り直す。generationが進んで
        // いるのでキャッシュは素通りしない。
        self.rehighlight_viewer();
        self.dirty.mark_all();

        if persist && let Err(e) = crate::config::persist_ui_theme(name) {
            log::warn!("failed to persist theme '{name}': {e}");
            self.set_status(
                format!("Theme saved in session but could not write config: {e}"),
                super::StatusLevel::Warning,
            );
        }
    }

    // 設定ファイルのライブリロード

    /// 新しい設定から、外観に関する（ライブリロード可能な）フィールドを
    /// 実行中のアプリへ適用する。
    ///
    /// LIVEに分類されたフィールドだけをコピーする。再起動が必要なフィールド
    /// （シェル、スクロールバック上限、API設定など）はあえて手つかずにする。
    /// これにより、呼び出しのたびにconfig.general.main_branchを読む
    /// refresh_diffが、古い値や過渡的な値を目にすることがなくなる。
    ///
    /// ここで適用されるLIVEフィールドは次のとおり。
    /// - ui.theme / viewer.theme → theme + theme_name + syntectの再構築
    ///   （syntect側もui.themeを優先する。以前はviewer.themeだけを見ていた
    ///   ので、ui.themeだけを設定するとコードの配色が取り残されていた）
    /// - viewer.syntax_theme_file → syntectの再構築（themeと同じ経路）
    /// - viewer.tab_width → configのコピー + refresh_viewer + refresh_diff
    /// - diff.word_diff → configのコピー + refresh_diff
    /// - diff.default_view → diff_state.view_mode + refresh_diff
    /// - general.decoration → configのコピー（毎フレーム直接描画される）
    /// - layout.* → configのコピー。LayoutCacheは自動的に無効化される
    ///
    /// viewer.word_wrapはadopt_appearance経由でconfigにはコピーされるが、
    /// AppearanceSnapshotには含まれておらず、描画経路が実装されるまでは
    /// 描画への影響を持たない。
    pub fn apply_appearance(&mut self, new: &config::Config) {
        // UI / シンタックステーマ
        let new_theme_name = super::resolve_theme_name(new);
        let new_high_contrast = new.ui.high_contrast;
        if new_theme_name != self.theme_sel.name
            || new_high_contrast != self.theme_sel.high_contrast
        {
            self.theme = super::build_theme(&new_theme_name, new_high_contrast);
            self.theme_sel.name = new_theme_name;
            self.theme_sel.high_contrast = new_high_contrast;
        }

        // diff表示モード
        // view_modeを直接適用する。diff_state.view_modeが書き込まれるのは
        // DiffState::newとここだけで、実行時のインタラクティブな切り替えは
        // 無いので、上書きしても安全。
        self.diff_state.view_mode = crate::diff_state::DiffViewMode::from(new.diff.default_view);

        // すべてのライブ設定フィールドをコピーする（再起動が必要なフィールドに
        // 対しては何もしない）。LayoutCacheはレイアウト比率をキーにしており
        // 変化を自動検出して次のフレームで再計算するので、明示的な無効化は
        // 不要。
        self.config.adopt_appearance(new);

        // syntectテーマの再構築。テーマ名もsyntax_theme_fileも self.config から
        // 読むので、adopt_appearance の後に呼ぶ必要がある。
        self.rebuild_syntect_theme();

        // Claude/Shellの分割はconfigから種を取るランタイムフィールドなので、
        // layout.terminal_split_pctへの外部からの編集がライブに反映される
        // よう再同期する。自分自身のリサイズ操作による書き込みはここには
        // 決して届かない — それらはappearanceスナップショットを変えない
        // ままにするので、reload_appearance_configが先に早期リターンする。
        self.layout.terminal_split_pct = self
            .config
            .layout
            .terminal_split_pct
            .clamp(Self::TERMINAL_SPLIT_MIN, Self::TERMINAL_SPLIT_MAX);

        // tab_width / word_diffを反映するため、viewerのファイルツリーと
        // diffをリフレッシュする。refresh_viewerは無条件にrehighlight_viewer
        // を呼ぶので、この呼び出しの一部として新しいsyntectテーマが開いている
        // ファイルに適用される。
        self.refresh_viewer();
        self.refresh_diff();

        // 全体の再描画を発生させる。
        self.dirty.mark_all();
    }

    /// 設定ファイルを再読み込みし、外観に関する変更を適用する。
    ///
    /// 設定ファイルが存在しない場合（削除してから書き込むアトミック保存に
    /// よるremoveイベントなど）はガードする: 読み込みをスキップし、
    /// Config::load()がデフォルトファイルを書き込んでユーザーの編集途中の
    /// 内容を上書きしてしまうのを避ける。
    ///
    /// ~/.config/conductor/config.tomlを読み込む。パースエラーの場合は
    /// エラーメッセージをフラッシュし、実行中の設定を変更せずに戻る。
    ///
    /// 外観フィールドと再起動が必要なフィールドのどちらが変わったかを
    /// 計算する。どちらも変わっていない（真の無変化）場合は、何もせず
    /// 静かに戻る。これはアプリ内テーマピッカーによる自己書き込みループを
    /// 吸収するガードでもある。
    ///
    /// 再起動が必要なフィールドが変わっていれば警告をフラッシュする。
    /// 外観フィールドが変わっていればapply_appearanceを呼び、
    /// （再起動警告を出していない場合は）情報メッセージをフラッシュする。
    pub fn reload_appearance_config(&mut self) {
        // ガード: ファイルがちょうど削除された直後（アトミックなエディタ
        // 保存によるremoveイベント）ならスキップする。ファイルが無い状態で
        // Config::load()を呼ぶとデフォルトを書き込んでConfig::default()を
        // 返してしまい、ユーザーの作業を上書きしてしまう。
        if !config::config_file_path().exists() {
            return;
        }

        let new = match config::Config::load() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("config reload: failed to parse config file: {e}");
                self.set_status(
                    format!("Config error — kept previous settings: {e}"),
                    super::StatusLevel::Error,
                );
                return;
            }
        };

        let appearance_changed = new.appearance_snapshot() != self.config.appearance_snapshot();
        let restart_changed = config::has_restart_changes(&self.config, &new);

        // 真の無変化: 何も変わっていない。これはアプリ内テーマピッカーからの
        // ファイルシステムイベントを吸収する（ui.themeは外観専用フィールド
        // なので、ピッカーが実行中の設定にすでに反映済みのテーマを永続化
        // した場合、両方のフラグがfalseになる）。
        if !appearance_changed && !restart_changed {
            return;
        }

        if restart_changed {
            self.set_status(
                String::from("Config updated — some changes require a restart to take effect"),
                super::StatusLevel::Warning,
            );
        }

        if appearance_changed {
            self.apply_appearance(&new);
            if !restart_changed {
                self.set_status_info(String::from("Config reloaded"));
            }
        }
    }
}
