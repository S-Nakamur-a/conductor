use super::*;
use KeyContext::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::ALT)
}

/// 端末が Shift+g を届ける形: 解決済みグリフ 'G' + SHIFT。
fn shift(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::SHIFT)
}

fn user(toml: &str) -> toml::Table {
    toml::from_str(toml).unwrap()
}

#[test]
fn 既定は警告なしで組み上がる() {
    let build = keymap_suite::from_toml_str(map::DEFAULT_KEYBINDS, Action::from_name).unwrap();
    assert!(build.warnings.is_empty(), "{:?}", build.warnings);
    let (_, warnings) = KeyMap::with_warnings(&toml::Table::new());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn 下区画の一覧はどの層からでも切り替えられる() {
    let km = KeyMap::default();
    let views = [
        (ch('d'), Action::ShowDiffList),
        (ch('c'), Action::ShowCommentList),
        (ch('t'), Action::ShowTourStops),
    ];
    let layers = [
        Explorer,
        ExplorerDiffList,
        ExplorerCommitLog,
        ExplorerCommentList,
        ExplorerTourStops,
    ];
    for (key, action) in views {
        for layer in layers {
            let own_view = matches!(
                (layer, action),
                (ExplorerDiffList | ExplorerCommitLog, Action::ShowDiffList)
                    | (ExplorerCommentList, Action::ShowCommentList)
                    | (ExplorerTourStops, Action::ShowTourStops)
            );
            assert!(
                km.resolve(&key, layer) == Some(action) || own_view,
                "{layer:?} から {action:?} へ行けない"
            );
        }
    }
}

#[test]
fn レビューのキーだった_w_は_explorer_で何にも当たらない() {
    let km = KeyMap::default();
    for key in [ch('w'), shift('W'), alt('w')] {
        for layer in [Explorer, ExplorerDiffList, ExplorerCommentList] {
            assert_eq!(km.resolve(&key, layer), None, "{layer:?} {key:?}");
        }
    }
}

#[test]
fn 既定のキーが解決する() {
    let km = KeyMap::default();
    let cases = [
        (Global, ctrl('q'), Action::Quit),
        (Global, ctrl('n'), Action::NewClaudeCode),
        (Global, ctrl('r'), Action::SwitchRepo),
        (
            Global,
            key(KeyCode::F(10), KeyModifiers::NONE),
            Action::FocusMenuBar,
        ),
        (Global, alt(']'), Action::NextWorktree),
        (Global, alt('['), Action::PrevWorktree),
        (Global, alt('1'), Action::FocusSessions),
        (Global, ctrl('w'), Action::FocusWorktree),
        (
            Global,
            key(
                KeyCode::Char('z'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
            Action::TogglePanelExpand,
        ),
        // macOS の Option がグリフを送る端末向けの複数バイト文字。
        (Global, ch('˙'), Action::CycleFocusBackward),
        (Global, ch('¬'), Action::CycleFocusForward),
        (Global, ch('¡'), Action::FocusSessions),
        (Global, ch('§'), Action::FocusTerminalShell),
        (Global, ch('†'), Action::OpenThemePicker),
        (Fleet, ch('j'), Action::NavigateDown),
        (Fleet, ch('G'), Action::GoToBottom),
        (
            Fleet,
            key(KeyCode::Enter, KeyModifiers::NONE),
            Action::Select,
        ),
        (
            Fleet,
            key(KeyCode::Esc, KeyModifiers::NONE),
            Action::ExitSubPanel,
        ),
        (Fleet, shift('D'), Action::StopSession),
        (Fleet, ctrl('n'), Action::NewSessionMenu),
        (Fleet, ctrl('g'), Action::ShowFleetChanges),
        (Fleet, ctrl('a'), Action::ShowFleetArtifacts),
        (Fleet, shift('J'), Action::NextRepo),
        (
            Fleet,
            key(KeyCode::Down, KeyModifiers::SHIFT),
            Action::NextRepo,
        ),
        (Fleet, shift('K'), Action::PrevRepo),
        (Fleet, shift('!'), Action::GoToWaiting),
        (
            Fleet,
            key(KeyCode::Up, KeyModifiers::SHIFT),
            Action::PrevRepo,
        ),
        (Worktree, ch('j'), Action::NavigateDown),
        (
            Worktree,
            key(KeyCode::Tab, KeyModifiers::NONE),
            Action::CycleFocusForward,
        ),
        (Worktree, ch('c'), Action::CherryPick),
        (Worktree, ch('p'), Action::PullWorktree),
        (Worktree, ch('o'), Action::OpenPullRequest),
        (Worktree, shift('X'), Action::PruneWorktrees),
        (Worktree, ch('g'), Action::GoToTop),
        (Worktree, shift('G'), Action::GoToBottom),
        (Worktree, ch('b'), Action::GrabBranch),
        (Worktree, shift('B'), Action::UngrabBranch),
        (Explorer, ch('c'), Action::ShowCommentList),
        (
            Explorer,
            key(KeyCode::Tab, KeyModifiers::CONTROL),
            Action::NextWorktree,
        ),
        (
            Explorer,
            key(KeyCode::Tab, KeyModifiers::NONE),
            Action::CycleFocusForward,
        ),
        (
            Explorer,
            key(KeyCode::F(10), KeyModifiers::NONE),
            Action::FocusMenuBar,
        ),
        (Viewer, ctrl('f'), Action::SearchFilename),
        (Viewer, ch('c'), Action::AddComment),
        (
            Viewer,
            key(KeyCode::Esc, KeyModifiers::NONE),
            Action::ExitToExplorer,
        ),
        (
            Viewer,
            key(KeyCode::Esc, KeyModifiers::CONTROL),
            Action::ExitToExplorer,
        ),
        (ViewerDiffMode, ctrl('f'), Action::SearchFilename),
        (ViewerDiffMode, ch('c'), Action::AddComment),
        (
            ViewerDiffMode,
            key(KeyCode::Enter, KeyModifiers::NONE),
            Action::ExpandContext,
        ),
        (
            ViewerDiffMode,
            key(KeyCode::Enter, KeyModifiers::SHIFT),
            Action::ExpandAllContext,
        ),
        (
            Terminal,
            key(KeyCode::Esc, KeyModifiers::CONTROL),
            Action::LeaveTerminal,
        ),
        (Terminal, alt('l'), Action::CycleFocusForward),
        (
            Terminal,
            key(KeyCode::F(10), KeyModifiers::NONE),
            Action::FocusMenuBar,
        ),
    ];
    for (context, event, action) in cases {
        assert_eq!(
            km.resolve(&event, context),
            Some(action),
            "{context:?} {event:?}"
        );
    }
}

/// keymap-core はグリフをそのまま信じ、冗長な単独 SHIFT だけを落とす。
/// 'g'+SHIFT は 'G' に直されず、BackTab は shift+tab と同じ。
#[test]
fn チョードは正規化してから引く() {
    let km = KeyMap::default();
    let cases = [
        (Worktree, shift('g'), Action::GoToTop),
        (Worktree, shift('G'), Action::GoToBottom),
        (
            Worktree,
            key(KeyCode::BackTab, KeyModifiers::NONE),
            Action::CycleFocusBackward,
        ),
        (
            Worktree,
            key(KeyCode::Tab, KeyModifiers::SHIFT),
            Action::CycleFocusBackward,
        ),
        (
            Explorer,
            key(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            Action::PrevWorktree,
        ),
        (
            Explorer,
            key(KeyCode::BackTab, KeyModifiers::CONTROL),
            Action::PrevWorktree,
        ),
    ];
    for (context, event, action) in cases {
        assert_eq!(
            km.resolve(&event, context),
            Some(action),
            "{context:?} {event:?}"
        );
    }
}

#[test]
fn 割り当ての無いキーは素通しする() {
    let km = KeyMap::default();
    let mut cases = vec![
        (Global, ch('q')),
        // 他の修飾キーがある間は SHIFT を保つので、alt+1 に畳まれない。
        (
            Global,
            key(KeyCode::Char('1'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        ),
        // 中立な表現を持たないキーは KeyInput に変換できず、panic せずに通す。
        (Terminal, key(KeyCode::CapsLock, KeyModifiers::NONE)),
        (Terminal, key(KeyCode::Tab, KeyModifiers::NONE)),
        (Worktree, ch('u')),
        (Worktree, ch('v')),
        (Worktree, shift('P')),
    ];
    cases.extend((2..=7).map(|n| (Global, key(KeyCode::F(n), KeyModifiers::NONE))));
    for (context, event) in cases {
        assert_eq!(km.resolve(&event, context), None, "{context:?} {event:?}");
    }
}

#[test]
fn ptyを持つ文脈が奪うのは発火するアクションだけ() {
    let km = KeyMap::default();
    let esc = key(KeyCode::Esc, KeyModifiers::NONE);
    let ctrl_esc = key(KeyCode::Esc, KeyModifiers::CONTROL);
    let ctrl_alt_z = key(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    let shift_pgup = key(KeyCode::PageUp, KeyModifiers::SHIFT);
    let cases = [
        (Terminal, ctrl('q'), None),
        (Terminal, ctrl('r'), None),
        (Terminal, ctrl('n'), None),
        (Fleet, ch('n'), None),
        (Terminal, ctrl('p'), Some(Action::CommandPalette)),
        (Terminal, alt(']'), Some(Action::NextWorktree)),
        (Terminal, ctrl_esc, Some(Action::LeaveTerminal)),
        (Editor, ctrl_esc, Some(Action::LeaveTerminal)),
        (Editor, esc, None),
        (Editor, ctrl('g'), None),
        (Editor, shift_pgup, None),
        (Editor, alt('l'), Some(Action::CycleFocusForward)),
        (Editor, ctrl_alt_z, Some(Action::TogglePanelExpand)),
    ];
    for (context, event, expected) in cases {
        assert_eq!(
            km.resolve(&event, context),
            expected,
            "{context:?} {event:?}"
        );
    }
}

#[test]
fn ヘルプの逆引きはfires_in_terminalと一致する() {
    let km = KeyMap::default();
    for &action in Action::ALL {
        let in_terminal = km.keys_for_action(Terminal, action);
        assert_eq!(
            in_terminal.is_empty(),
            !action.fires_in_terminal(),
            "{action:?} in terminal: {in_terminal:?}"
        );
        let in_editor = km.keys_for_action(Editor, action);
        assert!(
            action.fires_in_terminal() || in_editor.is_empty(),
            "{action:?} in editor: {in_editor:?}"
        );
    }
}

#[test]
fn 逆引きは正規形の綴りを返す() {
    let km = KeyMap::default();
    let for_action = [
        (Viewer, Action::ScrollHalfPageDown, vec!["ctrl+d"]),
        (Worktree, Action::NavigateDown, vec!["down", "j"]),
        (Worktree, Action::CommandPalette, vec![":", "ctrl+p"]),
    ];
    for (context, action, expected) in for_action {
        assert_eq!(
            km.keys_for_action(context, action),
            expected,
            "{context:?} {action:?}"
        );
    }
    let in_layer = [
        (Global, Action::FocusMenuBar, vec!["f10"]),
        (Worktree, Action::CommandPalette, vec![":"]),
        (Worktree, Action::Quit, vec![]),
    ];
    for (context, action, expected) in in_layer {
        assert_eq!(
            km.keys_in_layer(context, action),
            expected,
            "{context:?} {action:?}"
        );
    }
}

#[test]
fn アクション名は全バリアントで往復する() {
    for &action in Action::ALL {
        assert_eq!(Action::from_name(action.name()), Some(action));
    }
}

#[test]
fn 語彙から外したアクション名は解釈されない() {
    for name in [
        "go_to_definition",
        "go_to_implementation",
        "find_references",
        "update_and_restart",
        "check_for_update",
        "toggle_panel_overlay",
        "show_revidere",
        "analyze_revidere",
        "force_analyze_revidere",
        "revidere_next_section",
    ] {
        assert_eq!(Action::from_name(name), None, "{name}");
    }
}

#[test]
fn revidereの設定が残っていても警告で済み_ほかのキーは効く() {
    let leftover = user(
        "[layers.explorer]\n\"w\" = \"show_revidere\"\n\"W\" = \"analyze_revidere\"\n\"x\" = \"navigate_down\"\n\
         [layers.revidere]\n\"j\" = \"navigate_down\"",
    );
    let (km, warnings) = KeyMap::with_warnings(&leftover);
    let unknown_actions = warnings
        .iter()
        .filter(|w| matches!(w, KeybindWarning::UnknownAction { action, .. } if action.contains("revidere")))
        .count();
    assert_eq!(unknown_actions, 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::UnknownLayer { layer } if layer == "revidere")),
        "{warnings:?}"
    );
    assert_eq!(km.resolve(&ch('x'), Explorer), Some(Action::NavigateDown));
    assert_eq!(km.resolve(&ch('j'), Explorer), Some(Action::NavigateDown));
}

#[test]
fn ユーザ設定は既定に重なる() {
    let cases = [
        (
            "[layers.worktree]\n\"n\" = \"navigate_down\"",
            Worktree,
            ch('n'),
            Some(Action::NavigateDown),
        ),
        (
            "[layers.worktree]\n\"n\" = \"navigate_down\"",
            Worktree,
            ch('j'),
            Some(Action::NavigateDown),
        ),
        (
            "[layers.worktree]\n\"g\" = \"grab_branch\"",
            Worktree,
            ch('g'),
            Some(Action::GrabBranch),
        ),
        ("[keys]\n\"ctrl+q\" = false", Global, ctrl('q'), None),
        (
            "[keys]\n\"ctrl+q\" = false",
            Global,
            ctrl('n'),
            Some(Action::NewClaudeCode),
        ),
        ("[layers.worktree]\n\"c\" = false", Worktree, ch('c'), None),
    ];
    for (toml, context, event, expected) in cases {
        let (km, warnings) = KeyMap::with_warnings(&user(toml));
        assert!(warnings.is_empty(), "{toml}: {warnings:?}");
        assert_eq!(
            km.resolve(&event, context),
            expected,
            "{toml} {context:?} {event:?}"
        );
    }
}

#[test]
fn 全ての文脈の層をユーザが上書きできる() {
    let f9 = key(KeyCode::F(9), KeyModifiers::NONE);
    for context in KeyContext::PANELS {
        let toml = format!(
            "[layers.{}]\n\"f9\" = \"focus_viewer\"",
            context.layer_name()
        );
        let (km, warnings) = KeyMap::with_warnings(&user(&toml));
        assert!(warnings.is_empty(), "{context:?}: {warnings:?}");
        assert_eq!(
            km.resolve(&f9, context),
            Some(Action::FocusViewer),
            "{context:?}"
        );
    }
}

type WarningMatcher = fn(&KeybindWarning) -> bool;

#[test]
fn ユーザ設定の問題は警告になる() {
    let cases: [(&str, WarningMatcher); 4] = [
        (
            "[keys]\n\"ctrl+z\" = \"frobnicate\"",
            |w| matches!(w, KeybindWarning::UnknownAction { action, .. } if action == "frobnicate"),
        ),
        ("[worktree]\nnavigate_down = \"j\"", |w| {
            matches!(w, KeybindWarning::InvalidConfig { .. })
        }),
        (
            "[keys]\n\"ctrl+x\" = \"quit\"\n\"control+x\" = \"show_help\"",
            |w| matches!(w, KeybindWarning::Conflict { chord } if chord == "ctrl+x"),
        ),
        (
            "[layers.bogus]\n\"j\" = \"navigate_down\"",
            |w| matches!(w, KeybindWarning::UnknownLayer { layer } if layer == "bogus"),
        ),
    ];
    for (toml, expected) in cases {
        let (_, warnings) = KeyMap::with_warnings(&user(toml));
        assert!(warnings.iter().any(expected), "{toml}: {warnings:?}");
    }

    let conflict = "[keys]\n\"ctrl+x\" = \"quit\"\n\"control+x\" = \"show_help\"";
    let (km, _) = KeyMap::with_warnings(&user(conflict));
    assert!(matches!(
        km.resolve(&ctrl('x'), Global),
        Some(Action::Quit | Action::ShowHelp)
    ));
}

#[test]
fn 一覧の層は_worktree_の操作を拾わない() {
    let km = KeyMap::default();
    for c in ['x', 'w', 'm', 'R', 'b'] {
        assert_eq!(km.resolve(&ch(c), Fleet), None, "{c}");
    }
}

#[test]
fn 層が同じキーを別の操作に束ねていれば_その文脈の案内に出さない() {
    let km = KeyMap::default();
    assert!(
        km.keys_for_action(Global, Action::NewClaudeCode)
            .contains(&"ctrl+n".to_string())
    );
    assert!(
        !km.keys_for_action(Fleet, Action::NewClaudeCode)
            .contains(&"ctrl+n".to_string())
    );
    assert!(
        !km.keys_for_action(Viewer, Action::OpenRepo)
            .contains(&"ctrl+o".to_string())
    );
    assert!(
        km.keys_for_action(Fleet, Action::CommandPalette)
            .contains(&"ctrl+p".to_string())
    );
}
