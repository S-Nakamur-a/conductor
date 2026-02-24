//! Shared UI components used across multiple panels.
//!
//! Provides reusable widgets such as PTY output rendering, session tab bars,
//! and the status bar.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// A snapshot of a single cell's content and style, extracted from the vt100 screen.
struct CellSnapshot {
    text: String,
    style: Style,
}

/// A snapshot of the vt100 screen contents, captured while holding the lock
/// so that the lock can be released before the (slower) ratatui rendering step.
struct ScreenSnapshot {
    rows: Vec<Vec<CellSnapshot>>,
    effective_offset: usize,
}

/// Take a point-in-time snapshot of the vt100 screen contents.
///
/// Acquires and releases the parser lock as quickly as possible, extracting
/// only the cell data needed for rendering into a local structure.
fn snapshot_screen(
    screen_arc: &Arc<Mutex<vt100::Parser>>,
    scroll_offset: usize,
    max_rows: u16,
    max_cols: u16,
) -> ScreenSnapshot {
    let mut parser = screen_arc.lock().unwrap_or_else(|e| e.into_inner());

    let is_alt_screen = parser.screen().alternate_screen();
    let effective_offset = if is_alt_screen { 0 } else { scroll_offset };

    parser.set_scrollback(effective_offset);

    let screen = parser.screen();
    let (rows, cols) = screen.size();

    // Debug: log alternate screen state periodically.
    if is_alt_screen {
        let has_content = (0..rows.min(5)).any(|r| {
            (0..cols).any(|c| {
                if let Some(cell) = screen.cell(r, c) {
                    let ch = cell.contents();
                    !ch.is_empty() && ch != " "
                } else {
                    false
                }
            })
        });
        let cursor = screen.cursor_position();
        log::debug!(
            "ALT_SCREEN render: has_content={has_content}, size=({rows},{cols}), area=({max_rows},{max_cols}) cursor=({},{})",
            cursor.0, cursor.1,
        );
    }

    // Extract cell data into local snapshot.
    let mut snapshot_rows: Vec<Vec<CellSnapshot>> = Vec::with_capacity(rows.min(max_rows) as usize);
    for row in 0..rows.min(max_rows) {
        let mut row_cells: Vec<CellSnapshot> = Vec::new();
        for col in 0..cols.min(max_cols) {
            let cell = screen.cell(row, col).unwrap();
            row_cells.push(CellSnapshot {
                text: cell.contents(),
                style: vt100_cell_to_style(cell),
            });
        }
        snapshot_rows.push(row_cells);
    }

    // Restore live view so other readers see the current screen.
    parser.set_scrollback(0);

    // Lock is dropped here when `parser` goes out of scope.
    ScreenSnapshot {
        rows: snapshot_rows,
        effective_offset,
    }
}

/// Render the vt100 screen of a PTY session into the given area.
///
/// `scroll_offset` controls scrollback: 0 = live view (bottom), >0 = scroll back into history.
/// Scrollback is disabled when the PTY is in alternate screen mode (vim, less, etc.).
///
/// The screen lock is held only long enough to snapshot cell data; all ratatui
/// widget construction and rendering happens after the lock is released.
pub fn render_pty_output(
    frame: &mut Frame,
    area: Rect,
    screen_arc: &Arc<Mutex<vt100::Parser>>,
    scroll_offset: usize,
) {
    // Phase 1: snapshot screen contents (lock is held briefly).
    let snapshot = snapshot_screen(screen_arc, scroll_offset, area.height, area.width);

    // Phase 2: build ratatui widgets from the snapshot (no lock held).
    let mut text_lines: Vec<Line> = Vec::new();
    for row_cells in &snapshot.rows {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        let mut skip_cols: usize = 0;
        for cell in row_cells {
            if skip_cols > 0 {
                skip_cols -= 1;
                continue;
            }
            let ch = &cell.text;
            let style = cell.style;

            if style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut current_text), current_style));
                current_style = style;
            }
            if ch.is_empty() {
                current_text.push(' ');
            } else {
                let w = UnicodeWidthStr::width(ch.as_str());
                if w > 1 {
                    skip_cols = w - 1;
                }
                current_text.push_str(ch);
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        text_lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(text_lines);
    frame.render_widget(paragraph, area);

    // Show a visual indicator when scrolled back.
    if snapshot.effective_offset > 0 {
        let indicator = Line::from(Span::styled(
            format!(" ↑ scrollback ({} lines — Shift+End to return) ", snapshot.effective_offset),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        ));
        frame.render_widget(Paragraph::new(indicator), Rect { height: 1, ..area });
    }
}

/// Convert HSL (h: 0-360, s: 0-1, l: 0-1) to RGB.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h2 = h / 60.0;
    let x = c * (1.0 - (h2 % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h2 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// Generate badge background and branch text colors from a repository name.
///
/// Uses a hash of the name to pick a hue, then produces two colors:
/// - Badge background: muted (S=0.6, L=0.45)
/// - Branch text: brighter (S=0.7, L=0.75)
fn name_to_color(name: &str) -> (Color, Color) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();
    let hue = (hash % 360) as f64;

    let (br, bg, bb) = hsl_to_rgb(hue, 0.6, 0.45);
    let (tr, tg, tb) = hsl_to_rgb(hue, 0.7, 0.75);
    (Color::Rgb(br, bg, bb), Color::Rgb(tr, tg, tb))
}

/// Render the title bar at the top showing worktree name and working directory.
pub fn render_title_bar(frame: &mut Frame, area: Rect, app: &mut crate::app::App) {
    let wt_name = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.branch.as_str())
        .unwrap_or("—");
    let wt_path = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.path.display().to_string())
        .unwrap_or_else(|| app.repo_path.display().to_string());

    let (badge_bg, branch_fg) = name_to_color(&app.main_repo_name);

    let bar_bg = Color::DarkGray;
    let conductor_bg = badge_bg;
    let conductor_fg = Color::Black;

    let badge_text = format!(" {} ", app.main_repo_name);
    let line = Line::from(vec![
        Span::styled(
            &badge_text,
            Style::default().fg(conductor_fg).bg(conductor_bg).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(wt_name, Style::default().fg(branch_fg).add_modifier(Modifier::BOLD)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(wt_path, Style::default().fg(Color::Gray)),
    ]);
    let paragraph = Paragraph::new(line).style(Style::default().bg(bar_bg));
    frame.render_widget(paragraph, area);

    // ── Right-aligned stats display (today's activity + ccusage) ──────────
    {
        let sep = Span::styled(" | ", Style::default().fg(Color::DarkGray).bg(Color::DarkGray));
        let mut spans: Vec<Span> = Vec::new();

        if let Some(ref stats) = app.today_stats {
            spans.push(Span::styled(
                format!("{} branches", stats.branches_created),
                Style::default().fg(Color::Cyan).bg(Color::DarkGray),
            ));
            spans.push(sep.clone());
            spans.push(Span::styled(
                format!("{} commits", stats.commits_made),
                Style::default().fg(Color::Green).bg(Color::DarkGray),
            ));
            spans.push(sep.clone());
            spans.push(Span::styled(
                format!("{} reviews", stats.reviews_created),
                Style::default().fg(Color::Magenta).bg(Color::DarkGray),
            ));
        }
        if let Some(ref info) = app.ccusage_info {
            if !spans.is_empty() {
                spans.push(sep.clone());
            }
            spans.push(Span::styled(
                format!("{} tokens", format_tokens(info.total_tokens)),
                Style::default().fg(Color::Yellow).bg(Color::DarkGray),
            ));
            spans.push(sep.clone());
            spans.push(Span::styled(
                format!("${:.2}", info.total_cost),
                Style::default().fg(Color::LightGreen).bg(Color::DarkGray),
            ));
        }

        if !spans.is_empty() {
            // Add padding spaces
            spans.insert(0, Span::styled(" ", Style::default().bg(Color::DarkGray)));
            spans.push(Span::styled(" ", Style::default().bg(Color::DarkGray)));

            let stats_line = Line::from(spans);
            let stats_w = stats_line.width() as u16;
            if stats_w + 2 < area.width {
                let stats_area = Rect::new(
                    area.x + area.width - stats_w,
                    area.y,
                    stats_w,
                    1,
                );
                frame.render_widget(Paragraph::new(stats_line), stats_area);
            }
        }
    }
}

/// Render the notification bar showing CC waiting badges.
/// Returns the height consumed (0 if no notifications, 1 if shown).
/// Records badge positions in `app.notification_bar_badges` for click handling.
pub fn render_notification_bar(frame: &mut Frame, area: Rect, app: &mut crate::app::App) -> u16 {
    app.notification_bar_badges.clear();

    if app.cc_waiting_worktrees.is_empty() {
        return 0;
    }

    // Orange-tinted background for the notification bar.
    let pulse_on = (app.ui_tick / 20) % 2 == 0;
    let bar_bg = if pulse_on {
        Color::Rgb(50, 30, 0)  // warm dark orange
    } else {
        Color::Rgb(35, 20, 0)  // darker pulse
    };

    // Fill background.
    let bg_line = Line::from(Span::styled(
        " ".repeat(area.width as usize),
        Style::default().bg(bar_bg),
    ));
    frame.render_widget(Paragraph::new(bg_line), area);

    // Leading indicator.
    let prefix = " ⏳ ";
    let prefix_style = Style::default()
        .fg(Color::Rgb(255, 180, 50))
        .bg(bar_bg)
        .add_modifier(Modifier::BOLD);
    let prefix_area = Rect::new(area.x, area.y, prefix.len() as u16, 1);
    frame.render_widget(Paragraph::new(Span::styled(prefix, prefix_style)), prefix_area);

    // Collect waiting worktrees sorted by branch name.
    let mut waiting: Vec<(&PathBuf, String)> = app.cc_waiting_worktrees.iter().map(|p| {
        let name = app.worktrees.iter()
            .find(|w| &w.path == p)
            .map(|w| w.branch.clone())
            .unwrap_or_else(|| p.file_name().and_then(|f| f.to_str()).unwrap_or("?").to_string());
        (p, name)
    }).collect();
    waiting.sort_by(|a, b| a.1.cmp(&b.1));

    // Badge colors: orange pulse.
    let badge_bg = if pulse_on {
        Color::Rgb(200, 120, 0) // bright orange
    } else {
        Color::Rgb(180, 100, 0) // slightly dimmer
    };
    let badge_style = Style::default()
        .fg(Color::Black)
        .bg(badge_bg)
        .add_modifier(Modifier::BOLD);

    let sep_style = Style::default()
        .fg(Color::Rgb(180, 120, 40))
        .bg(bar_bg);

    let mut x = area.x + UnicodeWidthStr::width(prefix) as u16;

    for (i, (_path, name)) in waiting.iter().enumerate() {
        if i > 0 {
            // Separator between badges.
            let sep_area = Rect::new(x, area.y, 1, 1);
            frame.render_widget(Paragraph::new(Span::styled(" ", sep_style)), sep_area);
            x += 1;
        }

        let badge_str = format!(" {name} ⏳ ");
        let w = UnicodeWidthStr::width(badge_str.as_str()) as u16;

        if x + w > area.x + area.width {
            break; // not enough room
        }

        let badge_area = Rect::new(x, area.y, w, 1);
        frame.render_widget(Paragraph::new(Span::styled(&badge_str, badge_style)), badge_area);

        // Record position for click handling.
        app.notification_bar_badges.push((x, x + w, name.clone()));

        x += w;
    }

    // Trailing hint text.
    let hint = " (click to jump)";
    let hint_w = UnicodeWidthStr::width(hint) as u16;
    if x + hint_w < area.x + area.width {
        let hint_area = Rect::new(x + 1, area.y, hint_w, 1);
        let hint_style = Style::default().fg(Color::Rgb(120, 90, 40)).bg(bar_bg);
        frame.render_widget(Paragraph::new(Span::styled(hint, hint_style)), hint_area);
    }

    1
}

/// Render a status bar at the bottom of the screen.
pub fn render_status_bar(frame: &mut Frame, area: Rect, app: &crate::app::App) {
    use crate::app::StatusLevel;
    use crate::theme::Theme;

    let theme = Theme::from_name(&app.config.viewer.theme);

    if let Some(ref msg) = app.status_message {
        let age = app.ui_tick.wrapping_sub(msg.created_at_tick);

        // Color based on level.
        let fg_color = match msg.level {
            StatusLevel::Success => theme.success,
            StatusLevel::Error   => theme.error,
            StatusLevel::Warning => theme.warning,
            StatusLevel::Info    => theme.info,
        };

        // Flash background for the first ~500ms (30 ticks).
        let bg_color = if age < 30 {
            if (age / 5) % 2 == 0 {
                match msg.level {
                    StatusLevel::Success => Color::Rgb(0, 30, 0),
                    StatusLevel::Error   => Color::Rgb(40, 0, 0),
                    StatusLevel::Warning => Color::Rgb(40, 30, 0),
                    StatusLevel::Info    => Color::Rgb(0, 20, 40),
                }
            } else {
                Color::Reset
            }
        } else {
            Color::Reset
        };

        // Fade: after 2.5 seconds (150 ticks), dimmed style.
        let style = if age >= 150 {
            Style::default().fg(Color::DarkGray).bg(Color::Reset)
        } else {
            let mut s = Style::default().fg(fg_color).bg(bg_color);
            if age < 30 {
                s = s.add_modifier(Modifier::BOLD);
            }
            s
        };

        let display_text = format!("{}{}", msg.icon(), msg.text);
        let span = Span::styled(display_text, style);
        frame.render_widget(Paragraph::new(span), area);
    } else {
        // Default keybinding hint text.
        let hint = app.status_bar_text();
        let span = Span::styled(hint, Style::default().fg(Color::Gray));
        frame.render_widget(Paragraph::new(span), area);
    }
}

/// Render the current worktree branch and repository name at the far right
/// of the given row area (overlays on the same line).
pub fn render_worktree_label(
    frame: &mut Frame,
    row_area: Rect,
    worktree_branch: &str,
    repo_path: &std::path::Path,
) {
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| repo_path.display().to_string());

    let branch_part = worktree_branch;
    let repo_part = format!("[{repo_name}]");
    let total_width = UnicodeWidthStr::width(branch_part) + 1 + UnicodeWidthStr::width(repo_part.as_str());

    if total_width as u16 + 1 > row_area.width {
        return;
    }

    let label_area = Rect::new(
        row_area.x + row_area.width - total_width as u16,
        row_area.y,
        total_width as u16,
        1,
    );

    let line = Line::from(vec![
        Span::styled(branch_part, Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        Span::styled(repo_part, Style::default().fg(Color::DarkGray)),
    ]);
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, label_area);
}

// ── vt100 helpers ──────────────────────────────────────────────────────

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();
    style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    style = style.bg(vt100_color_to_ratatui(cell.bgcolor()));
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

/// Format a token count into a human-readable string (e.g. "1.2K", "14.2M").
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}
