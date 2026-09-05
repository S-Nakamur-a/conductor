//! 語に対してできることの一覧。構文層 (tree-sitter) が名前で引いた行き先だけを載せる。
//!
//! 索引の答えを混ぜないのは、こちらが名前一致でしかないため。同じ枠に並べると、
//! どちらの強さで読めばよいか分からなくなる。

use conductor_core::symbol_index::Symbol;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::picker::Cursor;
use crate::effect::Effect;
use crate::workspace::Ctx;

#[derive(Debug)]
pub struct SymbolActions {
    symbol: String,
    actions: Vec<Act>,
    cursor: Cursor,
}

#[derive(Debug)]
pub struct Act {
    pub key: char,
    pub label: String,
    pub path: String,
    /// 1 始まり。
    pub line: usize,
}

impl SymbolActions {
    /// 行き先が 1 つも無ければ開かない。
    pub fn build(symbol: &str, defs: &[Symbol], impls: &[Symbol]) -> Option<Self> {
        let mut actions = Vec::new();
        if let Some(first) = defs.first() {
            actions.push(Act {
                key: 'd',
                label: labelled("Go to definition", defs.len()),
                path: first.file_path.clone(),
                line: first.line,
            });
        }
        if let Some(first) = impls.first() {
            actions.push(Act {
                key: 'i',
                label: labelled("Go to implementation", impls.len()),
                path: first.file_path.clone(),
                line: first.line,
            });
        }
        (!actions.is_empty()).then(|| Self {
            symbol: symbol.to_string(),
            actions,
            cursor: Cursor::default(),
        })
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        if self.cursor.navigate(ctx.keymap, key, self.actions.len()) {
            return Vec::new();
        }
        let picked = match key.code {
            KeyCode::Esc => return vec![Effect::PopModal],
            KeyCode::Enter => self.actions.get(self.cursor.selected),
            KeyCode::Char(c) => self.actions.iter().find(|a| a.key == c),
            _ => None,
        };
        let Some(act) = picked else {
            return Vec::new();
        };
        vec![
            Effect::PopModal,
            Effect::JumpTo {
                path: act.path.clone().into(),
                line: act.line,
            },
        ]
    }
}

fn labelled(what: &str, count: usize) -> String {
    if count > 1 {
        format!("{what} ({count} results)")
    } else {
        what.to_string()
    }
}

pub fn title(modal: &SymbolActions) -> String {
    modal.symbol.clone()
}

pub fn lines(modal: &SymbolActions, ctx: &Ctx) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let mut out: Vec<Line<'static>> = modal
        .actions
        .iter()
        .enumerate()
        .map(|(i, act)| {
            let base = if i == modal.cursor.selected {
                Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.fg)
            };
            Line::from(vec![
                Span::styled(
                    format!("[{}] ", act.key),
                    base.fg(theme.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:<30}", act.label), base),
                Span::styled(format!("{}:{}", act.path, act.line), base.fg(theme.muted)),
            ])
        })
        .collect();
    out.push(Line::styled(
        "d/i: jump  \u{b7}  enter: pick  \u{b7}  esc: cancel",
        Style::default().fg(theme.hint),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use conductor_core::symbol_index::{Scope, SymbolKind};
    use crossterm::event::KeyModifiers;

    fn symbol(name: &str, path: &str, line: usize, kind: SymbolKind) -> Symbol {
        Symbol {
            name: name.into(),
            kind,
            scope: Scope::Global,
            file_path: path.into(),
            line,
            parent: None,
        }
    }

    #[test]
    fn 行き先が無ければ開かない() {
        assert!(SymbolActions::build("Foo", &[], &[]).is_none());
    }

    #[test]
    fn 候補の件数は行に出て飛び先は先頭になる() {
        let defs = vec![
            symbol("Foo", "a.rs", 3, SymbolKind::Struct),
            symbol("Foo", "b.rs", 7, SymbolKind::Struct),
        ];
        let impls = vec![symbol("Foo", "c.rs", 11, SymbolKind::Impl)];
        let modal = SymbolActions::build("Foo", &defs, &impls).unwrap();
        assert_eq!(modal.actions[0].label, "Go to definition (2 results)");
        assert_eq!(modal.actions[1].label, "Go to implementation");
        assert_eq!(
            (modal.actions[0].path.as_str(), modal.actions[0].line),
            ("a.rs", 3)
        );
    }

    #[test]
    fn 割り当てキーは一覧の位置に関係なくその行へ飛ぶ() {
        let ws = Workspace::for_test();
        let defs = vec![symbol("Foo", "a.rs", 3, SymbolKind::Struct)];
        let impls = vec![symbol("Foo", "c.rs", 11, SymbolKind::Impl)];
        let mut modal = SymbolActions::build("Foo", &defs, &impls).unwrap();
        let effects = modal.update(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
            &ws.ctx(),
        );
        let [Effect::PopModal, Effect::JumpTo { path, line }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!((path.to_str().unwrap(), *line), ("c.rs", 11));
    }
}
