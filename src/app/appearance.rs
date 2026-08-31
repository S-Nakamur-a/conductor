//! 外観設定（テーマ、シンタックスハイライト、diff表示モード、レイアウト比率）の
//! ランタイムでのテーマ切り替えと、設定ファイルのライブリロード。

use super::App;
use crate::config;

impl App {
    /// UI パレットを差し替え、そこから色を焼き込んだキャッシュを捨てる。
    ///
    /// syntect の作り直しとは分けてある。1 つの早期 return にまとめていた頃は、
    /// viewer.syntax_theme_file を指定していると syntect の id が変わらないため、
    /// UI テーマを切り替えても markdown の色が古いまま残った。
    pub(super) fn install_palette(&mut self, name: String, high_contrast: bool) {
        self.appearance.theme = super::build_theme(&name, high_contrast);
        self.appearance.sel.name = name;
        self.appearance.sel.high_contrast = high_contrast;
        self.appearance.markdown_cache.clear();
        // last_width=0 は幅の変化に関わらず build_lines を走らせるための細工。
        self.reflow.last_width = 0;
        self.reflow.cache.clear();
    }

    /// span の色はキャッシュに焼き込まれるので、テーマだけ差し替えても古い配色が残る。
    /// テーマを触る経路は必ずここを通すこと。
    fn rebuild_syntect_theme(&mut self) {
        let new_id = config::syntax_theme_id(&self.config);
        if new_id == self.appearance.highlight.theme_id {
            return;
        }

        self.appearance.highlight.theme =
            config::syntect_theme_for(&self.config, &self.appearance.highlight.themes);
        self.appearance.highlight.theme_id = new_id;
        // Viewerのキャッシュキーに混ざる。進めないと内容が同じままなので素通りする。
        self.appearance.highlight.generation = self.appearance.highlight.generation.wrapping_add(1);

        // このキャッシュはUIパレットだけを指紋にしているので、シンタックスのみの
        // 変更では自力で無効化できない。
        self.appearance.markdown_cache.clear();

        // last_width=0 は幅の変化に関わらず build_lines を走らせるための細工。
        self.reflow.last_width = 0;
        self.reflow.cache.clear();
    }

    /// アクティブなUIテーマをランタイムで切り替える。UIパレットとsyntectの
    /// 両方を切り替えないと、Viewerのコードの配色が取り残される。
    pub fn set_theme(&mut self, name: &str, persist: bool) {
        self.install_palette(name.to_string(), self.appearance.sel.high_contrast);
        self.config.ui.theme = Some(name.to_string());

        // configの現在値から解決するので ui.theme を書いた後に呼ぶ。
        self.rebuild_syntect_theme();
        self.rehighlight_viewer();
        self.request_redraw();

        if persist && let Err(e) = crate::config::persist_ui_theme(name) {
            log::warn!("failed to persist theme '{name}': {e}");
            self.set_status(
                format!("Theme saved in session but could not write config: {e}"),
                super::StatusLevel::Warning,
            );
        }
    }

    // 設定ファイルのライブリロード

    /// 新しい設定から、外観に関する（ライブリロード可能な）フィールドを適用する。
    ///
    /// 再起動が必要なフィールド（シェル、スクロールバック上限、API設定）をあえて
    /// 手つかずにするのは、毎回 config を読み直す refresh_diff に過渡的な値を
    /// 見せないため。
    ///
    /// viewer.word_wrap は config へコピーされるが AppearanceSnapshot に無く、
    /// 描画経路が実装されるまで効果を持たない。
    pub fn apply_appearance(&mut self, new: &config::Config) {
        // UI / シンタックステーマ
        let new_theme_name = super::resolve_theme_name(new);
        let new_high_contrast = new.ui.high_contrast;
        if new_theme_name != self.appearance.sel.name
            || new_high_contrast != self.appearance.sel.high_contrast
        {
            self.install_palette(new_theme_name, new_high_contrast);
        }

        // 無条件に上書きしてよい。view_mode を書くのは DiffState::new とここだけで、
        // 実行時のインタラクティブな切り替えが無いため。
        self.diff_state.view_mode = crate::diff_state::DiffViewMode::from(new.diff.default_view);

        // LayoutCache はレイアウト比率をキーにしていて変化を自力で検出するので、
        // 明示的な無効化は要らない。
        self.config.adopt_appearance(new);

        // テーマ名も syntax_theme_file も self.config から読むので adopt の後。
        self.rebuild_syntect_theme();

        // 外部エディタからの編集をライブに反映する。自分自身のリサイズ操作は
        // appearance スナップショットを変えないので、ここへは届かない
        // (reload_appearance_config が先に早期リターンする)。
        self.layout.terminal_split_pct = self
            .config
            .layout
            .terminal_split_pct
            .clamp(Self::TERMINAL_SPLIT_MIN, Self::TERMINAL_SPLIT_MAX);

        // refresh_viewer が無条件に rehighlight_viewer を呼ぶので、新しい syntect
        // テーマの適用もここで済む。
        self.refresh_viewer();
        self.refresh_diff();

        self.request_redraw();
    }

    /// ~/.config/conductor/config.toml を読み直し、外観の変更を適用する。
    pub fn reload_appearance_config(&mut self) {
        // アトミック保存の remove イベント直後を弾く。ファイルが無い状態で
        // Config::load() を呼ぶとデフォルトを書き込み、編集中の内容を破壊する。
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

        // テーマピッカー自身の書き込みが起こすイベントをここで吸収する。
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
