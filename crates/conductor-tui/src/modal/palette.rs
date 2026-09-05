//! コマンドパレット。全コマンドをあいまい検索して 1 本の実行口へ渡す。

use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::command::{self, Scope};
use crate::effect::Effect;
use crate::modal::picker::{Cursor, Filtered, filtered_key, scroll_for};
use crate::workspace::{Ctx, Workspace};

#[derive(Debug, Default)]
pub struct Palette {
    pub input: TextInput,
    pub cursor: Cursor,
}

impl Palette {
    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let hits = command::filter(self.input.text(), ctx.keymap, ctx.key_context);
        match key.code {
            KeyCode::Esc => return vec![Effect::PopModal],
            KeyCode::Enter => {
                let Some(hit) = hits.get(self.cursor.selected) else {
                    return vec![Effect::PopModal];
                };
                return vec![
                    Effect::PopModal,
                    Effect::Command(command::COMMANDS[hit.index].id),
                ];
            }
            _ => {}
        }
        if let Filtered::Typed = filtered_key(
            &mut self.cursor,
            &mut self.input,
            ctx.keymap,
            key,
            hits.len(),
        ) {
            self.cursor.selected = 0;
        }
        Vec::new()
    }
}

pub fn title() -> String {
    "Command Palette".into()
}

pub fn lines(palette: &Palette, ws: &Workspace, area: Rect) -> Vec<Line<'static>> {
    let ctx = &ws.ctx();
    let theme = ctx.theme;
    let hits = command::filter(palette.input.text(), ctx.keymap, ctx.key_context);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                crate::modal::input::with_caret(&palette.input, area.width as usize).join(""),
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(""),
    ];
    if hits.is_empty() {
        lines.push(Line::styled(
            "  no matching commands",
            Style::default().fg(theme.muted),
        ));
        return lines;
    }

    // スコープの見出しも行を食うので、窓は見出しを混ぜたあとの並びで数える。
    // 混ぜる前の件数で窓を決めると、下端の数行が枠からはみ出して消える。
    let mut rows = Vec::new();
    let mut selected_row = 0;
    let mut scope = None;
    for (rank, hit) in hits.iter().enumerate() {
        if scope != Some(hit.scope) {
            scope = Some(hit.scope);
            rows.push(Line::styled(
                format!(" {}", scope_label(hit.scope, ctx)),
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            ));
        }
        let command = &command::COMMANDS[hit.index];
        let selected = rank == palette.cursor.selected;
        if selected {
            selected_row = rows.len();
        }
        let chord = command
            .action
            .and_then(|a| crate::render::representative_chord(ctx.keymap, ctx.key_context, a))
            .unwrap_or_default();
        let label = match command::enabled(ws, command.id) {
            command::Enabled::Yes => Style::default().fg(theme.fg),
            // 灰色でも並びからは外さない。使えない理由は実行したときに出す。
            command::Enabled::No(_) => Style::default().fg(theme.fg).add_modifier(Modifier::DIM),
        };
        rows.push(crate::list::row_line(
            vec![
                Span::styled(if selected { " > " } else { "   " }, label),
                Span::styled(command.label, label),
                Span::styled(format!("  {chord}"), Style::default().fg(theme.hint)),
            ],
            theme,
            selected,
            true,
        ));
    }
    let height = (area.height as usize).saturating_sub(4);
    let start = scroll_for(selected_row, rows.len(), height);
    lines.extend(rows.into_iter().skip(start).take(height));
    lines
}

fn scope_label(scope: Scope, ctx: &Ctx) -> &'static str {
    match scope {
        Scope::Current => ctx.focus.label(),
        Scope::Global => "Global",
        Scope::Other => "Other",
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

    fn drawn(palette: &Palette, ws: &Workspace, height: u16) -> Vec<String> {
        lines(palette, ws, Rect::new(0, 0, 70, height))
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect()
    }

    /// 見出しを混ぜたあとの行数で窓を決めないと、下端が枠からはみ出して消える。
    #[test]
    fn 描く行は枠の内側に収まる() {
        let ws = Workspace::for_test();
        let mut palette = Palette::default();
        for height in [8, 16, 24] {
            palette.cursor.selected = 0;
            assert!(
                drawn(&palette, &ws, height).len() <= height as usize - 2,
                "{height}"
            );
            palette.cursor.selected = command::COMMANDS.len() - 1;
            let rows = drawn(&palette, &ws, height);
            assert!(
                rows.len() <= height as usize - 2,
                "{height}: {}",
                rows.len()
            );
            assert!(
                rows.iter().any(|r| r.starts_with(" > ")),
                "末尾の選択が窓から外れた: {rows:?}"
            );
        }
    }

    #[test]
    fn 入力で絞り込み選択は先頭に戻る() {
        let ws = Workspace::for_test();
        let mut palette = Palette::default();
        palette.update(key(KeyCode::Down), &ws.ctx());
        assert_eq!(palette.cursor.selected, 1);
        for c in "quit".chars() {
            palette.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        assert_eq!(palette.cursor.selected, 0);
        assert_eq!(palette.input.text(), "quit");
    }

    #[test]
    fn 一致が無ければ選んでも何も実行しない() {
        let ws = Workspace::for_test();
        let mut palette = Palette::default();
        for c in "zzzz".chars() {
            palette.update(key(KeyCode::Char(c)), &ws.ctx());
        }
        assert!(
            drawn(&palette, &ws, 20)
                .iter()
                .any(|r| r.contains("no matching"))
        );
        assert_eq!(
            palette.update(key(KeyCode::Enter), &ws.ctx()),
            vec![Effect::PopModal]
        );
    }
}
