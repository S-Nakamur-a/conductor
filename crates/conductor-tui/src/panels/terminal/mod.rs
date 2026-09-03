//! Claude Code とシェルの PTY パネル。
//!
//! PTY だけは svc の Event 経路に乗らない (バイト列が描画より高頻度で届く) ので、
//! [PtyStore] はこのパネルが直接持ち、描画のたびに画面をロックして読む。

pub mod ansi;
pub mod render;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use conductor_core::config::Config;
use conductor_core::keymap::Action;
use conductor_svc::pty::{Launch, PtyStore, SessionKind, Spawn};
use conductor_svc::watch::{CcState, WatchEvent};
use crossterm::event::KeyEvent;

use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::task::Task;
use crate::workspace::{Ctx, Focus, StatusLevel};

/// セッションのラベルに使う番号。同じ worktree で空いている一番小さいものを取る。
const SESSION_SLOTS: usize = 9;

/// 端末パネル 1 枚。
pub struct Pane {
    kind: SessionKind,
    /// 映しているセッションの id。添字は削除でずれるので持たない。
    session: Option<String>,
    /// 最後に PTY へ渡した内容領域の大きさ (行, 桁)。
    size: (u16, u16),
    /// スクロールバックのオフセット。0 が最新。
    scroll: usize,
}

impl Pane {
    fn new(kind: SessionKind, size: (u16, u16)) -> Self {
        Self {
            kind,
            session: None,
            size,
            scroll: 0,
        }
    }

    /// 映すセッションを変える。スクロール位置は今のセッションのものなので落とす。
    fn show(&mut self, session: Option<String>) {
        self.session = session;
        self.scroll = 0;
    }
}

pub struct TerminalPanel {
    pty: PtyStore,
    claude: Pane,
    shell: Pane,
    /// 選択中の worktree。セッションの絞り込みと spawn の作業ディレクトリ。
    worktree: Option<PathBuf>,
    waiting: HashSet<PathBuf>,
    active: HashSet<PathBuf>,
}

impl TerminalPanel {
    pub fn new(config: &Config) -> Self {
        Self {
            pty: PtyStore::new(
                config.terminal.active_scrollback,
                config.terminal.inactive_scrollback,
            ),
            claude: Pane::new(SessionKind::ClaudeCode, (24, 80)),
            shell: Pane::new(SessionKind::Shell, (6, 80)),
            worktree: None,
            waiting: HashSet::new(),
            active: HashSet::new(),
        }
    }

    pub fn is_waiting(&self, worktree: &Path) -> bool {
        self.waiting.contains(worktree)
    }

    pub fn is_active(&self, worktree: &Path) -> bool {
        self.active.contains(worktree)
    }

    /// 新しい PTY 出力が届いていたか。届いていれば描き直す理由になる。
    pub fn took_output(&self) -> bool {
        self.pty.take_output_notify()
    }

    pub(crate) fn pane(&self, region: Region) -> &Pane {
        match region {
            Region::TerminalShell => &self.shell,
            _ => &self.claude,
        }
    }

    fn pane_mut(&mut self, focus: Focus) -> Option<&mut Pane> {
        match focus {
            Focus::TerminalClaude => Some(&mut self.claude),
            Focus::TerminalShell => Some(&mut self.shell),
            _ => None,
        }
    }

    /// 選択中の worktree にある kind のセッション。PtyStore 全体の添字を添える。
    pub(crate) fn sessions(&self, kind: SessionKind) -> Vec<(usize, &str, &str)> {
        let worktree = self.worktree.as_deref();
        self.pty
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == kind && Some(s.working_dir.as_path()) == worktree)
            .map(|(i, s)| (i, s.id.as_str(), s.label.as_str()))
            .collect()
    }

    fn index_of(&self, session: Option<&String>) -> Option<usize> {
        let id = session?;
        self.pty.sessions().iter().position(|s| s.id == *id)
    }

    /// 選択中の worktree が変わったとき、両パネルの表示をその worktree のものへ移す。
    pub fn follow_worktree(&mut self, worktree: Option<PathBuf>) {
        if self.worktree == worktree {
            return;
        }
        self.worktree = worktree;
        for kind in [SessionKind::ClaudeCode, SessionKind::Shell] {
            let first = self.sessions(kind).first().map(|(_, id, _)| id.to_string());
            match kind {
                SessionKind::Shell => self.shell.show(first),
                _ => self.claude.show(first),
            }
        }
        self.activate_visible();
    }

    /// 見えているセッションの行バッファ上限を前面用へ上げる。
    fn activate_visible(&self) {
        for index in [
            self.index_of(self.claude.session.as_ref()),
            self.index_of(self.shell.session.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            self.pty.activate_session(index);
        }
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        match action {
            Action::LeaveTerminal => return Some(vec![Effect::Focus(Focus::Explorer)]),
            Action::NextSession => self.cycle(ctx.focus, true),
            Action::PrevSession => self.cycle(ctx.focus, false),
            Action::ScrollbackUp | Action::ScrollbackDown | Action::ScrollbackTop => {
                self.scroll(ctx.focus, action)
            }
            Action::SnapToLive => {
                if let Some(pane) = self.pane_mut(ctx.focus) {
                    pane.scroll = 0;
                }
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn cycle(&mut self, focus: Focus, forward: bool) {
        let Some(pane) = self.pane_mut(focus) else {
            return;
        };
        let (kind, current) = (pane.kind, pane.session.clone());
        let ids: Vec<String> = self
            .sessions(kind)
            .iter()
            .map(|(_, id, _)| (*id).to_string())
            .collect();
        if ids.len() <= 1 {
            return;
        }
        let at = current
            .and_then(|id| ids.iter().position(|i| *i == id))
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % ids.len()
        } else {
            (at + ids.len() - 1) % ids.len()
        };
        let target = ids[next].clone();
        if let Some(pane) = self.pane_mut(focus) {
            pane.show(Some(target));
        }
        self.activate_visible();
    }

    fn scroll(&mut self, focus: Focus, action: Action) {
        let Some(pane) = self.pane_mut(focus) else {
            return;
        };
        let page = (pane.size.0 as usize / 2).max(1);
        let want = match action {
            Action::ScrollbackUp => pane.scroll + page,
            Action::ScrollbackDown => pane.scroll.saturating_sub(page),
            _ => usize::MAX,
        };
        let index = self.index_of(self.pane(region_of(focus)).session.as_ref());
        let clamped = match index {
            Some(index) => render::clamp_scrollback(&self.pty, index, want),
            None => 0,
        };
        if let Some(pane) = self.pane_mut(focus) {
            pane.scroll = clamped;
        }
    }

    /// 起動したセッションを、その種類のパネルに映す。
    pub fn spawn(
        &mut self,
        kind: SessionKind,
        resume: Option<&str>,
        worktree: &Path,
        repo_root: &Path,
        config: &Config,
    ) -> Result<()> {
        let name = worktree
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let label = self.next_label(kind, worktree);
        let (rows, cols) = match kind {
            SessionKind::Shell => self.shell.size,
            _ => self.claude.size,
        };
        let launch = match kind {
            SessionKind::Shell => Launch::Shell {
                program: &config.general.shell,
            },
            _ => Launch::ClaudeCode {
                repo_root,
                resume_session_id: resume,
                session_name: None,
            },
        };
        let index = self.pty.spawn(Spawn {
            launch,
            worktree: &name,
            label: &label,
            working_dir: worktree,
            rows,
            cols,
        })?;
        let id = self.pty.sessions()[index].id.clone();
        // spawn した先の worktree を映していないと、そのセッションはどこにも出ない。
        self.worktree = Some(worktree.to_path_buf());
        match kind {
            SessionKind::Shell => self.shell.show(Some(id)),
            _ => self.claude.show(Some(id)),
        }
        self.activate_visible();
        Ok(())
    }

    /// 同じ worktree で空いている一番小さい番号。閉じた番号は空くので詰め直る。
    fn next_label(&self, kind: SessionKind, worktree: &Path) -> String {
        let prefix = match kind {
            SessionKind::Shell => "SH",
            _ => "CC",
        };
        let used: Vec<String> = self
            .pty
            .sessions()
            .iter()
            .filter(|s| s.kind == kind && s.working_dir == worktree)
            .map(|s| s.label.clone())
            .collect();
        (1..=SESSION_SLOTS)
            .map(|n| format!("{prefix}:{n}"))
            .find(|label| !used.contains(label))
            .unwrap_or_else(|| format!("{prefix}:{}", used.len() + 1))
    }

    /// 見えているセッションのスクロールバックを保存する。Claude を優先するのは、
    /// 残したくなるのはほぼそちらだから。
    pub fn save_history(&self) -> Vec<Effect> {
        let visible = self
            .index_of(self.claude.session.as_ref())
            .or_else(|| self.index_of(self.shell.session.as_ref()));
        let Some(index) = visible else {
            return vec![Effect::Status(
                StatusLevel::Warning,
                "no terminal session to save".into(),
            )];
        };
        let session = &self.pty.sessions()[index];
        vec![Effect::Spawn(Task::SaveHistory {
            session_id: session.id.clone(),
            worktree: session.worktree.clone(),
            label: session.label.clone(),
            kind: match session.kind {
                SessionKind::ClaudeCode => "claude_code",
                SessionKind::Shell => "shell",
                SessionKind::Editor => "editor",
            },
            output: self.pty.output(index).join("\n"),
        })]
    }

    /// 見えているシェルがあるか。
    pub fn has_shell(&self) -> bool {
        self.index_of(self.shell.session.as_ref()).is_some()
    }

    /// 見えているシェルへ 1 行流す。スクロールを最新へ戻すのは、送った行が
    /// 遡って読んでいる位置の外で流れると押しても何も起きなく見えるため。
    pub fn send_line(&mut self, line: &str) -> Result<()> {
        let index = self
            .index_of(self.shell.session.as_ref())
            .ok_or_else(|| anyhow::anyhow!("no shell session to run this in"))?;
        self.pty
            .write_to_session(index, format!("{line}\n").as_bytes())?;
        self.shell.scroll = 0;
        Ok(())
    }

    /// フォーカス中のパネルの PTY へキーを流す。
    pub fn forward_key(&self, key: KeyEvent, focus: Focus) {
        let Some(pane) = (match focus {
            Focus::TerminalClaude => Some(&self.claude),
            Focus::TerminalShell => Some(&self.shell),
            _ => None,
        }) else {
            return;
        };
        let Some(index) = self.index_of(pane.session.as_ref()) else {
            return;
        };
        let Some(bytes) = ansi::key_to_bytes(&key, self.pty.application_cursor(index)) else {
            return;
        };
        if let Err(e) = self.pty.write_to_session(index, &bytes) {
            log::warn!("could not write to the PTY session: {e:#}");
        }
    }

    /// 区画の大きさが変わっていたら PTY へ伝える。同じ worktree の同じ種類は
    /// まとめて合わせる — タブを切り替えた先が別の幅のままだと絵が崩れる。
    pub fn sync_sizes(&mut self, layout: &Layout) {
        for region in [Region::TerminalClaude, Region::TerminalShell] {
            let Some(rect) = layout.rect(region) else {
                continue;
            };
            let content = render::content_area(rect);
            let size = (content.height.max(1), content.width.max(1));
            let pane = match region {
                Region::TerminalShell => &mut self.shell,
                _ => &mut self.claude,
            };
            if pane.size == size {
                continue;
            }
            pane.size = size;
            let kind = pane.kind;
            let indices: Vec<usize> = self.sessions(kind).iter().map(|(i, _, _)| *i).collect();
            for index in indices {
                self.pty.resize_session(index, size.0, size.1);
            }
        }
    }

    /// Claude Code のフックとソケットが伝えてくる事実を取り込む。
    pub fn on_watch(&mut self, event: &WatchEvent) -> Vec<Effect> {
        match event {
            WatchEvent::CcState { kind, cwd } => {
                match kind {
                    CcState::Active => {
                        self.active.insert(cwd.clone());
                        self.waiting.remove(cwd);
                    }
                    CcState::Waiting => {
                        self.waiting.insert(cwd.clone());
                        self.active.remove(cwd);
                    }
                }
                Vec::new()
            }
            WatchEvent::CcSessionRotated {
                panel_id,
                session_id,
            } => {
                self.pty.set_claude_session_id(panel_id, session_id.clone());
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// 終わった子プロセスのセッションを片付ける。掃除したら true。
    pub fn cleanup_dead(&mut self) -> bool {
        let mut removed = false;
        for index in (0..self.pty.sessions().len()).rev() {
            if self.pty.is_session_alive(index) {
                continue;
            }
            self.pty.remove_session(index);
            removed = true;
        }
        if removed {
            // id で指しているので添字の詰め直しは要らない。消えたものだけ落とす。
            for kind in [SessionKind::ClaudeCode, SessionKind::Shell] {
                let alive: Vec<String> = self
                    .sessions(kind)
                    .iter()
                    .map(|(_, id, _)| (*id).to_string())
                    .collect();
                let pane = match kind {
                    SessionKind::Shell => &mut self.shell,
                    _ => &mut self.claude,
                };
                if !pane.session.as_ref().is_some_and(|id| alive.contains(id)) {
                    pane.show(alive.first().cloned());
                }
            }
        }
        removed
    }

    /// オルタネート画面へ入った直後のセッションを突く。fzf はこれを待っている。
    pub fn nudge(&mut self) {
        self.pty.nudge_alt_screen_sessions();
    }
}

fn region_of(focus: Focus) -> Region {
    match focus {
        Focus::TerminalShell => Region::TerminalShell,
        _ => Region::TerminalClaude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::time::{Duration, Instant};

    /// 本物のシェルを起動して、出力が画面に載るまで待つ。
    fn spawn_shell(ws: &mut Workspace, script: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let panel = &mut ws.panels.terminal;
        panel.worktree = Some(dir.path().to_path_buf());
        let index = panel
            .pty
            .spawn(Spawn {
                launch: Launch::Shell { program: "/bin/sh" },
                worktree: "t",
                label: "SH:1",
                working_dir: dir.path(),
                rows: 10,
                cols: 40,
            })
            .unwrap();
        let id = panel.pty.sessions()[index].id.clone();
        panel.shell.show(Some(id));
        panel
            .pty
            .write_to_session(index, script.as_bytes())
            .unwrap();
        dir
    }

    fn wait_for(ws: &Workspace, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = render::visible_text(&ws.panels.terminal, Region::TerminalShell);
            if text.contains(needle) || Instant::now() > deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn ptyの出力が画面に載る() {
        let mut ws = Workspace::for_test();
        let _dir = spawn_shell(&mut ws, "echo conductor-hello\n");
        let text = wait_for(&ws, "conductor-hello");
        assert!(text.contains("conductor-hello"), "{text}");
    }

    /// キーが spawn まで届くことを見る。claude の在否に依らないよう shell で確かめる。
    /// 2 本目を作るのに一度ターミナルを出るのは、ctrl+t が fires_in_terminal では
    /// ないから (ターミナルの中では PTY のキーになる)。
    #[test]
    fn ctrl_tでシェルのセッションが増える() {
        let mut ws = Workspace::for_test();
        let mut svc = conductor_svc::Services::new();
        let dir = tempfile::tempdir().unwrap();
        ws.config.general.shell = "/bin/sh".into();
        ws.panels.terminal.worktree = Some(dir.path().to_path_buf());
        ws.repo.root = dir.path().to_path_buf();

        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('t'),
            crossterm::event::KeyModifiers::CONTROL,
        );
        crate::run::on_key(&mut ws, &mut svc, key);
        assert_eq!(ws.panels.terminal.sessions(SessionKind::Shell).len(), 1);
        assert_eq!(ws.focus, Focus::TerminalShell);

        ws.focus = Focus::Explorer;
        crate::run::on_key(&mut ws, &mut svc, key);
        let sessions = ws.panels.terminal.sessions(SessionKind::Shell);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].2, "SH:2", "ラベルの番号が空きを埋めていない");
    }

    /// シェルが無い状態から起こして、実際に出力が返るまで。
    #[test]
    fn テストのコマンドはシェルが無くても起こして流れる() {
        let mut ws = Workspace::for_test();
        let mut svc = conductor_svc::Services::new();
        let dir = tempfile::tempdir().unwrap();
        ws.config.general.shell = "/bin/sh".into();
        ws.repo.root = dir.path().to_path_buf();
        assert!(!ws.panels.terminal.has_shell());

        crate::effect::apply(
            &mut ws,
            &mut svc,
            vec![Effect::SendToShell("echo conductor-ran-a-test".into())],
        );
        assert!(ws.panels.terminal.has_shell(), "無ければ起こす");
        assert_eq!(ws.focus, Focus::TerminalShell);
        let text = wait_for(&ws, "conductor-ran-a-test");
        assert!(text.contains("conductor-ran-a-test"), "{text}");
    }

    #[test]
    fn 死んだセッションは片付いてパネルの表示が残りへ移る() {
        let mut ws = Workspace::for_test();
        let _dir = spawn_shell(&mut ws, "exit\n");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ws.panels.terminal.cleanup_dead() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ws.panels.terminal.pty.sessions().is_empty());
        assert!(ws.panels.terminal.shell.session.is_none());
    }
}
