//! 全文検索。パターンを打つ段と結果の木を歩く段があり、tab で行き来する。
//!
//! 検索そのものはディスクを読むので svc に投げる。通し番号を添えて、打鍵の途中で
//! 追い越された結果を捨てる。

use std::time::{Duration, Instant};

use conductor_core::grep_search::{GrepMatch, MAX_RESULTS};
use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::{Cursor, scroll_for};
use crate::search_tree::{Row, SearchTree};
use crate::task::Task;
use crate::workspace::{Ctx, StatusLevel, Workspace};

/// 打鍵が止まってから検索を始めるまで。
const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Default)]
pub struct Grep {
    pub query: TextInput,
    pub cursor: Cursor,
    /// キーがパターン欄に向いているか。既定は true。
    input_focused: bool,
    regex: bool,
    case_sensitive: bool,
    results: SearchTree,
    seq: u64,
    running: bool,
    due: Option<Instant>,
}

impl Grep {
    pub fn open() -> Self {
        Self {
            input_focused: true,
            ..Self::default()
        }
    }

    /// 締切を過ぎていれば検索を始める。メインループが毎フレーム呼ぶ。
    pub fn tick(&mut self, root: &std::path::Path) -> Vec<Effect> {
        if self.due.is_none_or(|due| Instant::now() < due) {
            return Vec::new();
        }
        self.due = None;
        self.seq += 1;
        self.running = true;
        vec![Effect::Spawn(Task::Grep {
            root: root.to_path_buf(),
            query: self.query.text().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            seq: self.seq,
        })]
    }

    pub fn install(&mut self, seq: u64, found: Result<Vec<GrepMatch>, String>) -> Vec<Effect> {
        if seq != self.seq {
            return Vec::new();
        }
        self.running = false;
        match found {
            Ok(matches) => {
                let truncated = matches.len() >= MAX_RESULTS;
                self.results = SearchTree::build(matches);
                self.cursor.selected = 0;
                if truncated {
                    vec![Effect::Status(
                        StatusLevel::Warning,
                        format!("search truncated at {MAX_RESULTS} results"),
                    )]
                } else {
                    Vec::new()
                }
            }
            Err(e) => vec![Effect::Status(StatusLevel::Error, format!("search: {e}"))],
        }
    }

    /// 打鍵のたびに締切を引き直す。空にしたら結果ごと捨てる。
    fn reschedule(&mut self) {
        if self.query.text().is_empty() {
            self.results = SearchTree::default();
            self.cursor.selected = 0;
            self.running = false;
            self.due = None;
            // 通し番号を進めておかないと、飛んでいる検索の結果が空の画面に戻ってくる。
            self.seq += 1;
            return;
        }
        self.due = Some(Instant::now() + DEBOUNCE);
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // 結果を見ているところからは入力欄へ戻るだけ。閉じるのは入力欄から。
            KeyCode::Esc if !self.input_focused => {
                self.input_focused = true;
                return Vec::new();
            }
            KeyCode::Esc => return vec![Effect::PopModal],
            KeyCode::Tab | KeyCode::BackTab => {
                self.input_focused = !self.input_focused;
                return Vec::new();
            }
            KeyCode::Char('r') if ctrl => {
                self.regex = !self.regex;
                self.reschedule();
                return Vec::new();
            }
            KeyCode::Char('i') if ctrl => {
                self.case_sensitive = !self.case_sensitive;
                self.reschedule();
                return Vec::new();
            }
            KeyCode::Down if self.input_focused => {
                self.input_focused = false;
                return Vec::new();
            }
            KeyCode::Enter => return self.activate(),
            _ => {}
        }
        if self.input_focused {
            self.typed(key);
            return Vec::new();
        }
        self.browse(key, ctx)
    }

    /// 一致の行なら開き、見出しの行なら開閉する。
    fn activate(&mut self) -> Vec<Effect> {
        let Some(hit) = self.results.hit(self.cursor.selected) else {
            self.results.toggle(self.cursor.selected);
            return Vec::new();
        };
        vec![
            Effect::PopModal,
            Effect::OpenFile {
                path: hit.file_path.clone().into(),
                line: Some(hit.line_number),
                diff: None,
                preview: false,
            },
        ]
    }

    fn typed(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Backspace && key.modifiers.contains(KeyModifiers::SUPER) {
            self.query.delete_to_line_start();
            self.reschedule();
            return;
        }
        if self.query.handle_key(key)
            && matches!(
                key.code,
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
            )
        {
            self.reschedule();
        }
    }

    fn browse(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let len = self.results.visible().len();
        // 畳んだ枝の中に降りない。畳んだのは中を見ないと決めたということ。
        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
            if self.results.is_collapsed(self.cursor.selected)
                && let Some(next) = self.results.next_sibling(self.cursor.selected)
            {
                self.cursor.selected = next;
                return Vec::new();
            }
        } else if matches!(key.code, KeyCode::Left | KeyCode::Char('h')) {
            if self.results.is_collapsed(self.cursor.selected)
                || self.results.hit(self.cursor.selected).is_some()
            {
                if let Some(parent) = self.results.parent(self.cursor.selected) {
                    self.cursor.selected = parent;
                }
            } else {
                self.results.set_collapsed(self.cursor.selected, true);
            }
            return Vec::new();
        } else if matches!(key.code, KeyCode::Right | KeyCode::Char('l')) {
            self.results.set_collapsed(self.cursor.selected, false);
            return Vec::new();
        }
        if self.cursor.navigate(ctx.keymap, key, len) {
            return Vec::new();
        }
        // 結果を見ている最中の文字は入力の続き。打ちながら絞り込める。
        if matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
        ) {
            self.input_focused = true;
            self.typed(key);
        }
        Vec::new()
    }
}

pub fn title(grep: &Grep) -> String {
    let mut modes = Vec::new();
    if grep.regex {
        modes.push("regex");
    }
    if grep.case_sensitive {
        modes.push("case");
    }
    if grep.running {
        modes.push("searching\u{2026}");
    }
    let count = grep.results.match_count();
    format!(
        "Full-text Search \u{b7} {count} hits \u{b7} ctrl+r regex \u{b7} ctrl+i case{}",
        if modes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", modes.join(" "))
        }
    )
}

pub fn lines(grep: &Grep, ws: &Workspace, area: Rect) -> Vec<Line<'static>> {
    let ctx = &ws.ctx();
    let theme = ctx.theme;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                if grep.input_focused {
                    crate::modal::input::with_caret(&grep.query, area.width as usize).join("")
                } else {
                    grep.query.text().to_string()
                },
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(""),
    ];

    let rows = grep.results.visible();
    if rows.is_empty() {
        lines.push(Line::styled(
            "  no matches",
            Style::default().fg(theme.muted),
        ));
        return lines;
    }
    let height = (area.height as usize).saturating_sub(4);
    let start = scroll_for(grep.cursor.selected, rows.len(), height);
    for (i, row) in rows.iter().enumerate().skip(start).take(height) {
        let selected = i == grep.cursor.selected && !grep.input_focused;
        lines.push(crate::list::row_line(
            row_spans(row, grep, ctx),
            theme,
            selected,
            true,
        ));
    }
    lines
}

fn row_spans(row: &Row, grep: &Grep, ctx: &Ctx) -> Vec<Span<'static>> {
    let theme = ctx.theme;
    let indent = " ".repeat(row.depth() * 2 + 1);
    match row {
        Row::Dir {
            name,
            matches,
            path,
            ..
        }
        | Row::File {
            name,
            matches,
            path,
            ..
        } => {
            let folded = grep.results.collapsed_key(path);
            let (arrow, fg) = match row {
                Row::Dir { .. } => (if folded { "\u{25b8} " } else { "\u{25be} " }, theme.info),
                _ => (if folded { "\u{25b8} " } else { "\u{25be} " }, theme.fg),
            };
            vec![
                Span::styled(format!("{indent}{arrow}{name}"), Style::default().fg(fg)),
                Span::styled(format!(" ({matches})"), Style::default().fg(theme.muted)),
            ]
        }
        Row::Hit { index, .. } => {
            let Some(hit) = grep.results.match_at(*index) else {
                return Vec::new();
            };
            vec![
                Span::styled(
                    format!("{indent}{:>5} ", hit.line_number),
                    Style::default().fg(theme.hint),
                ),
                Span::styled(
                    hit.line_content.trim_start().to_string(),
                    Style::default().fg(theme.fg).add_modifier(Modifier::DIM),
                ),
            ]
        }
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

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn hit(path: &str, line: usize) -> GrepMatch {
        GrepMatch {
            file_path: path.into(),
            line_number: line,
            line_content: "hit".into(),
            match_start: 0,
            match_end: 1,
        }
    }

    /// 打鍵と結果の流し込みまでを 1 つにまとめる。以降のテストはここから始める。
    fn loaded(query: &str, matches: Vec<GrepMatch>) -> (Grep, Workspace) {
        let ws = Workspace::for_test();
        let mut grep = Grep::open();
        for c in query.chars() {
            grep.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        let seq = grep.seq + 1;
        grep.seq = seq;
        grep.install(seq, Ok(matches));
        grep.input_focused = false;
        (grep, ws)
    }

    #[test]
    fn 打鍵は締切を置き締切前は何も投げない() {
        let ws = Workspace::for_test();
        let mut grep = Grep::open();
        grep.update(key(KeyCode::Char('a')), &ws.ctx());
        assert!(grep.due.is_some());
        assert!(grep.tick(std::path::Path::new("/tmp")).is_empty());

        grep.due = Some(Instant::now());
        let effects = grep.tick(std::path::Path::new("/tmp"));
        assert!(
            matches!(effects.as_slice(), [Effect::Spawn(Task::Grep { .. })]),
            "{effects:?}"
        );
    }

    #[test]
    fn 空にすると結果ごと捨て古い結果も受け取らない() {
        let (mut grep, ws) = loaded("ab", vec![hit("a.rs", 1)]);
        let stale = grep.seq;
        grep.input_focused = true;
        grep.update(key(KeyCode::Backspace), &ws.ctx());
        grep.update(key(KeyCode::Backspace), &ws.ctx());

        assert!(grep.results.is_empty());
        assert!(grep.due.is_none());
        grep.install(stale, Ok(vec![hit("a.rs", 1)]));
        assert!(grep.results.is_empty(), "追い越された結果は捨てる");
    }

    #[test]
    fn 一致の行はファイルを開き見出しの行は開閉する() {
        let (mut grep, ws) = loaded("x", vec![hit("src/a.rs", 7)]);
        // 0:src/ 1:a.rs 2:hit
        grep.cursor.selected = 0;
        assert!(grep.update(key(KeyCode::Enter), &ws.ctx()).is_empty());
        assert_eq!(grep.results.visible().len(), 1, "畳んだ");

        grep.update(key(KeyCode::Enter), &ws.ctx());
        grep.cursor.selected = 2;
        let effects = grep.update(key(KeyCode::Enter), &ws.ctx());
        let [Effect::PopModal, Effect::OpenFile { path, line, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!((path.to_str().unwrap(), *line), ("src/a.rs", Some(7)));
    }

    #[test]
    fn 結果を見ている間の文字は入力に戻る() {
        let (mut grep, ws) = loaded("x", vec![hit("a.rs", 1)]);
        grep.update(key(KeyCode::Char('y')), &ws.ctx());
        assert!(grep.input_focused);
        assert_eq!(grep.query.text(), "xy");
    }

    #[test]
    fn escは結果から入力へ戻ってから閉じる() {
        let (mut grep, ws) = loaded("x", vec![hit("a.rs", 1)]);
        assert!(grep.update(key(KeyCode::Esc), &ws.ctx()).is_empty());
        assert!(grep.input_focused);
        assert_eq!(
            grep.update(key(KeyCode::Esc), &ws.ctx()),
            vec![Effect::PopModal]
        );
    }

    #[test]
    fn 検索モードの切替は検索をやり直す() {
        let (mut grep, ws) = loaded("x", vec![hit("a.rs", 1)]);
        for chord in [ctrl('r'), ctrl('i')] {
            grep.due = None;
            grep.update(chord, &ws.ctx());
            assert!(grep.due.is_some(), "{chord:?}");
        }
        assert!(grep.regex && grep.case_sensitive);
    }

    #[test]
    fn 畳んだ枝の中には降りない() {
        let (mut grep, ws) = loaded("x", vec![hit("a.rs", 1), hit("a.rs", 2), hit("b.rs", 3)]);
        // 0:a.rs 1:hit 2:hit 3:b.rs 4:hit
        grep.cursor.selected = 0;
        grep.results.set_collapsed(0, true);
        grep.update(key(KeyCode::Down), &ws.ctx());
        assert_eq!(grep.cursor.selected, 1);
        assert!(matches!(grep.results.row(1), Some(Row::File { .. })));
    }

    #[test]
    fn 左は畳みもう一度押すと親へ上がる() {
        let (mut grep, ws) = loaded("x", vec![hit("src/a.rs", 1)]);
        grep.cursor.selected = 2;
        grep.update(key(KeyCode::Left), &ws.ctx());
        assert_eq!(grep.cursor.selected, 1, "一致からはファイルへ");
        grep.update(key(KeyCode::Left), &ws.ctx());
        assert!(grep.results.is_collapsed(1));
        grep.update(key(KeyCode::Left), &ws.ctx());
        assert_eq!(grep.cursor.selected, 0, "畳んであれば親へ");
    }
}
