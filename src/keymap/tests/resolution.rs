//! デフォルトのキーマップが、各 [KeyContext] においてチョードに対して期待
//! どおりのアクションに解決されることのテスト。terminal/editor での横取りと、
//! コンテキストからグローバルへのフォールバックを含む。

use super::super::map::DEFAULT_KEYBINDS;
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keymap_suite::ActionName;

#[test]
fn defaults_build_without_warnings() {
    let (_km, warnings) = KeyMap::with_warnings(&toml::Table::new());
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}

#[test]
fn every_default_action_name_resolves() {
    // default_keybinds.toml のタイポを検知するガード: 未知のアクション名は
    // デフォルトのパース時に警告として表面化するはずである。
    let build = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_name).unwrap();
    assert!(build.warnings.is_empty(), "{:?}", build.warnings);
}

#[test]
fn critical_defaults_resolve() {
    let km = default_keymap();

    // Quit は ctrl+q に移動した。素の q は未バインド（そのまま通過）なので
    // うっかりアプリを終了させることはもうできない。
    let key_ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&key_ctrl_q, KeyContext::Global),
        Some(Action::Quit)
    );
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

    // Ctrl+Esc はターミナルから離れる。
    let key_ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&key_ctrl_esc, KeyContext::Terminal),
        Some(Action::LeaveTerminal)
    );
}

#[test]
fn worktree_switch_and_zoom_aliases_resolve() {
    // alt+]/alt+[ は kitty プロトコルを使わない ctrl+tab ワークツリー切り替えの
    // エイリアスである。ctrl+alt+z はフォーカス中のパネルをズームし（tmux の
    // prefix z）、ctrl+alt によるペインサイズ変更ファミリーに加わる。
    let km = default_keymap();
    let cases = [
        (
            KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT),
            Action::NextWorktree,
        ),
        (
            KeyEvent::new(KeyCode::Char('['), KeyModifiers::ALT),
            Action::PrevWorktree,
        ),
        (
            KeyEvent::new(
                KeyCode::Char('z'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
            Action::TogglePanelExpand,
        ),
    ];
    for (key, action) in cases {
        assert_eq!(
            km.resolve(&key, KeyContext::Global),
            Some(action),
            "{key:?}"
        );
    }
}

#[test]
fn terminal_intercepts_only_firing_actions() {
    let km = default_keymap();

    // Quit（ctrl+q）はグローバルだがターミナルでは発火しない — このチョードは
    // アプリを終了させず PTY に届く（内側のプログラムが ctrl+q / XON を保てる
    // ように）。switch_repo（ctrl+r → シェルの逆方向検索）も同様。
    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_q, KeyContext::Global), Some(Action::Quit));
    assert_eq!(km.resolve(&ctrl_q, KeyContext::Terminal), None);
    let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_r, KeyContext::Global),
        Some(Action::SwitchRepo)
    );
    assert_eq!(km.resolve(&ctrl_r, KeyContext::Terminal), None);

    // フォーカス/ナビゲーションのチョードは PTY から奪い返される。
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

    // レンダリングされるヘルプは解決結果と食い違わない: ターミナルでは Quit に
    // チョードは宣伝されないが、ターミナルで発火するアクションはそのチョードを
    // 保つ。
    assert!(
        km.keys_for_action(KeyContext::Terminal, Action::Quit)
            .is_empty()
    );
    assert!(
        !km.keys_for_action(KeyContext::Terminal, Action::LeaveTerminal)
            .is_empty()
    );
}

#[test]
fn terminal_usable_actions_all_resolve_in_terminal() {
    // ターミナルで発火すると分類されたすべてのアクションは、実際にそこで解決
    // するチョードを持たなければならない — fires_in_terminal にバリアントを
    // 追加してバインドを忘れる（あるいはその逆の）ことへのガード。
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
    // 組み込みエディタはほぼすべてを vim/emacs に転送する。奪い返すのは
    // Ctrl+Esc（離脱）とターミナルで発火するグローバルチョードだけ。
    // エディタが必要とするキー — Esc、Ctrl+G、Shift+PageUp — はそのまま
    // 通過する（None）。
    let km = default_keymap();

    let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_esc, KeyContext::Editor),
        Some(Action::LeaveTerminal)
    );

    // 素の Esc → vim のモード変更。奪われてはならない。
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(km.resolve(&esc, KeyContext::Editor), None);

    // Ctrl+G は *terminal* レイヤーでは open_file_from_terminal、グローバルでは
    // search_full_text だが、どちらもエディタでは発火しないので、横取りされず
    // 内側のプログラムに届く。
    let ctrl_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_g, KeyContext::Editor), None);

    // スクロールバックは terminal レイヤーにしか存在しないので、エディタには
    // 漏れない。
    let shift_pgup = KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT);
    assert_eq!(km.resolve(&shift_pgup, KeyContext::Editor), None);

    // グローバルなフォーカス/ズームのチョードはエディタ上でも引き続き機能する。
    let alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&alt_l, KeyContext::Editor),
        Some(Action::CycleFocusForward)
    );
    let ctrl_alt_z = KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(
        km.resolve(&ctrl_alt_z, KeyContext::Editor),
        Some(Action::TogglePanelExpand)
    );
}

#[test]
fn ctrl_esc_is_additive_in_viewer() {
    // アプリ全体の「フォーカスを離れる」チョードは PTY 以外のパネルでも
    // バインドされているが、加算的である: 素の Esc も引き続き機能する。
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

    // Tab は非ターミナルの各コンテキストごとにバインドされている — Worktree
    // では解決するが Terminal では解決しない（terminal レイヤーに Tab は無く、
    // グローバルにも無い）。
    let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_tab, KeyContext::Worktree),
        Some(Action::CycleFocusForward)
    );
    assert_eq!(km.resolve(&key_tab, KeyContext::Terminal), None);

    // Alt+l は Terminal コンテキストからも含めてグローバルに解決する。
    let key_alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
    assert_eq!(
        km.resolve(&key_alt_l, KeyContext::Terminal),
        Some(Action::CycleFocusForward)
    );
}

#[test]
fn context_shadows_are_per_context() {
    let km = default_keymap();

    // 'c' は Worktree では CherryPick、Explorer では ShowCommentList。
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
fn worktree_git_action_keys_resolve() {
    // ワークツリーパネルの git アクションを、より覚えやすいチョードに付け替えた
    // 0.67 の意図的な変更。新しいバインディングを静かな退行から守るために固定する。
    let km = default_keymap();
    let cases = [
        (
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty()),
            Action::PullWorktree,
        ),
        (
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()),
            Action::CherryPick,
        ),
        (
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()),
            Action::OpenPullRequest,
        ),
        // 'X' は解決済みグリフ 'X' + 冗長な SHIFT として届き、keymap-core が
        // "X" のバインディングに合うよう畳み込む（shift_g のテストを参照）。
        (
            KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT),
            Action::PruneWorktrees,
        ),
        // g/G はここでも go_to_top/bottom になった（以前は grab/ungrab）。
        // 他のすべてのパネルと揃えるため; grab/ungrab は b/B（"branch"、do/undo）
        // に移動した。
        (
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty()),
            Action::GoToTop,
        ),
        (
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            Action::GoToBottom,
        ),
        (
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()),
            Action::GrabBranch,
        ),
        (
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT),
            Action::UngrabBranch,
        ),
    ];
    for (key, action) in cases {
        assert_eq!(
            km.resolve(&key, KeyContext::Worktree),
            Some(action),
            "{key:?}"
        );
    }

    // 付け替えで空いたキーは、ワークツリーパネルでは今は未バインドである
    // （素の u/v/P にグローバルへのフォールバックは無い）— これは意図的な
    // no-op であり、思いがけない再割り当てではない。
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
    // 通常の端末は Shift+g を解決済みグリフ 'G' + SHIFT として届ける。
    // keymap-core が冗長な SHIFT を畳み込み、"G" のバインディングに合う。
    let key = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&key, KeyContext::Worktree),
        Some(Action::GoToBottom)
    );
}

#[test]
fn shift_tab_is_cycle_backward() {
    let km = default_keymap();
    // BackTab と Tab+SHIFT はどちらも keymap-core で Tab+SHIFT に正規化される。
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
    // グローバルレイヤーなので、すべての非ターミナルコンテキストで解決する。
    // Ctrl+Tab はワークツリーを移動し、素の Tab は引き続きパネルフォーカスを
    // 巡回する。
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
    // Ctrl+Shift+Tab と Ctrl+BackTab はどちらも Ctrl+Shift+Tab に正規化される。
    let ctrl_shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
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
    // メニューバーは、そのチョードが実際に解決してこそ発見可能になる。ここは
    // ファンクションキーが使われる唯一の箇所でもあるので、f10 を静かに落とす
    // パーサがあれば、他のすべてのバインディングが動き続ける中でバーだけが
    // キーボードから到達不能になってしまう。
    let km = default_keymap();
    let f10 = KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE);

    assert_eq!(
        km.resolve(&f10, KeyContext::Global),
        Some(Action::FocusMenuBar)
    );
    // 多くの時間が費やされる PTY 上でも含めて。
    assert_eq!(
        km.resolve(&f10, KeyContext::Terminal),
        Some(Action::FocusMenuBar),
        "must fire while a terminal panel is focused"
    );
    assert_eq!(
        km.resolve(&f10, KeyContext::Explorer),
        Some(Action::FocusMenuBar)
    );

    // そして生成されるチートシートにも宣伝されなければならない。
    assert_eq!(
        km.keys_in_layer(KeyContext::Global, Action::FocusMenuBar),
        vec!["f10".to_string()],
        "the cheatsheet reads this; an empty result means the binding is invisible"
    );
}

#[test]
fn revidere_layer_resolves() {
    let km = default_keymap();
    let cases = [
        (
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            Action::NavigateDown,
        ),
        (
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()),
            Action::RevidereNextSection,
        ),
        (
            KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
            Action::RevidererPrevSection,
        ),
        (
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
            Action::Select,
        ),
        (
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty()),
            Action::ExitSubPanel,
        ),
        // 画面の切り替えは行き先ごとに別のキー。1 つのキーで交互に切り替えると、
        // 押した結果がいまどちらを出しているかに依存する。
        (
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()),
            Action::RevidereShowOverview,
        ),
        (
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty()),
            Action::RevidereShowSections,
        ),
    ];
    for (key, action) in cases {
        assert_eq!(
            km.resolve(&key, KeyContext::Revidere),
            Some(action),
            "{key:?}"
        );
    }
}

#[test]
fn explorer_show_and_analyze_keys_resolve() {
    let km = default_keymap();
    assert_eq!(
        km.resolve(
            &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty()),
            KeyContext::Explorer
        ),
        Some(Action::ShowRevidere)
    );
    assert_eq!(
        km.resolve(
            &KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT),
            KeyContext::Explorer
        ),
        Some(Action::AnalyzeRevidere)
    );
    assert_eq!(
        km.resolve(
            &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
            KeyContext::Explorer
        ),
        Some(Action::ForceAnalyzeRevidere)
    );
}
