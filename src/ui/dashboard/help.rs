//! Help overlay: per-panel keybinding cheatsheet, auto-generated from the
//! keymap.

use crate::app::App;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Render the help overlay showing keybindings for the current context.
pub fn render_help_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::app::Focus;

    let theme = &app.theme;
    let popup_width = 72_u16.min(area.width.saturating_sub(4));
    let popup_height = 30_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // Tab bar showing which panel's help is displayed.
    let tabs = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).split(popup_area);

    let tab_labels = [
        ("1:Worktree", Focus::Worktree),
        ("2:Explorer", Focus::Explorer),
        ("3:Viewer", Focus::Viewer),
        ("4:Terminal", Focus::TerminalClaude),
    ];

    let tab_spans: Vec<Span> = tab_labels
        .iter()
        .flat_map(|(label, focus)| {
            let style = if *focus == app.overlays.help.context
                || (*focus == Focus::TerminalClaude
                    && app.overlays.help.context == Focus::TerminalShell)
            {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.fg)
            };
            vec![
                Span::styled(format!(" {label} "), style),
                Span::styled(" ", Style::default()),
            ]
        })
        .collect();

    let tab_line =
        Paragraph::new(Line::from(tab_spans)).style(Style::default().bg(theme.titlebar_bg));
    frame.render_widget(tab_line, tabs[0]);

    // Main content block.
    let block = Block::default()
        .title(" Help (?/Esc: close, 1-4: switch panel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(tabs[1]);
    frame.render_widget(block, tabs[1]);

    let lines = help_lines_for(app, app.overlays.help.context, theme);
    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Add a section header line.
fn help_section(lines: &mut Vec<Line<'static>>, title: &'static str, theme: &Theme) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        title,
        Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
    )));
}

/// Add a key binding line (dynamic: keys from KeyMap).
fn help_key_dyn(lines: &mut Vec<Line<'static>>, keys: String, desc: &'static str, theme: &Theme) {
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {keys:<18}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc, Style::default().fg(theme.fg)),
    ]));
}

/// Build the cheatsheet lines for a help tab, **auto-generated** from the
/// keymap so it always lists every binding that fires in that panel — nothing
/// is hand-curated, so no action can be silently missing (the old curated list
/// showed only a fraction). One section per layer, listing that layer's own
/// bindings (global chords are shown once, under "Global").
fn help_lines_for(app: &App, focus: crate::app::Focus, theme: &Theme) -> Vec<Line<'static>> {
    use crate::app::Focus;
    use crate::keymap::{Action, KeyContext};

    let mut lines = Vec::new();

    let section = |lines: &mut Vec<Line<'static>>, title: &'static str, ctx: KeyContext| {
        let mut entries: Vec<(String, &'static str)> = Vec::new();
        for &action in Action::ALL {
            let keys = app.keymap.keys_in_layer(ctx, action);
            if !keys.is_empty() {
                entries.push((keys.join(" / "), action.label()));
            }
        }
        if entries.is_empty() {
            return;
        }
        help_section(lines, title, theme);
        for (keys, desc) in entries {
            help_key_dyn(lines, keys, desc, theme);
        }
    };

    // Panel-specific layers first (most relevant to where you are), then the
    // always-available global chords.
    let panel_ctxs: &[(&'static str, KeyContext)] = match focus {
        Focus::Worktree => &[("Worktree panel", KeyContext::Worktree)],
        Focus::Explorer => &[
            ("Explorer — file tree", KeyContext::Explorer),
            ("Explorer — changed files", KeyContext::ExplorerDiffList),
            ("Explorer — comment list", KeyContext::ExplorerCommentList),
            ("Explorer — walkthrough", KeyContext::ExplorerWalkthrough),
        ],
        Focus::Viewer => &[
            ("Viewer", KeyContext::Viewer),
            ("Viewer — diff mode", KeyContext::ViewerDiffMode),
        ],
        Focus::TerminalClaude | Focus::TerminalShell => &[("Terminal panel", KeyContext::Terminal)],
        Focus::Editor => &[("Editor panel", KeyContext::Editor)],
    };
    for (title, ctx) in panel_ctxs {
        section(&mut lines, title, *ctx);
    }
    if matches!(focus, Focus::TerminalClaude | Focus::TerminalShell) {
        help_transcript_section(&mut lines, theme);
    }
    help_review_commands_section(&mut lines, theme);
    section(&mut lines, "Global — works anywhere", KeyContext::Global);

    lines
}

/// Keys for the Claude transcript, reached by scrolling up from the live tail.
///
/// Hand-written for the same reason as the section below: `handle_reflow_key`
/// owns these directly instead of going through `app.keymap`, so `section()` —
/// which walks the keymap — cannot see any of them. They were invisible in the
/// help until this existed, `G` included.
fn help_transcript_section(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    help_section(lines, "Claude transcript (scroll up to enter)", theme);
    for (keys, desc) in [
        ("j / k", "Scroll one line"),
        ("ctrl+d / ctrl+u", "Scroll half a page"),
        ("g / Home", "Oldest turn"),
        ("G / End", "Newest turn, and resume following it"),
        ("ctrl+o", "Expand / collapse tool results and thinking"),
        ("Esc", "Back to the live prompt"),
    ] {
        help_key_dyn(lines, keys.to_string(), desc, theme);
    }
}

/// The PR-intake, walkthrough-generation, and publish commands have no
/// default keybinding (see `default_keybinds.toml`) — they're reached only
/// through the command palette, so `section()` above (which walks
/// `app.keymap`) never finds them. Listed here instead so the help screen
/// still surfaces them.
fn help_review_commands_section(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    help_section(lines, "Review (via command palette)", theme);
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Review Pull Request…",
        theme,
    );
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Generate Walkthrough",
        theme,
    );
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Publish Comments to GitHub",
        theme,
    );
}
