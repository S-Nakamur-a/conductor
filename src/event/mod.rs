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
mod global;
pub(crate) mod input_target;
pub(crate) mod mouse;
mod overlay;
mod overlay_helpers;
mod paste;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, WorktreeInputMode};
use crate::keymap::{Action, KeyContext};
use crate::menu::input::handle_menu_key;
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use self::dialogs::{handle_publish_confirm_key, handle_update_key};
use self::global::dispatch_global_action;
use self::overlay::*;
use crate::reflow::key::handle_reflow_key;
use crate::terminal::input::{forward_key_to_pty, spawn_terminal_session};
use crate::viewer::input::handle_viewer_key;
use crate::worktree::input::handle_worktree_key;

pub(crate) use self::clipboard::clipboard_paste;
pub(crate) use self::overlay_helpers::open_filename_search;

/// ディスパッチ用に統一したオーバーレイ/モーダルの状態。複数の
/// bool/enum チェックを単一の判別子に集約する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveOverlay {
    /// アップデートの確認/進行状況/失敗ダイアログ。
    UpdateState,
    /// GitHub への publish 確認ダイアログ。
    PublishConfirm,
    /// コメント詳細ポップアップ。
    CommentDetail,
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

/// [effective_overlay] が実際に読む App の状態。
///
/// 順位そのものは App を組み立てずに確かめたい。入れ替わっても落ちる場所は無く、
/// 「別のモーダルが開いているのに入力が違う所へ行く」形でしか現れないため。
#[derive(Clone, Copy, Default)]
struct OverlayFlags {
    update: bool,
    publish_confirm: bool,
    comment_detail: bool,
    review_input: bool,
    worktree_input: bool,
    active: ActiveOverlay,
    filename_search: bool,
    viewer_search: bool,
    review_search: bool,
    template_picker: bool,
}

impl From<&App> for OverlayFlags {
    fn from(app: &App) -> Self {
        Self {
            update: app.update.is_active(),
            publish_confirm: app.publish.confirm.is_some(),
            comment_detail: app.review_state.comment_detail_active,
            review_input: app.review_state.input_mode != ReviewInputMode::Normal,
            worktree_input: app.worktree_mgr.input_mode != WorktreeInputMode::Normal,
            active: app.overlays.active,
            filename_search: app.viewer.filename_search.filename_search_active,
            viewer_search: app.viewer.search.search_active,
            review_search: app.review_state.search_active,
            template_picker: app.review_state.template_picker_active,
        }
    }
}

/// 入力を消費すべき、唯一の有効なオーバーレイ/モーダルを判定する。
fn effective_overlay(f: OverlayFlags) -> EffectiveOverlay {
    if f.update {
        return EffectiveOverlay::UpdateState;
    }
    if f.publish_confirm {
        return EffectiveOverlay::PublishConfirm;
    }
    if f.comment_detail {
        return EffectiveOverlay::CommentDetail;
    }
    if f.review_input {
        return EffectiveOverlay::ReviewInput;
    }
    if f.worktree_input {
        return EffectiveOverlay::WorktreeInput;
    }
    match f.active {
        ActiveOverlay::None => {}
        other => return EffectiveOverlay::Active(other),
    }
    if f.filename_search {
        return EffectiveOverlay::FilenameSearch;
    }
    if f.viewer_search {
        return EffectiveOverlay::ViewerSearch;
    }
    if f.review_search {
        return EffectiveOverlay::ReviewSearch;
    }
    if f.template_picker {
        return EffectiveOverlay::ReviewTemplate;
    }
    EffectiveOverlay::None
}

pub use self::mouse::handle_mouse_event;
pub use self::paste::handle_paste_event;

/// キーイベントを1つ処理し、必要に応じてアプリケーション状態を更新する。
///
/// 内側 (メニュー・オーバーレイ) から外側 (フォーカス別ハンドラ) へ 6 段の
/// バブリングで問い合わせ、いずれかの段が `None` (消費) を返した時点で
/// 打ち切る。既定を消費にしてあるので、テキスト入力欄が最内にいる限り
/// IME 合成中のグリフがこの外側の段まで抜けてくることはない。
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    let Some(key) = stage_menu(app, key) else {
        return;
    };
    let Some(key) = stage_overlay(app, key) else {
        return;
    };
    let Some(key) = stage_panel_popup(app, key) else {
        return;
    };
    let Some(key) = stage_pty(app, key) else {
        return;
    };
    let Some(key) = stage_keymap(app, key) else {
        return;
    };
    stage_focus(app, key);
}

/// 段1: メニューバー — フォーカスされている間はすべてのキーを消費する。
fn stage_menu(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if app.menu.focus.is_active() {
        return handle_menu_key(app, key);
    }
    Some(key)
}

/// 段2: オーバーレイ / モーダルのディスパッチ — アクティブな間はすべてのキーを消費する。
fn stage_overlay(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    match effective_overlay(OverlayFlags::from(&*app)) {
        EffectiveOverlay::UpdateState => handle_update_key(app, key),
        EffectiveOverlay::PublishConfirm => handle_publish_confirm_key(app, key),
        EffectiveOverlay::CommentDetail => handle_comment_detail_key(app, key),
        EffectiveOverlay::ReviewInput => handle_review_input_key(app, key),
        EffectiveOverlay::WorktreeInput => handle_worktree_input_key(app, key),
        EffectiveOverlay::Active(overlay) => match overlay {
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
            ActiveOverlay::CommentList => app.explorer_key(key),
            ActiveOverlay::ThemePicker => handle_theme_picker_key(app, key),
            ActiveOverlay::RevidereConfirm => handle_revidere_confirm_key(app, key),
            // effective_overlay は Active(ActiveOverlay::None) を返さない。
            ActiveOverlay::None => Some(key),
        },
        EffectiveOverlay::FilenameSearch => handle_filename_search_key(app, key),
        EffectiveOverlay::ViewerSearch => handle_viewer_search_key(app, key),
        EffectiveOverlay::ReviewSearch => handle_review_search_key(app, key),
        EffectiveOverlay::ReviewTemplate => handle_review_template_key(app, key),
        EffectiveOverlay::None => Some(key), // パネルのディスパッチへフォールスルー。
    }
}

/// 段3: パネル内ポップアップ — references → hover モーダル (pinned) →
/// symbol アクション → symbol ヒントの順。
fn stage_panel_popup(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if app.code_nav.references.active {
        return handle_references_key(app, key);
    }

    // pinned なら、キーはモーダルスタックを操作し消費される。そうでなければ一時的な
    // 自動ポップアップで、どのキーでも消える (Esc は消費し、他はバブルさせる)。
    if app.code_nav.hover_info.pinned {
        return handle_hover_modal_key(app, key);
    }
    if app.code_nav.hover_info.info.is_some() || app.code_nav.hover_info.pending.is_some() {
        app.clear_hover();
        if key.code == KeyCode::Esc {
            return None;
        }
    }

    if app.code_nav.symbol_action.active {
        return handle_symbol_action_key(app, key);
    }

    if app.code_nav.symbol_hint.active
        && (app.code_nav.symbol_hint.pending.is_some()
            || !app.code_nav.symbol_hint.input.is_empty())
    {
        return handle_symbol_hint_key(app, key);
    }

    Some(key)
}

/// 段4: Reflow ビューと PTY フォーカス。フォーカス別ハンドラより先に置かないと
/// キーが Claude へ転送される。
fn stage_pty(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if (app.reflow.active && app.focus.current() == Focus::TerminalClaude)
        || app.focus.current().is_pty()
    {
        return dispatch_pty_key(app, key);
    }
    Some(key)
}

/// 段5: terminal 以外のパネル — keymap で解決する。
fn stage_keymap(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if let Some(action) = app.keymap.resolve(&key, app.focus.current().key_context())
        && dispatch_global_action(app, action)
    {
        return None;
    }
    Some(key)
}

/// 段6: フォーカス固有のキーバインド。
fn stage_focus(app: &mut App, key: KeyEvent) {
    match app.focus.current() {
        Focus::Worktree => {
            handle_worktree_key(app, key);
        }
        Focus::Explorer => {
            app.explorer_key(key);
        }
        Focus::Viewer => {
            handle_viewer_key(app, key);
        }
        Focus::Revidere => {
            crate::revidere::input::handle_revidere_key(app, key);
        }
        // is_pty() な Focus は段 4 (PTY) が必ず先に消費するため、
        // ここには到達しない。
        Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => {}
    }
}

/// Esc は最も深い階層を 1 段戻す (プレビュー → refs リスト → ポップアップ全体)。
/// Enter は選択中のプレビューを開くか、出ていればその位置へ飛ぶ。
fn handle_hover_modal_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    match key.code {
        KeyCode::Esc => {
            app.hover_pop_level();
        }
        KeyCode::Up | KeyCode::Char('k') => app.hover_refs_move(-1),
        KeyCode::Down | KeyCode::Char('j') => app.hover_refs_move(1),
        KeyCode::Enter => {
            let has_preview = app
                .code_nav
                .hover_info
                .refs
                .as_ref()
                .is_some_and(|r| r.preview.is_some());
            if has_preview {
                app.hover_jump_to_preview();
            } else if let Some(sel) = app.code_nav.hover_info.refs.as_ref().map(|r| r.selected) {
                app.open_hover_preview(sel);
            } else {
                // 参照一覧を開いていないときの Enter は、説明している定義へ飛ぶ。
                app.jump_to_hover_definition();
            }
        }
        _ => {}
    }
    None
}

/// app.focus.current().is_pty() か reflow-over-Claude が成り立つときにしか呼んではならない。
/// 段 4 ([stage_pty]) がそれを保証する。
fn dispatch_pty_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    // Reflow ビューはアクティブな間すべてのキーを消費するが、ペインのリサイズ/ズーム
    // (Ctrl+Alt+Arrow など) は素通しする。reflow の j/k とは衝突しない。
    if app.reflow.active && app.focus.current() == Focus::TerminalClaude {
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
            return None;
        }
        return handle_reflow_key(app, key);
    }

    if !app.focus.current().is_pty() {
        return None;
    }
    let pty_context = app.focus.current().key_context();

    // grab された worktree では、ナビゲーションキー以外の terminal 入力をすべてブロックする
    // (フォーカス切り替えは段 5 の keymap に任せる)。
    if app.is_selected_worktree_grabbed() {
        if let Some(Action::LeaveTerminal) = app.keymap.resolve(&key, pty_context) {
            app.set_focus(Focus::Explorer);
        }
        return None;
    }

    // keymap で解決し、マッチしなければ下の PTY へフォールスルーする。このパネルが内部の
    // プログラムから何を奪うかについて、keymap を唯一の正とする (手作業の許可リストを持たない)。
    if let Some(action) = app.keymap.resolve(&key, pty_context)
        && (handle_terminal_only_action(app, action) || dispatch_global_action(app, action))
    {
        return None;
    }

    // Ctrl+Q は Conductor の終了コードだが terminal 内では転送される (XON / フロー制御)
    // ので、実際の終了方法をフラッシュ表示してから通常どおり転送する。
    if key.code == KeyCode::Char('q')
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        app.set_status_info(
            "Ctrl+Q is sent to the terminal here. To quit Conductor: Ctrl+Esc to leave, then Ctrl+Q.".to_string(),
        );
    }

    let session_idx = match app.focus.current() {
        Focus::Editor => app.editor.as_ref().map(|e| e.session_idx),
        f => app.terminal.pane(f).and_then(|p| p.active_session),
    };
    if let Some(idx) = session_idx {
        forward_key_to_pty(app, idx, key);
    } else if key.code == KeyCode::Enter && app.focus.current() != Focus::Editor {
        spawn_terminal_session(app);
    }
    None
}

/// terminal の状態を要り、他のパネルでは意味を持たないので dispatch_global_action を
/// 経由できない。処理したら true。terminal にフォーカスがある間しか呼ばれない。
fn handle_terminal_only_action(app: &mut App, action: Action) -> bool {
    match action {
        Action::LeaveTerminal => {
            // editor が開いている間、Ctrl+Esc は editor と Claude をトグルする (チャットしてから
            // 戻れる)。それ以外は従来どおり terminal から Explorer へ抜ける。
            let target = if app.editor.is_some() {
                match app.focus.current() {
                    Focus::Editor => Focus::TerminalClaude,
                    _ => Focus::Editor,
                }
            } else {
                Focus::Explorer
            };
            app.set_focus(target);
        }
        Action::ScrollbackUp => {
            // ライブの Claude terminal での最初の上スクロールを横取りし、上限のある vt100
            // スクロールバックではなく無限スクロールバックの reflow ビューに入る。
            if app.focus.current() == Focus::TerminalClaude
                && app.terminal.claude.scroll == 0
                && !app.reflow.active
            {
                app.open_reflow();
                // open_reflow が打ち切ったときも無条件に消費すると、scroll_claude が 0 のままなので
                // 次に押しても同じ分岐で同じように失敗し、scroll-up が完全に死ぬ。ビューが実際に
                // 開いたときだけキーを消費する。
                if app.reflow.active {
                    return true;
                }
            }
            let Some(pane) = app.terminal.pane_mut(app.focus.current()) else {
                unreachable!()
            };
            let page = pane.size.0 as usize / 2;
            pane.scroll = pane.scroll.saturating_add(page.max(1));
        }
        Action::ScrollbackDown => {
            let Some(pane) = app.terminal.pane_mut(app.focus.current()) else {
                unreachable!()
            };
            let page = pane.size.0 as usize / 2;
            pane.scroll = pane.scroll.saturating_sub(page.max(1));
        }
        Action::ScrollbackTop => {
            // ScrollbackUp と同じ横取り: Claude ライブ表示から reflow へ直接ジャンプする。
            if app.focus.current() == Focus::TerminalClaude
                && app.terminal.claude.scroll == 0
                && !app.reflow.active
            {
                app.open_reflow();
                if app.reflow.active {
                    return true;
                }
            }
            if let Some(pane) = app.terminal.pane_mut(app.focus.current()) {
                pane.scroll = 1000;
            }
        }
        Action::SnapToLive => {
            if let Some(pane) = app.terminal.pane_mut(app.focus.current()) {
                pane.scroll = 0;
            }
        }
        Action::OpenFileFromTerminal => crate::terminal::input::open_file_from_terminal_output(app),
        Action::NextSession => app.cycle_terminal_session(true),
        Action::PrevSession => app.cycle_terminal_session(false),
        _ => return false,
    }
    true
}

// 「既定は消費」が守られているかは各ハンドラの match 網羅性を見るしかない。App::new が
// 実際に git 処理を行うため、単独 unit test するための pure-fn の切れ目が無い。

#[cfg(test)]
mod overlay_priority_tests {
    use super::*;

    /// 1 段ぶん: その段を立てる手続きと、そのとき選ばれるべきもの。
    type Rung = (fn(&mut OverlayFlags), EffectiveOverlay);

    /// 上から順に、入力を取る権利が強い。この並びが唯一の真実。
    const LADDER: &[Rung] = &[
        (|f| f.update = true, EffectiveOverlay::UpdateState),
        (
            |f| f.publish_confirm = true,
            EffectiveOverlay::PublishConfirm,
        ),
        (|f| f.comment_detail = true, EffectiveOverlay::CommentDetail),
        (|f| f.review_input = true, EffectiveOverlay::ReviewInput),
        (|f| f.worktree_input = true, EffectiveOverlay::WorktreeInput),
        (
            |f| f.active = ActiveOverlay::Help,
            EffectiveOverlay::Active(ActiveOverlay::Help),
        ),
        (
            |f| f.filename_search = true,
            EffectiveOverlay::FilenameSearch,
        ),
        (|f| f.viewer_search = true, EffectiveOverlay::ViewerSearch),
        (|f| f.review_search = true, EffectiveOverlay::ReviewSearch),
        (
            |f| f.template_picker = true,
            EffectiveOverlay::ReviewTemplate,
        ),
    ];

    #[test]
    fn 各段は単独で自分が選ばれる() {
        for (set, expected) in LADDER {
            let mut f = OverlayFlags::default();
            set(&mut f);
            assert_eq!(effective_overlay(f), *expected);
        }
    }

    #[test]
    fn 上の段は下の段すべてに勝つ() {
        for (i, (set_hi, expected)) in LADDER.iter().enumerate() {
            for (set_lo, lower) in &LADDER[i + 1..] {
                let mut f = OverlayFlags::default();
                set_hi(&mut f);
                set_lo(&mut f);
                assert_eq!(
                    effective_overlay(f),
                    *expected,
                    "{expected:?} より下の {lower:?} が勝っている"
                );
            }
        }
    }

    #[test]
    fn 何も開いていなければフォーカス中のパネルへ渡す() {
        assert_eq!(
            effective_overlay(OverlayFlags::default()),
            EffectiveOverlay::None
        );
    }
}
