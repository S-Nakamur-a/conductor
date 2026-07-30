//! Rendering of the explorer's bottom-half changed-files list (Committed /
//! Uncommitted sections) and its per-file review-comment count badge.

use crate::app::{App, Focus};
use crate::viewer::file_icon;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem};

/// Render the diff file list (bottom half) with Committed / Uncommitted sections.
pub(super) fn render_diff_list(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    use crate::diff_state::{DiffListEntry, DiffSection};

    let theme = &app.theme;
    let vs_explorer = &app.viewer_state.explorer;
    let on_diff = vs_explorer.explorer_focus_on_diff_list;
    let diff_focused = panel_focused && on_diff;
    let border_color = if diff_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        theme.border_secondary
    } else if on_diff {
        app.animated_border_color(Focus::Explorer)
    } else {
        theme.border_unfocused
    };

    let total = app.diff_state.committed_files.len() + app.diff_state.uncommitted_files.len();
    let title = diff_list_title(total, app.diff_state.error.is_some());

    let border_type = if panel_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if diff_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs_explorer.diff_list_scroll;

    // Base-resolution failures used to be completely silent: the committed
    // section just came back empty and read as "no changes". Pin the message to
    // the top row so the two are never confused. The banner is not part of
    // `display_list`, so it can't be selected and doesn't shift any index the
    // navigation keys work with — it only costs one row of list height.
    // Newlines are flattened because a multi-line `ListItem` would silently
    // consume more rows than the one reserved here; the List widget clips the
    // overflow at the panel edge.
    let error_banner: Option<ListItem> = app.diff_state.error.as_deref().map(|msg| {
        ListItem::new(Span::styled(
            format!("  \u{26a0} {}", msg.replace('\n', " ")),
            Style::default().fg(theme.error),
        ))
    });
    let list_height = inner_height.saturating_sub(diff_list_banner_rows(error_banner.is_some()));

    let entry_items = app
        .diff_state
        .display_list
        .iter()
        .enumerate()
        .skip(scroll)
        .take(list_height)
        .map(|(idx, entry)| match entry {
            DiffListEntry::Directory {
                name,
                depth,
                collapsed,
                ..
            } => {
                let indent = "  ".repeat(*depth);
                let arrow = if *collapsed { "\u{25b6}" } else { "\u{25bc}" };
                let label = format!("  {indent}{arrow} \u{1f4c1} {name}");

                let style = if idx == vs_explorer.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs_explorer.diff_list_selected {
                    Style::default()
                        .fg(theme.selected_fg_inactive)
                        .bg(theme.selected_bg_inactive)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.info)
                };

                ListItem::new(Span::styled(label, style))
            }
            DiffListEntry::File {
                section,
                file_index,
                depth,
            } => {
                let files = match section {
                    DiffSection::Committed => &app.diff_state.committed_files,
                    DiffSection::Uncommitted => &app.diff_state.uncommitted_files,
                };
                let file_diff = &files[*file_index];

                let filename = file_diff.path.rsplit('/').next().unwrap_or(&file_diff.path);

                let indent = "  ".repeat(*depth);
                let icon = file_icon(filename);
                // Origin marker: C = committed (in HEAD), U = uncommitted
                // (working tree). A file changed both ways appears twice.
                let marker = match section {
                    DiffSection::Committed => "C",
                    DiffSection::Uncommitted => "U",
                };
                let label = format!(
                    "  {indent}{marker} {icon} {filename} +{} -{}",
                    file_diff.added_lines, file_diff.deleted_lines
                );

                let style = if idx == vs_explorer.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs_explorer.diff_list_selected {
                    Style::default()
                        .fg(theme.selected_fg_inactive)
                        .bg(theme.selected_bg_inactive)
                        .add_modifier(Modifier::BOLD)
                } else if file_diff.is_new {
                    Style::default().fg(theme.success)
                } else if file_diff.is_deleted {
                    Style::default().fg(theme.error)
                } else {
                    Style::default().fg(theme.fg)
                };

                // GitHub-style comment badge: 💬N for files with review comments,
                // coloured by whether any are still unresolved.
                let mut spans = vec![Span::styled(label, style)];
                if let Some(badge) = comment_badge(app, &file_diff.path, theme) {
                    spans.push(badge);
                }
                if vs_explorer.viewed.contains(&file_diff.path) {
                    spans.push(Span::styled(
                        "  \u{2713}",
                        Style::default().fg(theme.success),
                    ));
                }
                ListItem::new(Line::from(spans))
            }
            DiffListEntry::Summary {} => {
                let style = if idx == vs_explorer.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs_explorer.diff_list_selected {
                    Style::default()
                        .fg(theme.selected_fg_inactive)
                        .bg(theme.selected_bg_inactive)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
                };
                ListItem::new(Span::styled("  \u{25A3} SUMMARY", style))
            }
        });
    let items: Vec<ListItem> = error_banner.into_iter().chain(entry_items).collect();

    // Clear first so rows below the last item (or stale rows after scrolling /
    // a height change) don't show the previous frame's glyphs — the same
    // scroll-bleed guard the viewer uses.
    frame.render_widget(ratatui::widgets::Clear, area);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Title for the changed-files block. The `— diff error` suffix distinguishes
/// "the committed section is missing because something failed" from a genuine
/// `(0)`; without it the two render identically. Deliberately not "base error":
/// resolving the base ref is the common failure but not the only one — an
/// unresolvable HEAD or a missing merge-base land here too.
fn diff_list_title(total: usize, has_error: bool) -> String {
    if has_error {
        format!(" Changed files ({total}) — diff error ")
    } else {
        format!(" Changed files ({total}) ")
    }
}

/// Rows the error banner occupies at the top of the changed-files list.
///
/// The single source of truth for that geometry. Three places have to agree on
/// it: the renderer (how many entry rows fit), the scroll page size, and the
/// mouse handler (which screen row maps to which `display_list` index). They
/// used to be able to drift, and a one-row disagreement silently opens the
/// wrong file on click.
pub(super) fn diff_list_banner_rows(has_error: bool) -> usize {
    usize::from(has_error)
}

/// Build a GitHub-style comment-count badge (e.g. ` 💬3`) for a file path, or
/// `None` when the file has no review comments. Unresolved comments colour the
/// badge with the accent; an all-resolved file uses muted.
pub(crate) fn comment_badge<'a>(
    app: &App,
    file_path: &str,
    theme: &crate::theme::Theme,
) -> Option<Span<'a>> {
    use crate::review_store::CommentStatus;
    let mut total = 0usize;
    let mut unresolved = 0usize;
    for c in app.review_state.comments.iter().filter(|c| c.file_path == file_path) {
        total += 1;
        if c.status == CommentStatus::Pending {
            unresolved += 1;
        }
    }
    if total == 0 {
        return None;
    }
    let color = if unresolved > 0 {
        theme.accent
    } else {
        theme.muted
    };
    Some(Span::styled(
        format!("  \u{1f4ac}{total}"),
        Style::default().fg(color),
    ))
}

#[cfg(test)]
mod tests {
    use super::{diff_list_banner_rows, diff_list_title};

    /// The whole point of the error suffix: a failed base resolution and a
    /// genuinely clean tree must not render the same title.
    #[test]
    fn error_title_differs_from_a_genuine_zero() {
        assert_ne!(diff_list_title(0, true), diff_list_title(0, false));
        assert_eq!(diff_list_title(0, false), " Changed files (0) ");
        assert!(diff_list_title(0, true).contains("error"));
    }

    /// With the uncommitted section surviving a base failure, the count is
    /// non-zero *and* the error marker is present — both must show.
    #[test]
    fn error_title_keeps_the_count() {
        let title = diff_list_title(17, true);
        assert!(title.contains("(17)"), "{title}");
        assert!(title.contains("error"), "{title}");
    }

    /// The renderer, the scroll page size, and the mouse row→index conversion
    /// all derive the banner's row cost from here. If they ever disagree by one,
    /// a click opens the wrong file — so pin the contract.
    #[test]
    fn banner_costs_exactly_one_row_and_only_when_erroring() {
        assert_eq!(diff_list_banner_rows(false), 0);
        assert_eq!(diff_list_banner_rows(true), 1);
    }
}
