//! イベント処理 — キーボード・マウスイベントをアプリケーションの
//! アクションへマッピングする。
//!
//! フォーカスベースのディスパッチ: Tab / Shift+Tab は terminal 以外の
//! パネル間を、Alt+h / Alt+l は terminal を含む全パネル間を巡回する。
//! オーバーレイのハンドラ (worktree 入力、cherry-pick など) が最優先。
//! terminal がフォーカスされているパネルは、キーをアクティブな PTY
//! セッションへ転送する。

mod clipboard;
mod dialogs;
mod explorer;
mod explorer_walkthrough;
mod global;
mod menu;
mod mouse;
mod overlay;
mod overlay_helpers;
mod paste;
pub mod reflow;
mod reflow_key;
mod scroll;
mod terminal;
mod viewer;
mod worktree;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, WorktreeInputMode};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use self::dialogs::{handle_publish_confirm_key, handle_update_key};
use self::explorer::handle_explorer_comment_list_key;
use self::explorer::handle_explorer_key;
use self::global::dispatch_global_action;
use self::menu::handle_menu_key;
use self::overlay::*;
use self::overlay_helpers::dismiss_overlays;
use self::reflow_key::handle_reflow_key;
use self::terminal::{forward_key_to_pty, spawn_terminal_session};
use self::viewer::handle_viewer_key;
use self::worktree::handle_worktree_key;

// 元は crate::event::X だったが、今は隣接するサブモジュールへ移った項目を
// re-export する。こうすることで、隣接モジュール側の既存の super::X 参照が
// 変更なしに解決され続ける。
pub(in crate::event) use self::clipboard::clipboard_paste;
pub(in crate::event) use self::overlay_helpers::open_filename_search;
pub(in crate::event) use self::scroll::{
    adjust_diff_list_scroll, adjust_tree_scroll, adjust_walkthrough_scroll,
};

// 有効なオーバーレイ

/// ディスパッチ用に統一したオーバーレイ/モーダルの状態。複数の
/// bool/enum チェックを単一の判別子に集約する。
enum EffectiveOverlay {
    /// アップデートの確認/進行状況/失敗ダイアログ。
    UpdateState,
    /// GitHub への publish 確認ダイアログ。
    PublishConfirm,
    /// コメント詳細ポップアップ。
    CommentDetail,
    /// walkthrough ステップ詳細ポップアップ (Explorer walkthrough ビューの領域)。
    WalkthroughDetail,
    /// レビューのテキスト入力 (追加/編集/返信)。
    ReviewInput,
    /// worktree のテキスト入力 (作成/確認/スマート)。
    WorktreeInput,
    /// ActiveOverlay のバリアント (switch-branch, cherry-pick など)。
    Active(ActiveOverlay),
    /// ファイル名検索のサブモーダル。
    FilenameSearch,
    /// Viewer 内ファイル検索のサブモーダル。
    ViewerSearch,
    /// レビューコメント検索のサブモーダル。
    ReviewSearch,
    /// レビューテンプレート選択のサブモーダル。
    ReviewTemplate,
    /// オーバーレイなし — フォーカスされたパネルへディスパッチする。
    None,
}

/// 入力を消費すべき、唯一の有効なオーバーレイ/モーダルを判定する。
fn effective_overlay(app: &App) -> EffectiveOverlay {
    if app.update.is_active() {
        return EffectiveOverlay::UpdateState;
    }
    if app.publish.confirm.is_some() {
        return EffectiveOverlay::PublishConfirm;
    }
    if app.review_state.comment_detail_active {
        return EffectiveOverlay::CommentDetail;
    }
    if app.viewer_state.explorer.walkthrough_detail_active {
        return EffectiveOverlay::WalkthroughDetail;
    }
    if app.review_state.input_mode != ReviewInputMode::Normal {
        return EffectiveOverlay::ReviewInput;
    }
    if app.worktree_mgr.input_mode != WorktreeInputMode::Normal {
        return EffectiveOverlay::WorktreeInput;
    }
    match app.overlays.active {
        ActiveOverlay::None => {}
        other => return EffectiveOverlay::Active(other),
    }
    if app.viewer_state.filename_search.filename_search_active {
        return EffectiveOverlay::FilenameSearch;
    }
    if app.viewer_state.search.search_active {
        return EffectiveOverlay::ViewerSearch;
    }
    if app.review_state.search_active {
        return EffectiveOverlay::ReviewSearch;
    }
    if app.review_state.template_picker_active {
        return EffectiveOverlay::ReviewTemplate;
    }
    EffectiveOverlay::None
}

/// テキスト入力欄が現在フォーカスされていて、印字可能な文字をリテラル
/// テキストとして挿入することを期待している場合に true。[handle_paste_event]
/// に列挙されている入力先すべてに対応する。あの関数と歩調を合わせてある:
/// あちらに宛先を追加したらここにも追加すること。なお
/// WorktreeInputMode::Confirming* の y/n サブモードはテキスト入力では
/// ないので、これは false になる点に注意。
fn is_text_input_active(app: &App) -> bool {
    if app.viewer_state.explorer.inline_reply_line.is_some()
        || app.review_state.input_mode != ReviewInputMode::Normal
        || app.review_state.search_active
        || app.viewer_state.search.search_active
        || app.viewer_state.filename_search.filename_search_active
    {
        return true;
    }
    if matches!(
        app.worktree_mgr.input_mode,
        WorktreeInputMode::CreatingWorktree
            | WorktreeInputMode::CreatingWorktreeBase
            | WorktreeInputMode::SmartDescription
    ) {
        return true;
    }
    matches!(
        app.overlays.active,
        ActiveOverlay::GrepSearch
            | ActiveOverlay::SwitchBranch
            | ActiveOverlay::CommandPalette
            | ActiveOverlay::OpenRepo
            | ActiveOverlay::PrInput
            | ActiveOverlay::History
            | ActiveOverlay::ResumeSession
    )
}

// 公開 API の re-export。
pub use self::mouse::handle_mouse_event;
pub use self::paste::handle_paste_event;

/// キーイベントを1つ処理し、必要に応じてアプリケーション状態を更新する。
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // 0. グローバルなフォーカス切り替え — テキスト以外のオーバーレイ
    // (y/n 確認、リスト選択) の上では有効だが、テキストフィールドが
    // フォーカスされている間は無効。フォーカス済みのテキストフィールドは
    // モーダルグラブであり、すべてのキーを握るので、グローバルフォーカス
    // 層は参照されず、コード入力中はコード (chord) も突き抜けられない
    // (抜けるにはまず Esc を押す)。
    //
    // これが IME 入力中のフォーカス奪取を止めている仕組み。kitty
    // keyboard protocol のもとでは、macOS は Option で合成した入力を
    // ALT ビット付きで報告するため、合成されたグリフ ('∞' → Claude
    // パネル) や Meta モードの数字 ('alt+5') が、そうでなければフォーカス
    // アクションとして解決され、入力中にフォーカスをかっさらってしまう。
    // is_text_input_active でグラブすることで、terminal がどんなキー形状
    // を出しうるか列挙することなく、このカテゴリ全体を一括で塞げる。
    // テキスト以外のオーバーレイでは is_text_input_active が false のため、
    // 引き続き突き抜けられる。
    if !is_text_input_active(app)
        && let Some(action) = app.keymap.resolve(&key, KeyContext::Global)
    {
        match action {
            Action::FocusWorktree
            | Action::FocusExplorer
            | Action::FocusExplorerDiffList
            | Action::FocusViewer
            | Action::FocusTerminalClaude
            | Action::FocusTerminalShell => {
                dismiss_overlays(app);
                dispatch_global_action(app, action);
                return;
            }
            _ => {}
        }
    }

    // 0.5. メニューバー — フォーカスされている間はすべてのキーを消費する
    //
    // オーバーレイのディスパッチより先に置いている。メニューはオーバーレイが
    // 何も出ていないときにしかアクティブにならない (開く条件は下でそちらを
    // ゲートにしている) ので、ここを先に通しても何もコストがかからず、
    // メニュー自身の Esc/矢印処理を一箇所にまとめておける。
    if app.menu.focus.is_active() {
        handle_menu_key(app, key);
        return;
    }

    // 1. オーバーレイ / モーダルのディスパッチ — アクティブな間はすべてのキーを消費する

    match effective_overlay(app) {
        EffectiveOverlay::UpdateState => {
            handle_update_key(app, key);
            return;
        }
        EffectiveOverlay::PublishConfirm => {
            handle_publish_confirm_key(app, key);
            return;
        }
        EffectiveOverlay::CommentDetail => {
            handle_comment_detail_key(app, key);
            return;
        }
        EffectiveOverlay::WalkthroughDetail => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ')
            ) {
                app.viewer_state.explorer.walkthrough_detail_active = false;
            }
            return;
        }
        EffectiveOverlay::ReviewInput => {
            handle_review_input_key(app, key);
            return;
        }
        EffectiveOverlay::WorktreeInput => {
            handle_worktree_input_key(app, key);
            return;
        }
        EffectiveOverlay::Active(overlay) => {
            match overlay {
                ActiveOverlay::SwitchBranch => handle_switch_branch_key(app, key),
                ActiveOverlay::Grab => handle_grab_key(app, key),
                ActiveOverlay::Prune => handle_prune_key(app, key),
                ActiveOverlay::CherryPick => handle_cherry_pick_key(app, key),
                ActiveOverlay::History => handle_history_key(app, key),
                ActiveOverlay::ResumeSession => handle_resume_session_key(app, key),
                ActiveOverlay::RepoSelector => handle_repo_selector_key(app, key),
                ActiveOverlay::OpenRepo => handle_open_repo_key(app, key),
                ActiveOverlay::PrInput => handle_pr_input_key(app, key),
                ActiveOverlay::GrepSearch => handle_grep_search_key(app, key),
                ActiveOverlay::Help => handle_help_key(app, key),
                ActiveOverlay::CommandPalette => handle_command_palette_key(app, key),
                ActiveOverlay::WorktreeSwitcher => handle_worktree_key(app, key),
                ActiveOverlay::CommentList => handle_explorer_comment_list_key(app, key),
                ActiveOverlay::ThemePicker => handle_theme_picker_key(app, key),
                ActiveOverlay::None => unreachable!(),
            }
            return;
        }
        EffectiveOverlay::FilenameSearch => {
            handle_filename_search_key(app, key);
            return;
        }
        EffectiveOverlay::ViewerSearch => {
            handle_viewer_search_key(app, key);
            return;
        }
        EffectiveOverlay::ReviewSearch => {
            handle_review_search_key(app, key);
            return;
        }
        EffectiveOverlay::ReviewTemplate => {
            handle_review_template_key(app, key);
            return;
        }
        EffectiveOverlay::None => {} // パネルのディスパッチへフォールスルー。
    }

    // 1b. References オーバーレイ (パネルレベルのポップアップで、OverlayManager の一部ではない)
    if app.code_nav.references.active {
        handle_references_key(app, key);
        return;
    }

    // 1b1. Hover モーダル。pinned (ユーザがクリックして固定した) 状態では、
    // キーはモーダルスタックを操作する — Esc は1段階戻る、矢印は refs
    // リストのナビゲーション、Enter は掘り下げ/ジャンプ — そしてキーは
    // 消費される。そうでなければ一時的な自動ポップアップであり、どの
    // キーでも消える (Esc は消費し、他のキーはポップアップが消えつつ
    // 通常どおりの役目を果たすようフォールスルーする)。
    if app.code_nav.hover_info.pinned {
        handle_hover_modal_key(app, key);
        return;
    }
    if app.code_nav.hover_info.info.is_some() || app.code_nav.hover_info.pending.is_some() {
        app.clear_hover();
        if key.code == KeyCode::Esc {
            return;
        }
    }

    // 1b2. Symbol アクションオーバーレイ (ヒント選択後)
    if app.code_nav.symbol_action.active {
        handle_symbol_action_key(app, key);
        return;
    }

    // 1b3. Symbol ヒントオーバーレイの入力 (ラベルの2文字目)
    if app.code_nav.symbol_hint.active && !app.code_nav.symbol_hint.input.is_empty() {
        handle_symbol_hint_key(app, key);
        return;
    }

    // 1b4/1c. Reflow トランスクリプトビューと PTY フォーカス
    // 下の Focus ディスパッチより先に置く必要がある。そうしないとキーが
    // Claude へ転送されてしまう。dispatch_pty_key に括り出すことで、
    // reflow-over-Claude のケースと素の PTY フォーカスのケースが1つの
    // コードパスを共有する。
    if (app.reflow.active && app.focus == Focus::TerminalClaude) || app.focus.is_pty() {
        dispatch_pty_key(app, key);
        return;
    }

    // 2. terminal 以外のパネル — keymap で解決する

    let context = match app.focus {
        Focus::Worktree => KeyContext::Worktree,
        Focus::Explorer => KeyContext::Explorer,
        Focus::Viewer => KeyContext::Viewer,
        Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => unreachable!(),
    };

    if let Some(action) = app.keymap.resolve(&key, context)
        && dispatch_global_action(app, action)
    {
        return;
    }

    // 3. フォーカス固有のキーバインド

    match app.focus {
        Focus::Worktree => handle_worktree_key(app, key),
        Focus::Explorer => handle_explorer_key(app, key),
        Focus::Viewer => handle_viewer_key(app, key),
        Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => unreachable!(),
    }
}

/// pinned 状態のインタラクティブな hover モーダルスタックのキー操作。
/// Esc は開いている最も深い階層を1段階戻す (プレビュー → refs リスト →
/// ポップアップ全体)。Up/Down (または k/j) は references の選択を動かす。
/// Enter は選択中の reference のプレビューを開くか、プレビューが既に
/// 表示されていればその位置へジャンプする。
fn handle_hover_modal_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.hover_pop_level();
        }
        KeyCode::Up | KeyCode::Char('k') => app.hover_refs_move(-1),
        KeyCode::Down | KeyCode::Char('j') => app.hover_refs_move(1),
        KeyCode::Enter => {
            let has_preview = app
                .code_nav.hover_info
                .refs
                .as_ref()
                .is_some_and(|r| r.preview.is_some());
            if has_preview {
                app.hover_jump_to_preview();
            } else if let Some(sel) =
                app.code_nav.hover_info.refs.as_ref().map(|r| r.selected)
            {
                app.open_hover_preview(sel);
            }
        }
        _ => {}
    }
}

/// PTY がフォーカスされているパネル (Claude/Shell/Editor) 向け、または
/// Claude terminal の上に重なる reflow トランスクリプトビュー向けに、
/// キーイベントをディスパッチする。呼び出し側は app.focus.is_pty() か
/// reflow-over-Claude の条件が成り立つときにしかこれを呼んではならない
/// — handle_key_event のパネルディスパッチがこれを保証する。
fn dispatch_pty_key(app: &mut App, key: KeyEvent) {
    // Reflow トランスクリプトビュー — アクティブな間はすべてのキーを
    // 消費する。ペインのリサイズ/ズーム/パネルオーバーレイは、スクロール
    // バック中でも引き続き効く — reflow の素のキーナビゲーション
    // (j/k/矢印) とは衝突しないので、reflow に黙って飲み込ませるのでは
    // なく、こうしたコード (Ctrl+Alt+Arrow など) は素通しする。
    if app.reflow.active && app.focus == Focus::TerminalClaude {
        if let Some(action) = app.keymap.resolve(&key, KeyContext::Terminal)
            && matches!(
                action,
                Action::ResizePaneLeft
                    | Action::ResizePaneRight
                    | Action::ResizePaneUp
                    | Action::ResizePaneDown
                    | Action::TogglePanelExpand
                    | Action::TogglePanelOverlay
            )
            && dispatch_global_action(app, action)
        {
            return;
        }
        handle_reflow_key(app, key);
        return;
    }

    if !app.focus.is_pty() {
        return;
    }
    let pty_context = app.focus.key_context();

    // 選択中の worktree が grab されている場合、ナビゲーションキー以外の
    // すべての terminal 入力をブロックする (フォーカス切り替えは上の §0 で
    // 扱い済み)。(editor は grab された worktree では決して開かないので、
    // 実際にはこれは Claude/Shell の terminal だけをガードしている。)
    if app.is_selected_worktree_grabbed() {
        // Esc で terminal を抜けることは許すが、それ以外はすべてブロックする。
        if let Some(Action::LeaveTerminal) = app.keymap.resolve(&key, pty_context) {
            app.set_focus(Focus::Explorer);
        }
        return;
    }

    // keymap で解決する (パネル層 + グローバルフォールバックで、terminal で
    // 発火するアクションだけに絞り込み済み)。terminal 専用のアクションは
    // terminal の状態を必要とし、それ以外は共有のグローバルディスパッチを
    // 再利用する。マッチしなければ下の PTY へフォールスルーする — このパネル
    // が内部のプログラムから何を奪うかについて、keymap が唯一の正とする
    // 情報源 (手作業で維持する許可リストは持たない)。
    if let Some(action) = app.keymap.resolve(&key, pty_context)
        && (handle_terminal_only_action(app, action) || dispatch_global_action(app, action))
    {
        return;
    }

    // 親切のためのヒント: Ctrl+Q は Conductor の終了コードだが、terminal
    // 内では内部のプログラムへ転送される (XON / フロー制御) ので、ここで
    // 押しても終了しない。実際の終了方法をフラッシュ表示してから、通常
    // どおり転送する。
    if key.code == KeyCode::Char('q')
        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
    {
        app.set_status_info(
            "Ctrl+Q is sent to the terminal here. To quit Conductor: Ctrl+Esc to leave, then Ctrl+Q.".to_string(),
        );
    }

    // 残りのキーはすべてアクティブな PTY セッションへ転送する。
    let session_idx = match app.focus {
        Focus::TerminalClaude => app.terminal.active_claude_session,
        Focus::TerminalShell => app.terminal.active_shell_session,
        Focus::Editor => app.editor.as_ref().map(|e| e.session_idx),
        _ => unreachable!(),
    };
    if let Some(idx) = session_idx {
        forward_key_to_pty(app, idx, key);
    } else if key.code == KeyCode::Enter && app.focus != Focus::Editor {
        spawn_terminal_session(app);
    }
}

/// terminal 専用のアクション (スクロールバック、離脱、ファイルを開く) を
/// 処理する。terminal の状態が必要で、他のどのパネルでも意味を持たないため
/// dispatch_global_action を経由できない。処理したら true を返す。それ以外
/// のアクションでは false を返す (呼び出し側はそれを dispatch_global_action
/// へ渡す)。terminal パネルがフォーカスされている間しか呼ばれないので、
/// unreachable!() の分岐は成立する。
fn handle_terminal_only_action(app: &mut App, action: Action) -> bool {
    match action {
        Action::LeaveTerminal => {
            // editor パネルが開いている間、Ctrl+Esc はそれと Claude を
            // トグルする (editor は開いたままなので、チャットしてから
            // 戻れる)。editor 自身からは Claude へ移る。それ以外の場合は、
            // 従来どおり terminal から Explorer へ抜ける。
            let target = if app.editor.is_some() {
                match app.focus {
                    Focus::Editor => Focus::TerminalClaude,
                    _ => Focus::Editor,
                }
            } else {
                Focus::Explorer
            };
            app.set_focus(target);
        }
        Action::ScrollbackUp => {
            // ライブの Claude terminal での最初の上スクロール
            // (scroll_claude == 0) を横取りし、上限のある vt100
            // スクロールバックバッファではなく無限スクロールバックの
            // reflow ビューに入る。
            if app.focus == Focus::TerminalClaude
                && app.terminal.scroll_claude == 0
                && !app.reflow.active
            {
                app.open_reflow();
                // open_reflow は、パネルに pinned なセッションがない、
                // またはそのログがディスク上に見当たらない場合、
                // (ステータス表示をフラッシュして) 処理を打ち切る。
                // このとき無条件にキーを消費したことにすると、ユーザは
                // 立ち往生していた: scroll_claude は 0 のままなので、
                // 次に押しても同じ分岐に再度入って同じように失敗する
                // — scroll-up が vt100 バッファへ縮退するのではなく
                // 完全に死んでしまっていた。ビューが実際に開いたときだけ
                // キーを消費したことにする。
                if app.reflow.active {
                    return true;
                }
            }
            let page = match app.focus {
                Focus::TerminalClaude => app.terminal.size_claude.0 as usize / 2,
                Focus::TerminalShell => app.terminal.size_shell.0 as usize / 2,
                _ => unreachable!(),
            };
            let scroll = match app.focus {
                Focus::TerminalClaude => &mut app.terminal.scroll_claude,
                Focus::TerminalShell => &mut app.terminal.scroll_shell,
                _ => unreachable!(),
            };
            *scroll = scroll.saturating_add(page.max(1));
        }
        Action::ScrollbackDown => {
            let page = match app.focus {
                Focus::TerminalClaude => app.terminal.size_claude.0 as usize / 2,
                Focus::TerminalShell => app.terminal.size_shell.0 as usize / 2,
                _ => unreachable!(),
            };
            let scroll = match app.focus {
                Focus::TerminalClaude => &mut app.terminal.scroll_claude,
                Focus::TerminalShell => &mut app.terminal.scroll_shell,
                _ => unreachable!(),
            };
            *scroll = scroll.saturating_sub(page.max(1));
        }
        Action::ScrollbackTop => {
            // ScrollbackUp と同じ横取り: Claude ライブ表示から reflow へ直接ジャンプする。
            if app.focus == Focus::TerminalClaude
                && app.terminal.scroll_claude == 0
                && !app.reflow.active
            {
                app.open_reflow();
                // ビューが開けなかった場合の扱いは ScrollbackUp と同じフォールスルー。
                if app.reflow.active {
                    return true;
                }
            }
            match app.focus {
                Focus::TerminalClaude => app.terminal.scroll_claude = 1000,
                Focus::TerminalShell => app.terminal.scroll_shell = 1000,
                _ => unreachable!(),
            }
        }
        Action::SnapToLive => match app.focus {
            Focus::TerminalClaude => app.terminal.scroll_claude = 0,
            Focus::TerminalShell => app.terminal.scroll_shell = 0,
            _ => unreachable!(),
        },
        Action::OpenFileFromTerminal => terminal::open_file_from_terminal_output(app),
        Action::NextSession => app.cycle_terminal_session(true),
        Action::PrevSession => app.cycle_terminal_session(false),
        _ => return false,
    }
    true
}

// NOTE: フォーカスグラブの判断は今、handle_key_event の §0 をゲートする
// is_text_input_active (App 状態の関数) に集約されている。ここで単独
// unit test するための安価な pure-fn の切れ目がない — App::new は実際に
// git 処理を行うため — なので、このファイル内の unit test ではなく
// 手動/統合テストでカバーしている。
