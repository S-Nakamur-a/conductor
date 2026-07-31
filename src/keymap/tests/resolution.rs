//! Tests that the default keymap resolves the expected action for a chord in
//! each [`KeyContext`], including terminal/editor interception and
//! context-to-global fallback.

use super::*;
use super::super::map::DEFAULT_KEYBINDS;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keymap_suite::ActionName;

#[test]
fn defaults_build_without_warnings() {
    let (_km, warnings) = KeyMap::with_warnings(&toml::Table::new());
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}

#[test]
fn every_default_action_name_resolves() {
    // Guards against a typo in default_keybinds.toml: an unknown action name
    // would surface as a warning when the defaults are parsed.
    let build = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_name).unwrap();
    assert!(build.warnings.is_empty(), "{:?}", build.warnings);
}

#[test]
fn critical_defaults_resolve() {
    let km = default_keymap();

    // Quit moved to ctrl+q; bare q is unbound (passes through) so it can no
    // longer kill the app by accident.
    let key_ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&key_ctrl_q, KeyContext::Global), Some(Action::Quit));
    let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
    assert_eq!(km.resolve(&key_q, KeyContext::Global), None);

    let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_j, KeyContext::Worktree),
        Some(Action::NavigateDown)
    );

    let key_ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&key_ctrl_n, KeyContext::Global),
        Some(Action::NewClaudeCode)
    );

    // Ctrl+Esc leaves the terminal.
    let key_ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&key_ctrl_esc, KeyContext::Terminal),
        Some(Action::LeaveTerminal)
    );
}

#[test]
fn worktree_switch_and_zoom_aliases_resolve() {
    // alt+]/alt+[ are the kitty-protocol-free aliases for ctrl+tab worktree
    // switching; ctrl+alt+z zooms the focused panel (tmux `prefix z`), joining
    // the ctrl+alt pane-sizing family.
    let km = default_keymap();
    let cases = [
        (KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT), Action::NextWorktree),
        (KeyEvent::new(KeyCode::Char('['), KeyModifiers::ALT), Action::PrevWorktree),
        (
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::ALT),
            Action::TogglePanelExpand,
        ),
    ];
    for (key, action) in cases {
        assert_eq!(km.resolve(&key, KeyContext::Global), Some(action), "{key:?}");
    }
}

#[test]
fn explorer_walkthrough_layer_resolves() {
    let km = default_keymap();
    let cases = [
        (KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()), Action::NavigateDown),
        (KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()), Action::NavigateUp),
        (KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()), Action::Select),
        (KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()), Action::WalkthroughNextStep),
        (KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT), Action::WalkthroughPrevStep),
        (KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), Action::ExitSubPanel),
    ];
    for (key, action) in cases {
        assert_eq!(
            km.resolve(&key, KeyContext::ExplorerWalkthrough),
            Some(action),
            "{key:?}"
        );
    }
}

#[test]
fn terminal_intercepts_only_firing_actions() {
    let km = default_keymap();

    // Quit (ctrl+q) is global but does NOT fire in the terminal — the chord
    // reaches the PTY instead of killing the app (so the inner program keeps
    // ctrl+q / XON). Same for switch_repo (ctrl+r → shell reverse-search).
    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_q, KeyContext::Global), Some(Action::Quit));
    assert_eq!(km.resolve(&ctrl_q, KeyContext::Terminal), None);
    let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_r, KeyContext::Global), Some(Action::SwitchRepo));
    assert_eq!(km.resolve(&ctrl_r, KeyContext::Terminal), None);

    // Focus/navigation chords ARE stolen back from the PTY.
    let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_p, KeyContext::Terminal),
        Some(Action::CommandPalette)
    );
    let alt_next = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&alt_next, KeyContext::Terminal),
        Some(Action::NextWorktree)
    );
    let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_esc, KeyContext::Terminal),
        Some(Action::LeaveTerminal)
    );

    // Rendered help stays honest with resolution: no chord is advertised for
    // Quit in the terminal, but terminal-firing actions keep theirs.
    assert!(km.keys_for_action(KeyContext::Terminal, Action::Quit).is_empty());
    assert!(
        !km.keys_for_action(KeyContext::Terminal, Action::LeaveTerminal)
            .is_empty()
    );
}

#[test]
fn terminal_usable_actions_all_resolve_in_terminal() {
    // Every action classified as firing in the terminal must actually have a
    // chord that resolves there — guards against adding a variant to
    // `fires_in_terminal` but forgetting to bind it (or vice versa).
    let km = default_keymap();
    let usable = [
        Action::LeaveTerminal,
        Action::ScrollbackUp,
        Action::ScrollbackDown,
        Action::ScrollbackTop,
        Action::SnapToLive,
        Action::OpenFileFromTerminal,
        Action::FocusWorktree,
        Action::FocusExplorer,
        Action::FocusExplorerDiffList,
        Action::FocusViewer,
        Action::FocusTerminalClaude,
        Action::FocusTerminalShell,
        Action::CommandPalette,
        Action::CycleFocusForward,
        Action::CycleFocusBackward,
        Action::NextWorktree,
        Action::PrevWorktree,
        Action::TogglePanelExpand,
        Action::TogglePanelOverlay,
    ];
    for a in usable {
        assert!(
            !km.keys_for_action(KeyContext::Terminal, a).is_empty(),
            "{a:?} should have a working chord in the terminal"
        );
    }
}

#[test]
fn editor_context_steals_only_leave_and_globals() {
    // The embedded editor forwards almost everything to vim/emacs. It steals
    // back only Ctrl+Esc (leave) and the terminal-firing global chords; keys
    // the editor needs — Esc, Ctrl+G, Shift+PageUp — pass through (None).
    let km = default_keymap();

    let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_esc, KeyContext::Editor),
        Some(Action::LeaveTerminal)
    );

    // Bare Esc → vim mode changes; must not be stolen.
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(km.resolve(&esc, KeyContext::Editor), None);

    // Ctrl+G is open_file_from_terminal in the *terminal* layer and
    // search_full_text globally — neither fires in the editor, so it reaches
    // the inner program instead of being intercepted.
    let ctrl_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_g, KeyContext::Editor), None);

    // Scrollback lives only in the terminal layer, so it does not leak into
    // the editor.
    let shift_pgup = KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT);
    assert_eq!(km.resolve(&shift_pgup, KeyContext::Editor), None);

    // Global focus/zoom chords still work over the editor.
    let alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&alt_l, KeyContext::Editor),
        Some(Action::CycleFocusForward)
    );
    let ctrl_alt_z =
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&ctrl_alt_z, KeyContext::Editor),
        Some(Action::TogglePanelExpand)
    );
}

#[test]
fn ctrl_esc_is_additive_in_viewer() {
    // The app-wide "leave focus" chord is bound in non-PTY panels too, but
    // additively: bare Esc keeps working alongside it.
    let km = default_keymap();
    let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(
        km.resolve(&ctrl_esc, KeyContext::Viewer),
        Some(Action::ExitToExplorer)
    );
    assert_eq!(
        km.resolve(&esc, KeyContext::Viewer),
        Some(Action::ExitToExplorer)
    );
}

#[test]
fn context_falls_back_to_global() {
    let km = default_keymap();

    // Tab is bound per non-terminal context — resolves in Worktree but NOT
    // in Terminal (terminal layer has no Tab, neither does global).
    let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_tab, KeyContext::Worktree),
        Some(Action::CycleFocusForward)
    );
    assert_eq!(km.resolve(&key_tab, KeyContext::Terminal), None);

    // Alt+l resolves globally, including from the Terminal context.
    let key_alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&key_alt_l, KeyContext::Terminal),
        Some(Action::CycleFocusForward)
    );
}

#[test]
fn context_shadows_are_per_context() {
    let km = default_keymap();

    // 'c' = CherryPick in Worktree, ShowCommentList in Explorer.
    let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_c, KeyContext::Worktree),
        Some(Action::CherryPick)
    );
    assert_eq!(
        km.resolve(&key_c, KeyContext::Explorer),
        Some(Action::ShowCommentList)
    );
}

#[test]
fn explorer_walkthrough_show_and_generate_keys_resolve() {
    // w shows the walkthrough, Shift+W (re)generates it — the show/heavier
    // pairing. Both resolve in the Explorer context (generate rides the
    // global-action dispatch even though the chord lives in this layer).
    let km = default_keymap();
    let key_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_w, KeyContext::Explorer),
        Some(Action::ShowWalkthrough)
    );
    // Shift+W arrives as the resolved glyph 'W' + redundant SHIFT.
    let key_shift_w = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&key_shift_w, KeyContext::Explorer),
        Some(Action::GenerateWalkthrough)
    );
    // Alt+w is the force-regenerate escape hatch past the same-commit skip.
    let key_alt_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&key_alt_w, KeyContext::Explorer),
        Some(Action::ForceGenerateWalkthrough)
    );
}

#[test]
fn worktree_git_action_keys_resolve() {
    // The intentional 0.67 remap of the worktree panel's git actions to
    // more mnemonic chords. Pins the new bindings against silent regression.
    let km = default_keymap();
    let cases = [
        (KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty()), Action::PullWorktree),
        (KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()), Action::CherryPick),
        (KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()), Action::OpenPullRequest),
        // 'X' arrives as the resolved glyph 'X' + redundant SHIFT, which
        // keymap-core folds to match the "X" binding (cf. shift_g test).
        (KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT), Action::PruneWorktrees),
        // g/G are now go_to_top/bottom here too (was grab/ungrab), matching
        // every other panel; grab/ungrab moved to b/B ("branch", do/undo).
        (KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty()), Action::GoToTop),
        (KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT), Action::GoToBottom),
        (KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()), Action::GrabBranch),
        (KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT), Action::UngrabBranch),
    ];
    for (key, action) in cases {
        assert_eq!(km.resolve(&key, KeyContext::Worktree), Some(action), "{key:?}");
    }

    // The keys vacated by the remap are now unbound in the worktree panel
    // (no global fallback for bare u/v/P) — a deliberate no-op, not a
    // surprise reassignment.
    for key in [
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::empty()),
        KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
    ] {
        assert_eq!(km.resolve(&key, KeyContext::Worktree), None, "{key:?}");
    }
}

#[test]
fn shift_g_resolves_uppercase_binding() {
    let km = default_keymap();
    // A normal terminal delivers Shift+g as the resolved glyph 'G' + SHIFT;
    // keymap-core folds the redundant SHIFT, matching the "G" binding.
    let key = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&key, KeyContext::Worktree),
        Some(Action::GoToBottom)
    );
}

#[test]
fn shift_tab_is_cycle_backward() {
    let km = default_keymap();
    // BackTab and Tab+SHIFT both normalize to Tab+SHIFT in keymap-core.
    let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty());
    assert_eq!(
        km.resolve(&backtab, KeyContext::Worktree),
        Some(Action::CycleFocusBackward)
    );
    let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&shift_tab, KeyContext::Worktree),
        Some(Action::CycleFocusBackward)
    );
}

#[test]
fn ctrl_tab_switches_worktree() {
    let km = default_keymap();
    // Global layer, so it resolves in every non-terminal context. Ctrl+Tab
    // jumps worktrees while plain Tab still cycles panel focus.
    let ctrl_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_tab, KeyContext::Explorer),
        Some(Action::NextWorktree)
    );
    let plain_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
    assert_eq!(
        km.resolve(&plain_tab, KeyContext::Explorer),
        Some(Action::CycleFocusForward)
    );
    // Ctrl+Shift+Tab and Ctrl+BackTab both normalize to Ctrl+Shift+Tab.
    let ctrl_shift_tab =
        KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&ctrl_shift_tab, KeyContext::Explorer),
        Some(Action::PrevWorktree)
    );
    let ctrl_backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_backtab, KeyContext::Explorer),
        Some(Action::PrevWorktree)
    );
}

#[test]
fn ctrl_f_is_filename_search_in_viewer() {
    let km = default_keymap();
    let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&key, KeyContext::Viewer),
        Some(Action::SearchFilename)
    );
    assert_eq!(
        km.resolve(&key, KeyContext::ViewerDiffMode),
        Some(Action::SearchFilename)
    );
}

#[test]
fn viewer_c_is_add_comment() {
    let km = default_keymap();
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key, KeyContext::Viewer),
        Some(Action::AddComment)
    );
    assert_eq!(
        km.resolve(&key, KeyContext::ViewerDiffMode),
        Some(Action::AddComment)
    );
}

#[test]
fn f10_opens_the_menu_bar_from_every_context() {
    // The menu bar is only discoverable if its chord actually resolves. It is
    // also the one place a function key is used, so a parser that silently
    // dropped `f10` would leave the bar keyboard-unreachable while every other
    // binding kept working.
    let km = default_keymap();
    let f10 = KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE);

    assert_eq!(km.resolve(&f10, KeyContext::Global), Some(Action::FocusMenuBar));
    // Including over a PTY, where most time is spent.
    assert_eq!(
        km.resolve(&f10, KeyContext::Terminal),
        Some(Action::FocusMenuBar),
        "must fire while a terminal panel is focused"
    );
    assert_eq!(km.resolve(&f10, KeyContext::Explorer), Some(Action::FocusMenuBar));

    // And it must be advertised in the generated cheatsheet.
    assert_eq!(
        km.keys_in_layer(KeyContext::Global, Action::FocusMenuBar),
        vec!["f10".to_string()],
        "the cheatsheet reads this; an empty result means the binding is invisible"
    );
}
