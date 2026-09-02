//! キー 1 つの行き先を決める唯一の関数。段は menu → modal の top → PTY 転送 → keymap。

use conductor_core::keymap::Action;
use conductor_svc::pty::SessionKind;
use crossterm::event::KeyEvent;

use crate::effect::Effect;
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
    if ws.chrome.menu_open {
        return Routed::Ignored;
    }
    if let Some(top) = ws.modals.last_mut() {
        let ctx = crate::workspace::Ctx {
            theme: &ws.theme,
            keymap: &ws.keymap,
            config: &ws.config,
            repo: &ws.repo,
            focus: ws.focus,
        };
        return Routed::Effects(top.update(key, &ctx));
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

/// フォーカス移動など、パネルを跨ぐ Action の既定の解釈。パネルが先に消費した Action は来ない。
pub fn global_effects(ws: &Workspace, action: Action) -> Vec<Effect> {
    match action {
        Action::Quit => vec![Effect::Quit],
        Action::CycleFocusForward => vec![Effect::Focus(ws.focus.next())],
        Action::CycleFocusBackward => vec![Effect::Focus(ws.focus.prev())],
        Action::FocusWorktree => vec![Effect::Focus(Focus::Worktree)],
        Action::FocusExplorer => vec![Effect::Focus(Focus::Explorer)],
        Action::FocusViewer => vec![Effect::Focus(Focus::Viewer)],
        Action::FocusTerminalClaude => vec![Effect::Focus(Focus::TerminalClaude)],
        Action::FocusTerminalShell => vec![Effect::Focus(Focus::TerminalShell)],
        Action::ShowHelp => vec![Effect::PushModal(crate::modal::Modal::Help)],
        Action::NewClaudeCode => vec![Effect::NewSession(SessionKind::ClaudeCode)],
        Action::NewShell => vec![Effect::NewSession(SessionKind::Shell)],
        Action::RefreshWorktrees => vec![Effect::Spawn(crate::task::Task::ListWorktrees)],
        _ => vec![],
    }
}

impl PartialEq for Effect {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}
impl Eq for Effect {}

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
        ws.modals.push(Modal::Help);
        for k in [key(KeyCode::Char('j')), ctrl('q'), key(KeyCode::Char('あ'))] {
            assert!(matches!(route(&mut ws, k), Routed::Effects(_)), "{k:?}");
        }
        assert!(!ws.should_quit);
    }

    #[test]
    fn モーダルは_escで閉じる() {
        let mut ws = Workspace::for_test();
        ws.modals.push(Modal::Help);
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
