//! はい / いいえ の確認。答えの受け方と 2 つのボタンの描き方は [Choice] が 1 つに持ち、
//! 本文が定型で足りる確認は [Confirm] で組む。

use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

impl Answer {
    fn other(self) -> Self {
        match self {
            Self::Yes => Self::No,
            Self::No => Self::Yes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keys {
    /// y / n / Esc と、Enter・選択の移動。
    Buttons,
    /// y / n / Esc だけ。ほかは無視する。
    Letters,
    /// y が Yes (大文字・Ctrl・Alt 付きも)、← で Yes・→ で No を選び Enter で選択中を答える。ほかのキーはどれも No。
    /// h / l を選択に使わないのは、打ちかけの文の l で Yes に倒れ、続く Enter で答えてしまうため。
    YesOnly,
}

/// 選択中のボタンを持つ 2 択。
#[derive(Debug)]
pub struct Choice {
    yes: &'static str,
    no: &'static str,
    selected: Answer,
    keys: Keys,
}

impl Choice {
    pub fn new(default: Answer) -> Self {
        Self {
            yes: "Yes",
            no: "No",
            selected: default,
            keys: Keys::Buttons,
        }
    }

    pub fn labels(mut self, yes: &'static str, no: &'static str) -> Self {
        self.yes = yes;
        self.no = no;
        self
    }

    pub fn key(&mut self, key: KeyEvent) -> Option<Answer> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(Answer::Yes),
            KeyCode::Left if self.keys == Keys::YesOnly => {
                self.selected = Answer::Yes;
                None
            }
            KeyCode::Right if self.keys == Keys::YesOnly => {
                self.selected = Answer::No;
                None
            }
            KeyCode::Enter if self.keys == Keys::YesOnly => Some(self.selected),
            _ if self.keys == Keys::YesOnly => Some(Answer::No),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(Answer::No),
            _ if self.keys == Keys::Letters => None,
            KeyCode::Enter => Some(self.selected),
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                self.selected = self.selected.other();
                None
            }
            _ => None,
        }
    }

    pub fn line(&self, theme: &Theme) -> Line<'static> {
        let button = |label: &str, answer: Answer| {
            let style = if answer == self.selected {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.hint)
            };
            Span::styled(format!(" {label} "), style)
        };
        let keys = match self.keys {
            Keys::Buttons => "   y / n · tab",
            Keys::Letters => "   y / n",
            Keys::YesOnly => "   y / ←→ enter / else: no",
        };
        Line::from(vec![
            button(self.yes, Answer::Yes),
            Span::raw("  "),
            button(self.no, Answer::No),
            Span::styled(keys, Style::default().fg(theme.hint)),
        ])
    }
}

/// 答えで発火する Effect を積んだ確認。開いた側が対象を捕まえたまま作れるよう、
/// 閉包ではなく組み立て済みの Effect を持つ。
#[derive(Debug)]
pub struct Confirm {
    pub title: String,
    pub question: String,
    detail: Vec<String>,
    pub choice: Choice,
    pub on_yes: Vec<Effect>,
}

impl Confirm {
    pub fn ask(question: impl Into<String>, on_yes: Vec<Effect>) -> Self {
        Self::with_default(Answer::Yes, question.into(), on_yes)
    }

    /// 失うものがある確認。Enter を反射で押しても何も起きない。
    pub fn destructive(question: impl Into<String>, on_yes: Vec<Effect>) -> Self {
        Self::with_default(Answer::No, question.into(), on_yes)
    }

    /// 外から出る確認。別の区画で打っていた Enter や h / l が答えにならないよう、
    /// y / n でしか答えられない。
    pub fn letters_only(question: impl Into<String>, on_yes: Vec<Effect>) -> Self {
        let mut confirm = Self::destructive(question, on_yes);
        confirm.choice.keys = Keys::Letters;
        confirm
    }

    /// 別の区画に打つつもりの文が届きうる場所で出す確認。文の中の y 以外の字で閉じるので、
    /// 打ち続けた文が後の y で答えてしまわない。
    pub fn yes_only(question: impl Into<String>, on_yes: Vec<Effect>) -> Self {
        let mut confirm = Self::destructive(question, on_yes);
        confirm.choice.keys = Keys::YesOnly;
        confirm
    }

    fn with_default(default: Answer, question: String, on_yes: Vec<Effect>) -> Self {
        Self {
            title: "Confirm".into(),
            question,
            detail: Vec::new(),
            choice: Choice::new(default),
            on_yes,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn detail(mut self, line: impl Into<String>) -> Self {
        self.detail.push(line.into());
        self
    }

    pub fn labels(mut self, yes: &'static str, no: &'static str) -> Self {
        self.choice = self.choice.labels(yes, no);
        self
    }

    pub fn update(&mut self, key: KeyEvent) -> Vec<Effect> {
        match self.choice.key(key) {
            Some(Answer::Yes) => {
                let mut effects = vec![Effect::PopModal];
                effects.append(&mut self.on_yes);
                effects
            }
            Some(Answer::No) => vec![Effect::PopModal],
            None => Vec::new(),
        }
    }
}

pub fn lines(confirm: &Confirm, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        confirm.question.clone(),
        Style::default().fg(theme.fg),
    )];
    lines.extend(
        confirm
            .detail
            .iter()
            .map(|line| Line::styled(line.clone(), Style::default().fg(theme.hint))),
    );
    lines.push(Line::from(""));
    lines.push(confirm.choice.line(theme));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn answered(effects: &[Effect]) -> bool {
        matches!(effects, [Effect::PopModal, Effect::Quit])
    }

    #[test]
    fn enterは選択中を答え_tabと左右で選択が動く() {
        let mut confirm = Confirm::ask("go?", vec![Effect::Quit]);
        assert!(confirm.update(key(KeyCode::Tab)).is_empty());
        assert!(matches!(
            confirm.update(key(KeyCode::Enter)).as_slice(),
            [Effect::PopModal]
        ));

        let mut confirm = Confirm::ask("go?", vec![Effect::Quit]);
        confirm.update(key(KeyCode::Left));
        confirm.update(key(KeyCode::Right));
        assert!(answered(&confirm.update(key(KeyCode::Enter))));
    }

    #[test]
    fn 失うものがある確認はnoから始まりyは直接効く() {
        let mut confirm = Confirm::destructive("delete?", vec![Effect::Quit]);
        assert!(matches!(
            confirm.update(key(KeyCode::Enter)).as_slice(),
            [Effect::PopModal]
        ));

        let mut confirm = Confirm::destructive("delete?", vec![Effect::Quit]);
        assert!(answered(&confirm.update(key(KeyCode::Char('y')))));
    }

    #[test]
    fn noとescは閉じるだけで何も起こさない() {
        for code in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut confirm = Confirm::ask("go?", vec![Effect::Quit]);
            assert!(
                matches!(confirm.update(key(code)).as_slice(), [Effect::PopModal]),
                "{code:?}"
            );
        }
        let mut confirm = Confirm::ask("go?", vec![Effect::Quit]);
        assert!(confirm.update(key(KeyCode::Char('j'))).is_empty());
    }

    #[test]
    fn 文字だけの確認は選択の移動とenterに答えない() {
        let mut confirm = Confirm::letters_only("analyse?", vec![Effect::Quit]);
        for code in [
            KeyCode::Char('l'),
            KeyCode::Tab,
            KeyCode::Right,
            KeyCode::Enter,
        ] {
            assert!(confirm.update(key(code)).is_empty(), "{code:?}");
        }
        assert!(answered(&confirm.update(key(KeyCode::Char('y')))));
    }

    #[test]
    fn yで答える確認は大文字や修飾付きのyでも答え_左右とenter以外のキーは何も起こさずに閉じる() {
        for code in [
            KeyCode::Char('e'),
            KeyCode::Char('n'),
            KeyCode::Char('l'),
            KeyCode::Char('h'),
            KeyCode::Tab,
            KeyCode::Esc,
        ] {
            let mut confirm = Confirm::yes_only("stop?", vec![Effect::Quit]);
            assert!(
                matches!(confirm.update(key(code)).as_slice(), [Effect::PopModal]),
                "{code:?}"
            );
        }
        for yes in [
            key(KeyCode::Char('y')),
            key(KeyCode::Char('Y')),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT),
        ] {
            let mut confirm = Confirm::yes_only("stop?", vec![Effect::Quit]);
            assert!(answered(&confirm.update(yes)), "{yes:?}");
        }
    }

    #[test]
    fn yで答える確認は左でyes右でnoを選びenterで選択中を答える() {
        use KeyCode::{Left, Right};
        let cases: [(&[KeyCode], bool); 7] = [
            (&[Left], true),
            (&[Right], false),
            (&[Left, Right], false),
            (&[Right, Left], true),
            (&[Left, Left], true),
            (&[Right, Right], false),
            (&[], false),
        ];
        for (arrows, yes) in cases {
            let mut confirm = Confirm::yes_only("stop?", vec![Effect::Quit]);
            for arrow in arrows {
                assert!(confirm.update(key(*arrow)).is_empty(), "{arrows:?}");
            }
            let effects = confirm.update(key(KeyCode::Enter));
            assert_eq!(answered(&effects), yes, "{arrows:?}");
            assert!(matches!(effects.first(), Some(Effect::PopModal)));
        }
    }

    /// 文言が変わるだけでは、どちらにいるか一目で分からない。
    #[test]
    fn 選択中のボタンだけを塗る() {
        let theme = Theme::from_name("dracula");
        let painted = |choice: &Choice| -> Vec<String> {
            choice
                .line(&theme)
                .spans
                .iter()
                .filter(|s| s.style.bg == Some(theme.accent))
                .map(|s| s.content.trim().to_string())
                .collect()
        };
        let mut choice = Choice::new(Answer::Yes).labels("Publish", "Cancel");
        assert_eq!(painted(&choice), ["Publish"]);
        choice.key(key(KeyCode::Tab));
        assert_eq!(painted(&choice), ["Cancel"]);
    }
}
