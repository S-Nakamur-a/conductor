//! トランスクリプトを Claude 区画の内側に描く。行の組み直しは
//! [super::Reflow::prepare] が先に済ませてあるので、ここは転送するだけ。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::Reflow;
use super::build::LineMeta;
use super::style::{INACTIVE, USER_BG, truncate_to_width};

/// 追従が外れているときだけ出すチップ。長い順に並べ、収まる最初のものを選ぶ。
///
/// 意図的に ASCII だけにしている。パネルの右端に接するので、端末が unicode-width より
/// 広く描く字があると末尾が枠にはみ出す。
pub(super) const JUMP_LABELS: [&str; 3] = [" Jump to latest (G) ", " Latest (G) ", " (G) "];

pub(crate) fn render(frame: &mut Frame, area: Rect, reflow: &Reflow) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // 部分的にしか埋まらないトランスクリプトでも、下のライブ PTY が透けないよう必ず消す。
    frame.render_widget(Clear, area);

    if reflow.loading {
        let msg = "Loading transcript\u{2026}";
        let cols = UnicodeWidthStr::width(msg).min(area.width as usize) as u16;
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(cols)) / 2,
            area.y + area.height / 2,
            cols,
            1,
        );
        let text = truncate_to_width(msg, area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(INACTIVE),
            ))),
            rect,
        );
        return;
    }

    let buffer = frame.buffer_mut();
    for (row, (line, meta)) in reflow
        .lines
        .iter()
        .zip(reflow.meta.iter())
        .skip(reflow.scroll)
        .take(area.height as usize)
        .enumerate()
    {
        let y = area.y + row as u16;
        buffer.set_line(area.x, y, line, area.width);
        // 溝の字の直後を 1 セル未書き込みで残す。Buffer::reset で毎フレーム消えるので、
        // フレームごとに付け直す必要がある。
        if let Some(col) = meta.skip_col
            && col < area.width
            && let Some(cell) = buffer.cell_mut((area.x + col, y))
        {
            cell.set_skip(true);
        }
    }

    if let Some((rect, label)) = badge(area, reflow.follow) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label,
                Style::default().fg(INACTIVE).bg(USER_BG),
            ))),
            rect,
        );
    }
}

/// 追従中は無い — 無いこと自体が合図で、戻った後に古い矩形がクリックを飲み続けることもない。
pub(super) fn badge(area: Rect, following: bool) -> Option<(Rect, &'static str)> {
    if following || area.height == 0 {
        return None;
    }
    let label = JUMP_LABELS
        .iter()
        .find(|l| UnicodeWidthStr::width(**l) < area.width as usize)?;
    let width = UnicodeWidthStr::width(*label) as u16;
    let rect = Rect::new(
        area.x + area.width - width,
        area.y + area.height - 1,
        width,
        1,
    );
    Some((rect, label))
}

/// 組み直した後に anchor と一致する行。(entry, block, offset) がそれ以上の最初の行を返すので、
/// 縮んだり消えたりしたブロックでも今その位置を占めている何かに着地する。
pub(super) fn anchor_index(meta: &[LineMeta], anchor: LineMeta) -> usize {
    let key = (anchor.entry, anchor.block, anchor.offset);
    meta.iter()
        .position(|m| (m.entry, m.block, m.offset) >= key)
        .unwrap_or_else(|| meta.len().saturating_sub(1))
}
