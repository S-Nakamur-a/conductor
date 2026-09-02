//! 入力を独占するもの。1 バリアント 1 ファイルで状態・update・render を同居させる。
//! スタックの top だけが入力を受け、既定は消費する (IME の変換中グリフを外に漏らさない)。

use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent};

use crate::effect::Effect;
use crate::workspace::Ctx;

#[derive(Debug)]
pub enum Modal {
    Help,
    Prompt(Prompt),
    Confirm(Confirm),
}

/// 1 行のテキスト入力。確定した文字列は `on_submit` が Effect に変える。
#[derive(Debug)]
pub struct Prompt {
    pub title: String,
    pub input: TextInput,
    pub on_submit: fn(String) -> Vec<Effect>,
}

/// y で発火する Effect を積んだ確認。開いた側が対象を捕まえたまま作れるよう、
/// 閉包ではなく組み立て済みの Effect を持つ。
#[derive(Debug)]
pub struct Confirm {
    pub question: String,
    pub on_yes: Vec<Effect>,
}

impl Modal {
    pub fn update(&mut self, key: KeyEvent, _ctx: &Ctx) -> Vec<Effect> {
        match self {
            Modal::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => vec![Effect::PopModal],
                _ => vec![],
            },
            Modal::Prompt(prompt) => match key.code {
                KeyCode::Esc => vec![Effect::PopModal],
                KeyCode::Enter => {
                    let mut effects = vec![Effect::PopModal];
                    effects.extend((prompt.on_submit)(prompt.input.text().to_string()));
                    effects
                }
                _ => {
                    prompt.input.handle_key(key);
                    vec![]
                }
            },
            Modal::Confirm(confirm) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let mut effects = vec![Effect::PopModal];
                    effects.append(&mut confirm.on_yes);
                    effects
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => vec![Effect::PopModal],
                _ => vec![],
            },
        }
    }
}
