//! 保存したターミナル出力の一覧。左に記録、右に本文。

use conductor_core::review_store::SessionHistory;
use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::command::CommandId;
use crate::effect::Effect;
use crate::modal::picker::{Cursor, scroll_for};
use crate::task::Task;
use crate::workspace::Ctx;

#[derive(Debug, Default)]
pub struct HistoryBrowser {
    pub cursor: Cursor,
    pub query: TextInput,
    /// 検索欄にキーが向いているか。`/` で入り、enter か esc で出る。
    searching: bool,
    records: Vec<SessionHistory>,
}

impl HistoryBrowser {
    pub fn open() -> (Self, Effect) {
        (
            Self::default(),
            Effect::Spawn(Task::ListHistory {
                query: String::new(),
            }),
        )
    }

    pub fn install(&mut self, records: Vec<SessionHistory>) {
        self.records = records;
        self.cursor.selected = 0;
    }

    pub fn selected(&self) -> Option<&SessionHistory> {
        self.records.get(self.cursor.selected)
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        if self.searching {
            return self.search_key(key);
        }
        if self.cursor.navigate(ctx.keymap, key, self.records.len()) {
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => vec![Effect::PopModal],
            KeyCode::Char('/') => {
                self.searching = true;
                self.query.clear();
                Vec::new()
            }
            KeyCode::Char('s') => vec![Effect::Command(CommandId::SaveSessionHistory)],
            _ => Vec::new(),
        }
    }

    fn search_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Enter => {
                self.searching = false;
                vec![Effect::Spawn(Task::ListHistory {
                    query: self.query.text().to_string(),
                })]
            }
            KeyCode::Esc => {
                self.searching = false;
                self.query.clear();
                Vec::new()
            }
            _ => {
                self.query.handle_key(key);
                Vec::new()
            }
        }
    }
}

pub fn title() -> String {
    "Saved Terminal Output  j/k move  / search  s save current  esc close".into()
}

pub fn lines(browser: &HistoryBrowser, ctx: &Ctx, area: Rect) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let mut lines = Vec::new();
    if browser.searching {
        lines.push(Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.accent)),
            Span::styled(
                crate::modal::input::with_caret(&browser.query, area.width as usize).join(""),
                Style::default().fg(theme.fg),
            ),
        ]));
        lines.push(Line::from(""));
    }
    if browser.records.is_empty() {
        lines.push(Line::styled(
            "  no saved output",
            Style::default().fg(theme.muted),
        ));
        return lines;
    }

    // 一覧と本文を上下に分ける。左右に割ると保存した出力の行が折り返しで潰れる。
    let listed = ((area.height as usize).saturating_sub(lines.len() + 4) / 2).max(1);
    let start = scroll_for(browser.cursor.selected, browser.records.len(), listed);
    for (i, record) in browser.records.iter().enumerate().skip(start).take(listed) {
        lines.push(crate::list::row_line(
            vec![
                Span::styled(
                    format!(" [{}] ", badge(&record.kind)),
                    Style::default().fg(theme.info),
                ),
                Span::styled(
                    format!("{:<10}", record.label),
                    Style::default().fg(theme.fg),
                ),
                Span::styled(
                    format!("{:<20}", record.worktree),
                    Style::default().fg(theme.success),
                ),
                Span::styled(record.saved_at.clone(), Style::default().fg(theme.muted)),
            ],
            theme,
            i == browser.cursor.selected,
            true,
        ));
    }
    lines.push(Line::styled(
        "\u{2500}".repeat(area.width.saturating_sub(2) as usize),
        Style::default().fg(theme.border_unfocused),
    ));
    if let Some(record) = browser.selected() {
        let room = (area.height as usize).saturating_sub(lines.len() + 2);
        lines.extend(
            record
                .output_text
                .lines()
                .rev()
                .take(room)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|line| Line::styled(line.to_string(), Style::default().fg(theme.fg))),
        );
    }
    lines
}

fn badge(kind: &str) -> &str {
    match kind {
        "claude_code" => "CC",
        "shell" => "SH",
        _ => "??",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn record(label: &str) -> SessionHistory {
        SessionHistory {
            worktree: "main".into(),
            label: label.into(),
            kind: "claude_code".into(),
            output_text: format!("output of {label}"),
            saved_at: "2026-09-03".into(),
        }
    }

    fn browser() -> (HistoryBrowser, Workspace) {
        let (mut browser, _) = HistoryBrowser::open();
        browser.install(vec![record("CC:1"), record("CC:2")]);
        (browser, Workspace::for_test())
    }

    #[test]
    fn 選択が本文を選ぶ() {
        let (mut browser, ws) = browser();
        assert_eq!(browser.selected().unwrap().label, "CC:1");
        browser.update(key(KeyCode::Char('j')), &ws.ctx());
        assert_eq!(browser.selected().unwrap().label, "CC:2");
    }

    #[test]
    fn 検索はenterで問い合わせescで取り消す() {
        let (mut browser, ws) = browser();
        browser.update(key(KeyCode::Char('/')), &ws.ctx());
        for c in "parser".chars() {
            browser.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        assert_eq!(browser.query.text(), "parser");
        let effects = browser.update(key(KeyCode::Enter), &ws.ctx());
        let [Effect::Spawn(Task::ListHistory { query })] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(query, "parser");

        browser.update(key(KeyCode::Char('/')), &ws.ctx());
        browser.update(key(KeyCode::Char('x')), &ws.ctx());
        browser.update(key(KeyCode::Esc), &ws.ctx());
        assert_eq!(browser.query.text(), "");
        assert_eq!(
            browser.update(key(KeyCode::Esc), &ws.ctx()),
            vec![Effect::PopModal],
            "検索を抜けたあとの esc は閉じる"
        );
    }

    #[test]
    fn sは保存コマンドを実行口へ渡す() {
        let (mut browser, ws) = browser();
        let effects = browser.update(key(KeyCode::Char('s')), &ws.ctx());
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Command(CommandId::SaveSessionHistory)]
            ),
            "{effects:?}"
        );
    }
}
