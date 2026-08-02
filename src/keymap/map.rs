//! KeyMap — 与えられた KeyContext に対して KeyEvent を Action に解決する。
//! 埋め込みのデフォルトと、ユーザの [keybinds] 設定オーバーレイをマージして
//! 構築する。

use crossterm::event::KeyEvent;
use keymap_suite::{ActionName, KeyInput, Keymap, Loaded, resolve_layered};

use super::action::Action;
use super::context::{KeyContext, PANEL_CONTEXTS};
use super::warning::KeybindWarning;

// KeyMap

/// 埋め込みのデフォルトバインディング（keymap-suite の key→action TOML）。
/// スキーマはそのファイルを参照。[keybinds] 配下でユーザが書ける内容の基準になる。
pub(crate) const DEFAULT_KEYBINDS: &str = include_str!("../default_keybinds.toml");

pub struct KeyMap {
    /// マージ済みのキーマップ: デフォルト（default_keybinds.toml）にユーザの
    /// [keybinds] を keymap_suite::merge で重ねたもの。layers マップはレイヤー名
    /// をキーにしており、KeyContext::layer_name がイベントごとに1つを選び、
    /// global() は最後に参照される。facade 自身の Loaded 値をそのまま保持する
    /// （バケットを詰め替えたりしない）のが suite が意図する形。
    loaded: Loaded<Action>,
}

impl KeyMap {
    /// デフォルトとユーザの [keybinds] 設定テーブルから KeyMap を構築し、
    /// 警告は捨てる。アプリ本体は警告を表示するため with_warnings を使うので、
    /// この簡易コンストラクタはテスト専用。
    #[cfg(test)]
    pub fn new(user: &toml::Table) -> Self {
        Self::with_warnings(user).0
    }

    /// KeyMap を構築し、ユーザ設定内で見つかった致命的でない問題を返す。
    /// 呼び出し側がそれを表示できるようにする（アプリは起動時にフラッシュ表示する）。
    pub fn with_warnings(user: &toml::Table) -> (Self, Vec<KeybindWarning>) {
        let mut warnings = Vec::new();

        // 1. 埋め込みのデフォルト — マージの土台。リポジトリ内で書かれている
        //    ものなので、警告が出るのはビルドのバグである: debug ではその場で
        //    落ち、ユーザには決して届かない。
        let defaults = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_name)
            .expect("embedded default keybinds must be valid TOML");
        debug_assert!(
            defaults.warnings.is_empty(),
            "default keybinds produced warnings: {:?}",
            defaults.warnings
        );
        for w in &defaults.warnings {
            log::error!("default keybinds produced a warning (bug): {w:?}");
        }

        // 2. ユーザの [keybinds] オーバーレイをパースし、デフォルトの上に
        //    マージする。merge がチョードごとの上書きと = false のトゥームストーン
        //    の適用を行う。ここでは本当に問題のあるものだけを警告として残す
        //    （override/unbind の注記は情報であって警告ではない）。
        let loaded = match parse_user_keybinds(user, &mut warnings) {
            Some(overlay) => {
                warn_unknown_layers(&overlay, &mut warnings);
                let merged = keymap_suite::merge(defaults, overlay);
                collect_warnings(&merged.output.warnings, &mut warnings);
                merged.output
            }
            None => defaults,
        };

        (KeyMap { loaded }, warnings)
    }

    /// あるコンテキストのアクティブなレイヤーチェーン: そのコンテキスト自身の
    /// レイヤー（存在し、かつ Global でない場合）を先に、その後に常時有効な
    /// グローバルレイヤーを続ける。これは、suite が呼び出し側に組み立てさせる
    /// イベントごとのスタックである。
    fn chain(&self, context: KeyContext) -> Vec<&Keymap<Action>> {
        let global = self.loaded.global();
        if context == KeyContext::Global {
            return vec![global];
        }
        match self.loaded.layers.get(context.layer_name()) {
            Some(layer) => vec![layer, global],
            None => vec![global],
        }
    }

    /// 与えられたコンテキストでキーイベントをアクションに解決する。まずコンテキスト
    /// のレイヤーが参照され、次にグローバルレイヤーが参照される。解決不能な
    /// キーイベントや完全な不一致は None を返す（呼び出し側はキーをそのまま通す）。
    ///
    /// terminal コンテキストでは、terminal で発火しないアクション
    /// （Action::fires_in_terminal）は None に解決され、そのチョードは PTY に
    /// 届く — グローバルへのフォールバックは残るが、terminal が奪うべきでない
    /// グローバルバインドのアクション（quit、switch-repo、…）はここでフィルタ
    /// され、ディスパッチャ側の許可リストによるものではない。
    pub fn resolve(&self, key: &KeyEvent, context: KeyContext) -> Option<Action> {
        let input = KeyInput::try_from(*key).ok()?;
        let action = resolve_layered(self.chain(context).iter().copied(), &input).copied()?;
        // エディタパネルは terminal とまったく同じように自分の PTY へキーを転送
        // するので、同じ「terminal で発火するアクションだけを奪う」フィルタに
        // 従う — それ以外（Esc、Ctrl+G、…）は手つかずのまま vim/emacs に届く。
        if matches!(context, KeyContext::Terminal | KeyContext::Editor)
            && !action.fires_in_terminal()
        {
            return None;
        }
        Some(action)
    }

    /// あるコンテキスト（コンテキストのレイヤーとグローバルレイヤー）内で、
    /// あるアクションにバインドされているすべてのキーの表示文字列。ヘルプ
    /// 画面向け。文字列は keymap-core の正規形式（例: "ctrl+d"、"down"、"G"）
    /// で、設定の文法に逆変換できる。
    pub fn keys_for_action(&self, context: KeyContext, action: Action) -> Vec<String> {
        // 表示するヘルプを resolve と一致させておく: terminal と editor の
        // コンテキストでは、そこで発火しないグローバルバインドのアクション
        // には有効なチョードがない。
        if matches!(context, KeyContext::Terminal | KeyContext::Editor)
            && !action.fires_in_terminal()
        {
            return Vec::new();
        }

        // resolve が参照するのと同じチェーンを逆にたどるので、レンダリング
        // されたヘルプが実際には発火しないチョードを宣伝することは決してない。
        let mut keys: Vec<String> = self
            .chain(context)
            .iter()
            .flat_map(|layer| keymap_suite::keys_for_action(layer, &action))
            .map(|input| input.to_string())
            .collect();

        keys.sort();
        keys.dedup();
        keys
    }

    /// コンテキスト自身のレイヤーだけにバインドされているキー — keys_for_action
    /// と違い、グローバルレイヤーを含めない。「このパネルにバインドされて
    /// いる」のか「グローバルにバインドされていて、ここからも単に到達できる」
    /// だけなのかを呼び出し側が区別できるようにする（コマンドパレットの
    /// スコープ絞り込みに使う）。
    pub fn keys_in_layer(&self, context: KeyContext, action: Action) -> Vec<String> {
        let layer = if context == KeyContext::Global {
            self.loaded.global()
        } else {
            match self.loaded.layers.get(context.layer_name()) {
                Some(layer) => layer,
                None => return Vec::new(),
            }
        };
        let mut keys: Vec<String> = keymap_suite::keys_for_action(layer, &action)
            .into_iter()
            .map(|input| input.to_string())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }
}

/// ユーザの [keybinds.layers.<name>] のうち、どの KeyContext の名前にも
/// 一致しないものについて警告する — そのバインディングはマージはされるが
/// 決して参照されない。ローダが常に注入する空の GLOBAL_LAYER はスキップ
/// されるので、本当に未知で空でない名前付きレイヤーだけが警告になる。
fn warn_unknown_layers(overlay: &Loaded<Action>, warnings: &mut Vec<KeybindWarning>) {
    for (name, layer) in &overlay.layers {
        if name == keymap_suite::GLOBAL_LAYER || layer.is_empty() {
            continue;
        }
        if PANEL_CONTEXTS.iter().all(|c| c.layer_name() != name) {
            warnings.push(KeybindWarning::UnknownLayer {
                layer: name.clone(),
            });
        }
    }
}

/// ユーザの [keybinds] テーブルを keymap-suite のオーバーレイにパースする。
/// テーブルが空かパースできない場合は None（上書きなし）を返す。パース失敗は
/// KeybindWarning::InvalidConfig として記録し、アプリがユーザにカスタマイズが
/// 無視されたことを伝えられるようにする。
fn parse_user_keybinds(
    user: &toml::Table,
    warnings: &mut Vec<KeybindWarning>,
) -> Option<Loaded<Action>> {
    if user.is_empty() {
        return None;
    }

    // keymap-suite は独立したドキュメントをパースするので、[keybinds]
    // サブツリーだけを TOML テキストとして再出力する（Conductor 側の toml と
    // suite 側の toml でバージョンが異なる可能性があるため、両者の間の
    // インターフェースは型ではなくテキストにしてある）。
    let toml_text = match toml::to_string(user) {
        Ok(text) => text,
        Err(e) => {
            warnings.push(KeybindWarning::InvalidConfig {
                detail: e.to_string(),
            });
            return None;
        }
    };

    match keymap_suite::from_toml_str(&toml_text, Action::from_name) {
        Ok(build) => Some(build),
        Err(e) => {
            warnings.push(KeybindWarning::InvalidConfig {
                detail: format!(
                    "{e} (note: the keybind format is now key→action under \
                     [keybinds.keys] / [keybinds.layers.*]; the old \
                     [keybinds.<context>] action→key tables are no longer read)"
                ),
            });
            None
        }
    }
}

/// keymap-suite の警告のうち Conductor が関心を持つものを、自前の警告型に
/// 変換する。Conductor が使わないシーケンス関連のバリアントは捨てる。
fn collect_warnings(from: &[keymap_suite::Warning], into: &mut Vec<KeybindWarning>) {
    for w in from {
        match w {
            keymap_suite::Warning::UnknownAction { key, action } => {
                into.push(KeybindWarning::UnknownAction {
                    key: key.clone(),
                    action: action.clone(),
                });
            }
            keymap_suite::Warning::Conflict { chord, .. } => {
                into.push(KeybindWarning::Conflict {
                    chord: chord.clone(),
                });
            }
            // PrefixShadow / EmptySequence / SequenceShadow はシーケンスに
            // 関するもので、Conductor は使わない。Warning は #[non_exhaustive]。
            _ => {}
        }
    }
}
