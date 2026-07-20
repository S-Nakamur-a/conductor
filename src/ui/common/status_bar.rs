//! The status bar at the bottom of the screen: transient flash messages, or
//! (when idle) the keybinding hint for the focused panel derived live from
//! the keymap.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

/// Render a status bar at the bottom of the screen.
pub fn render_status_bar(frame: &mut Frame, area: Rect, app: &crate::app::App) {
    use crate::app::StatusLevel;

    let theme = &app.theme;

    if let Some(ref msg) = app.status_message {
        let age = app.ui_tick.wrapping_sub(msg.created_at_tick);

        // Color based on level.
        let fg_color = match msg.level {
            StatusLevel::Success => theme.success,
            StatusLevel::Error => theme.error,
            StatusLevel::Warning => theme.warning,
            StatusLevel::Info => theme.info,
        };

        // Flash background for the first ~500ms (30 ticks).
        let bg_color = if age < 30 {
            if (age / 5) % 2 == 0 {
                match msg.level {
                    StatusLevel::Success => theme.status_bg_success,
                    StatusLevel::Error => theme.status_bg_error,
                    StatusLevel::Warning => theme.status_bg_warning,
                    StatusLevel::Info => theme.status_bg_info,
                }
            } else {
                Color::Reset
            }
        } else {
            Color::Reset
        };

        // Fade: after 2.5 seconds (150 ticks), dimmed style.
        let style = if age >= 150 {
            Style::default().fg(theme.muted).bg(Color::Reset)
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
        // Default keybinding hint text, derived live from the keymap so it
        // never drifts from the actual bindings (including user overrides).
        let hint = status_bar_hint(app.focus, &app.keymap);
        let span = Span::styled(hint, Style::default().fg(theme.hint));
        frame.render_widget(Paragraph::new(span), area);
    }
}

/// Build the footer keybinding hint for the focused panel from the live keymap.
///
/// Each panel has an ordered list of `(label, actions)`; for each entry we show
/// one representative chord per action (joined by `/`), e.g. `j/k: nav`. Entries
/// whose actions are all unbound are dropped, so the hint can never advertise a
/// key that does nothing.
pub(super) fn status_bar_hint(focus: crate::app::Focus, keymap: &crate::keymap::KeyMap) -> String {
    use crate::app::Focus;
    use crate::keymap::Action;

    // (label, actions whose representative chords are shown joined by '/').
    let entries: &[(&str, &[Action])] = match focus {
        Focus::Worktree => &[
            ("nav", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("open", &[Action::Select]),
            ("new", &[Action::CreateWorktree]),
            ("switch", &[Action::SwitchBranch]),
            ("grab", &[Action::GrabBranch]),
        ],
        Focus::Explorer => &[
            ("nav", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("open", &[Action::Select]),
            ("fold", &[Action::CollapseOrLeft, Action::ExpandOrRight]),
            ("diff", &[Action::ShowDiffList]),
            ("search", &[Action::SearchFilename]),
        ],
        Focus::Viewer => &[
            ("scroll", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("search", &[Action::SearchInFile]),
            ("thread", &[Action::ToggleInlineThread]),
            ("back", &[Action::ExitToExplorer]),
        ],
        Focus::TerminalClaude => &[
            ("leave", &[Action::LeaveTerminal]),
            ("panel", &[Action::CycleFocusForward]),
            ("new CC", &[Action::NewClaudeCode]),
            ("session", &[Action::NextSession]),
        ],
        Focus::TerminalShell => &[
            ("leave", &[Action::LeaveTerminal]),
            ("panel", &[Action::CycleFocusForward]),
            ("new shell", &[Action::NewShell]),
            ("session", &[Action::NextSession]),
        ],
        Focus::Editor => &[
            ("Claude", &[Action::LeaveTerminal]),
            ("zoom", &[Action::TogglePanelExpand]),
            ("panel", &[Action::CycleFocusForward]),
        ],
    };

    let context = focus.key_context();
    let mut parts: Vec<String> = Vec::new();
    for (label, actions) in entries {
        let chords: Vec<String> = actions
            .iter()
            .filter_map(|a| representative_chord(keymap, context, *a))
            .collect();
        if !chords.is_empty() {
            parts.push(format!("{}: {label}", chords.join("/")));
        }
    }

    // Always advertise the command palette and the cheatsheet — they're the
    // entry points to every other action, so they belong in every context's
    // footer (the palette even fires over a PTY; `?` is shown only where it
    // actually fires, i.e. not in the terminal/editor).
    if let Some(c) = representative_chord(keymap, context, Action::CommandPalette) {
        parts.push(format!("{c}: cmds"));
    }
    if let Some(c) = representative_chord(keymap, context, Action::ShowHelp) {
        parts.push(format!("{c}: keys"));
    }

    // Terminals forward everything else to the PTY — set that expectation.
    if matches!(focus, Focus::TerminalClaude | Focus::TerminalShell) {
        parts.push("keys → terminal".to_string());
    }

    parts.join(" | ")
}

/// The single chord best suited to show a user for `action` in `context`:
/// shortest, ASCII-only. The macOS Option-glyph fallbacks (`¬`, `˙`, …) and
/// other non-ASCII chords round-trip through the keymap but are meaningless on
/// screen, so a plain chord is preferred whenever one exists.
pub(crate) fn representative_chord(
    keymap: &crate::keymap::KeyMap,
    context: crate::keymap::KeyContext,
    action: crate::keymap::Action,
) -> Option<String> {
    keymap
        .keys_for_action(context, action)
        .into_iter()
        .filter(|c| c.is_ascii())
        .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}
