//! キー 1 つの行き先を決める唯一の関数。段は menu → modal の top → パネルの 2 打鍵目
//! → PTY 転送 → keymap。
//!
//! 2 打鍵目の段があるのは、折りたたみ (za/zc/zo/zm/zr/zR/zM) の語彙をキーマップに
//! 載せていないため。キーマップに載せると 7 つのアクションが増え、しかも 1 打鍵目を
//! 押した後だけ意味を持つという条件が表に出せない。

use conductor_core::keymap::Action;
use crossterm::event::KeyEvent;

use crate::effect::Effect;
use crate::modal::{Modal, palette::Palette};
use crate::workspace::{Focus, Workspace};

#[derive(Debug, PartialEq, Eq)]
pub enum Routed {
    Effects(Vec<Effect>),
    /// PTY へそのまま書く。
    ForwardToPty(KeyEvent),
    /// keymap で解決した Action をフォーカス中のパネルへ。
    Action(Action),
    Ignored,
}

pub fn route(ws: &mut Workspace, key: KeyEvent) -> Routed {
    if ws.chrome.menu.is_active() {
        return Routed::Effects(crate::menu::key(ws, key));
    }
    if !ws.modals.is_empty() {
        let key_context = ws.key_context();
        let root = ws.panels.viewer.root().to_path_buf();
        let Workspace {
            modals,
            theme,
            keymap,
            config,
            repo,
            review,
            focus,
            ..
        } = ws;
        let ctx = crate::workspace::Ctx {
            theme,
            keymap,
            config,
            repo,
            review,
            root: &root,
            focus: *focus,
            key_context,
        };
        let top = modals.last_mut().expect("empty をここまでに弾いている");
        return Routed::Effects(top.update(key, &ctx));
    }
    if ws.focus == Focus::Viewer && ws.panels.viewer.awaiting_chord() {
        return Routed::Effects(ws.panels.viewer.chord_key(key));
    }
    let action = ws.keymap.resolve(&key, ws.key_context());
    if ws.focus.is_pty() {
        return match action {
            Some(action) if action.fires_in_terminal() => Routed::Action(action),
            _ => Routed::ForwardToPty(key),
        };
    }
    match action {
        Some(action) => Routed::Action(action),
        None => Routed::Ignored,
    }
}

/// パネルが消費しなかった Action の解釈。ほとんどは同じ意味のコマンドがあるので
/// [crate::command::execute] の 1 本に落ち、ここに残るのはコマンドを持たない語彙だけ。
pub fn global_effects(ws: &Workspace, action: Action) -> Vec<Effect> {
    match action {
        Action::CycleFocusForward => vec![Effect::Focus(ws.focus.next())],
        Action::CycleFocusBackward => vec![Effect::Focus(ws.focus.prev())],
        Action::FocusMenuBar => vec![Effect::FocusMenuBar],
        Action::CommandPalette => vec![Effect::PushModal(Modal::Palette(Palette::default()))],
        Action::OpenCommentList => vec![crate::comment_list::open_modal()],
        action => crate::command::COMMANDS
            .iter()
            .find(|c| c.action == Some(action))
            .map(|c| vec![Effect::Command(c.id)])
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal::Modal;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn モーダルが開いていれば_topが全てのキーを消費する() {
        let mut ws = Workspace::for_test();
        ws.modals
            .push(Modal::Help(crate::modal::help::Help::open(ws.focus)));
        for k in [key(KeyCode::Char('j')), ctrl('q'), key(KeyCode::Char('あ'))] {
            assert!(matches!(route(&mut ws, k), Routed::Effects(_)), "{k:?}");
        }
        assert!(!ws.should_quit);
    }

    #[test]
    fn モーダルは_escで閉じる() {
        let mut ws = Workspace::for_test();
        ws.modals
            .push(Modal::Help(crate::modal::help::Help::open(ws.focus)));
        assert_eq!(
            route(&mut ws, key(KeyCode::Esc)),
            Routed::Effects(vec![Effect::PopModal])
        );
    }

    #[test]
    fn ptyフォーカスでは_fires_in_terminalな_actionだけ横取りする() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::TerminalClaude;
        assert!(matches!(
            route(&mut ws, ctrl('p')),
            Routed::Action(Action::CommandPalette)
        ));
        assert!(matches!(
            route(&mut ws, key(KeyCode::Char('j'))),
            Routed::ForwardToPty(_)
        ));
        assert!(matches!(route(&mut ws, ctrl('q')), Routed::ForwardToPty(_)));
    }

    #[test]
    fn パネルフォーカスでは_keymapで解決する() {
        let mut ws = Workspace::for_test();
        ws.focus = Focus::Explorer;
        assert!(matches!(
            route(&mut ws, ctrl('q')),
            Routed::Action(Action::Quit)
        ));
        assert_eq!(route(&mut ws, key(KeyCode::Char('あ'))), Routed::Ignored);
    }

    #[test]
    fn フォーカスの輪() {
        let mut f = Focus::Worktree;
        let mut seen = vec![f];
        for _ in 0..5 {
            f = f.next();
            seen.push(f);
        }
        assert_eq!(
            seen,
            [
                Focus::Worktree,
                Focus::Explorer,
                Focus::Viewer,
                Focus::TerminalClaude,
                Focus::TerminalShell,
                Focus::Explorer
            ]
        );
        assert_eq!(Focus::Revidere.next(), Focus::Explorer);
        assert_eq!(Focus::Revidere.prev(), Focus::Explorer);
    }
}
