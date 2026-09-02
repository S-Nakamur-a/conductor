//! 埋め込みの既定バインディングにユーザの [keybinds] を重ね、KeyEvent を Action に解決する。

use crossterm::event::KeyEvent;
use keymap_suite::{ActionName, KeyInput, Keymap, Loaded, resolve_layered};

use super::action::Action;
use super::context::KeyContext;
use super::warning::KeybindWarning;

pub(crate) const DEFAULT_KEYBINDS: &str = include_str!("default_keybinds.toml");

pub struct KeyMap {
    loaded: Loaded<Action>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self::with_warnings(&toml::Table::new()).0
    }
}

impl KeyMap {
    /// 既定にユーザの [keybinds] テーブルを重ねて構築する。ユーザ設定の問題は
    /// 致命的にせず警告として返すので、呼び出し側がそれを表示できる。
    pub fn with_warnings(user: &toml::Table) -> (Self, Vec<KeybindWarning>) {
        let defaults = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_name)
            .expect("embedded default keybinds must be valid TOML");
        let mut warnings = Vec::new();
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

    /// コンテキストのレイヤー、次にグローバルの順で引く。どちらにも無いキーは None
    /// (呼び出し側はそのまま通す)。PTY を持つコンテキストでは、そこで発火しない
    /// アクションも None にして、チョードを内側のプログラムへ届ける。
    pub fn resolve(&self, key: &KeyEvent, context: KeyContext) -> Option<Action> {
        let input = KeyInput::try_from(*key).ok()?;
        resolve_layered(self.chain(context), &input)
            .copied()
            .filter(|action| fires_in(*action, context))
    }

    /// コンテキストで action を発火させる全チョードの表示文字列 (keymap-core の
    /// 正規形。設定の文法に戻せる)。resolve と同じチェーンを逆引きするので、
    /// ヘルプが発火しないチョードを載せることはない。
    pub fn keys_for_action(&self, context: KeyContext, action: Action) -> Vec<String> {
        if !fires_in(action, context) {
            return Vec::new();
        }
        sorted_keys(
            self.chain(context)
                .into_iter()
                .flat_map(|layer| keymap_suite::keys_for_action(layer, &action)),
        )
    }

    /// コンテキスト自身のレイヤーだけで action に束縛されたチョード。グローバル
    /// からの到達を含めないので、「このパネル固有か」を呼び出し側が区別できる。
    pub fn keys_in_layer(&self, context: KeyContext, action: Action) -> Vec<String> {
        match self.layer(context) {
            Some(layer) => sorted_keys(keymap_suite::keys_for_action(layer, &action)),
            None => Vec::new(),
        }
    }

    fn layer(&self, context: KeyContext) -> Option<&Keymap<Action>> {
        self.loaded.layers.get(context.layer_name())
    }

    fn chain(&self, context: KeyContext) -> Vec<&Keymap<Action>> {
        let mut chain = Vec::with_capacity(2);
        if context != KeyContext::Global {
            chain.extend(self.layer(context));
        }
        chain.push(self.loaded.global());
        chain
    }
}

fn fires_in(action: Action, context: KeyContext) -> bool {
    !context.forwards_to_pty() || action.fires_in_terminal()
}

fn sorted_keys<'a>(inputs: impl IntoIterator<Item = &'a KeyInput>) -> Vec<String> {
    let mut keys: Vec<String> = inputs.into_iter().map(ToString::to_string).collect();
    keys.sort();
    keys.dedup();
    keys
}

fn warn_unknown_layers(overlay: &Loaded<Action>, warnings: &mut Vec<KeybindWarning>) {
    for (name, layer) in &overlay.layers {
        if name == keymap_suite::GLOBAL_LAYER || layer.is_empty() {
            continue;
        }
        if KeyContext::PANELS.iter().all(|c| c.layer_name() != name) {
            warnings.push(KeybindWarning::UnknownLayer {
                layer: name.clone(),
            });
        }
    }
}

/// 空かパースできなければ None (上書きなし)。
/// suite に渡すのは型ではなく TOML テキスト。conductor と suite で toml crate の
/// バージョンが食い違っても、境界がテキストなら影響しない。
fn parse_user_keybinds(
    user: &toml::Table,
    warnings: &mut Vec<KeybindWarning>,
) -> Option<Loaded<Action>> {
    if user.is_empty() {
        return None;
    }
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
            // 残りはシーケンス (多打鍵) に関する警告で、conductor はシーケンスを使わない。
            _ => {}
        }
    }
}
