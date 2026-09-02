//! 参照・定義・実装の一覧。ファイルごとに畳める。
//!
//! 開いたときに畳んでおくのは 2 つ目以降のファイル。実索引では 1 シンボルが 15 ファイル
//! 63 箇所に散るので、全部開くと見出しが流れて件数が読めない。

use std::collections::HashSet;

use conductor_core::symbol_index::Reference;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::picker::{Cursor, scroll_for};
use crate::effect::Effect;
use crate::workspace::Ctx;

#[derive(Debug)]
pub struct References {
    title: String,
    hits: Vec<Reference>,
    collapsed: HashSet<String>,
    cursor: Cursor,
}

/// 一覧の 1 行。見出しか、その中の 1 件か。
#[derive(Debug, PartialEq, Eq)]
pub enum Row<'a> {
    File {
        path: &'a str,
        count: usize,
        collapsed: bool,
    },
    Hit {
        index: usize,
    },
}

impl References {
    pub fn new(title: String, hits: Vec<Reference>) -> Self {
        let mut files: Vec<&str> = Vec::new();
        for hit in &hits {
            if !files.contains(&hit.file_path.as_str()) {
                files.push(&hit.file_path);
            }
        }
        let collapsed = files.iter().skip(1).map(|f| (*f).to_string()).collect();
        Self {
            title,
            hits,
            collapsed,
            cursor: Cursor::default(),
        }
    }

    /// ファイルごとにまとめた表示行。
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut order: Vec<&str> = Vec::new();
        let mut counts: std::collections::HashMap<&str, Vec<usize>> = Default::default();
        for (i, hit) in self.hits.iter().enumerate() {
            counts
                .entry(hit.file_path.as_str())
                .or_insert_with(|| {
                    order.push(hit.file_path.as_str());
                    Vec::new()
                })
                .push(i);
        }
        let mut rows = Vec::new();
        for path in order {
            let hits = &counts[path];
            let collapsed = self.collapsed.contains(path);
            rows.push(Row::File {
                path,
                count: hits.len(),
                collapsed,
            });
            if !collapsed {
                rows.extend(hits.iter().map(|&index| Row::Hit { index }));
            }
        }
        rows
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let len = self.rows().len();
        if self.cursor.navigate(ctx.keymap, key, len) {
            return Vec::new();
        }
        let picked = match self.rows().get(self.cursor.selected) {
            Some(Row::File {
                path, collapsed, ..
            }) => Ok(((*path).to_string(), *collapsed)),
            Some(Row::Hit { index }) => Err(*index),
            None => return maybe_close(key),
        };
        match key.code {
            KeyCode::Esc => vec![Effect::PopModal],
            KeyCode::Char('h') | KeyCode::Char('l') => {
                let fold = key.code == KeyCode::Char('h');
                match &picked {
                    // 既にその状態なら何もしない。
                    Ok((path, collapsed)) if *collapsed != fold => self.toggle(path),
                    // 1 件の上での h は、そのファイルの見出しへ上がる。
                    Err(index) if fold => {
                        let path = self.hits[*index].file_path.clone();
                        self.select_file(&path);
                    }
                    _ => {}
                }
                Vec::new()
            }
            KeyCode::Enter => match picked {
                Ok((path, _)) => {
                    self.toggle(&path);
                    Vec::new()
                }
                Err(index) => {
                    let hit = &self.hits[index];
                    vec![
                        Effect::PopModal,
                        Effect::JumpTo {
                            path: hit.file_path.clone().into(),
                            line: hit.line,
                        },
                    ]
                }
            },
            _ => Vec::new(),
        }
    }

    /// 畳んだ結果その行が消えるので、選択を見出し自身へ寄せる。
    fn toggle(&mut self, path: &str) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        self.select_file(path);
    }

    fn select_file(&mut self, path: &str) {
        self.cursor.selected = self
            .rows()
            .iter()
            .position(|r| matches!(r, Row::File { path: p, .. } if *p == path))
            .unwrap_or(0);
    }
}

fn maybe_close(key: KeyEvent) -> Vec<Effect> {
    match key.code {
        KeyCode::Esc => vec![Effect::PopModal],
        _ => Vec::new(),
    }
}

pub fn title(modal: &References) -> String {
    format!("{} \u{b7} {} results", modal.title, modal.hits.len())
}

pub fn lines(modal: &References, ctx: &Ctx, area: ratatui::layout::Rect) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let rows = modal.rows();
    // 枠と案内の 1 行を除いた高さ。
    let height = (area.height as usize).saturating_sub(3);
    let scroll = scroll_for(modal.cursor.selected, rows.len(), height);
    let mut out: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, row)| {
            let selected = i == modal.cursor.selected;
            let base = if selected {
                Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.fg)
            };
            match row {
                Row::File {
                    path,
                    count,
                    collapsed,
                } => Line::from(vec![
                    Span::styled(
                        format!(
                            "{} {path}",
                            if *collapsed { '\u{25b8}' } else { '\u{25be}' }
                        ),
                        base.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {count}"), base),
                ]),
                Row::Hit { index } => {
                    let hit = &modal.hits[*index];
                    Line::from(vec![
                        Span::styled(format!("    {:>5}  ", hit.line), base),
                        Span::styled(hit.content.trim().to_string(), base),
                    ])
                }
            }
        })
        .collect();
    out.push(Line::styled(
        "j/k: move  \u{b7}  h/l: fold  \u{b7}  enter: jump  \u{b7}  esc: close",
        Style::default().fg(theme.hint),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;

    fn hit(path: &str, line: usize) -> Reference {
        Reference {
            file_path: path.to_string(),
            line,
            content: format!("line {line}"),
        }
    }

    fn modal() -> References {
        References::new(
            "sym (index)".into(),
            vec![hit("a.rs", 1), hit("a.rs", 9), hit("b.rs", 3)],
        )
    }

    fn labels(modal: &References) -> Vec<String> {
        modal
            .rows()
            .iter()
            .map(|row| match row {
                Row::File {
                    path,
                    count,
                    collapsed,
                } => format!("{} {path} ({count})", if *collapsed { '+' } else { '-' }),
                Row::Hit { index } => format!("  {}", modal.hits[*index].line),
            })
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn 開いた時点で2つ目以降のファイルは畳んである() {
        assert_eq!(labels(&modal()), ["- a.rs (2)", "  1", "  9", "+ b.rs (1)"]);
    }

    #[test]
    fn 見出しの開閉で選択がその見出しに残る() {
        let ws = Workspace::for_test();
        let mut m = modal();
        // 1 件の上へ降りてから h を押すと、そのファイルの見出しへ上がる。
        m.update(key(KeyCode::Down), &ws.ctx());
        m.update(key(KeyCode::Down), &ws.ctx());
        assert_eq!(m.rows()[m.cursor.selected], Row::Hit { index: 1 });
        m.update(key(KeyCode::Char('h')), &ws.ctx());
        assert_eq!(m.cursor.selected, 0);

        // 見出しの上での h は畳む。選択は見出しに残り、行は消える。
        m.update(key(KeyCode::Char('h')), &ws.ctx());
        assert_eq!(labels(&m), ["+ a.rs (2)", "+ b.rs (1)"]);
        assert_eq!(m.cursor.selected, 0);
    }

    #[test]
    fn 一件の上のenterは閉じて飛ぶ() {
        let ws = Workspace::for_test();
        let mut m = modal();
        m.update(key(KeyCode::Down), &ws.ctx());
        let effects = m.update(key(KeyCode::Enter), &ws.ctx());
        let [Effect::PopModal, Effect::JumpTo { path, line }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!((path.to_str().unwrap(), *line), ("a.rs", 1));
    }

    #[test]
    fn 見出しの上のenterは開閉でありジャンプしない() {
        let ws = Workspace::for_test();
        let mut m = modal();
        assert!(m.update(key(KeyCode::Enter), &ws.ctx()).is_empty());
        assert_eq!(labels(&m), ["+ a.rs (2)", "+ b.rs (1)"]);
    }
}
