//! Claude Code とシェルの PTY パネル。
//!
//! PTY だけは svc の Event 経路に乗らない (バイト列が描画より高頻度で届く) ので、
//! [PtyStore] はこのパネルが直接持ち、描画のたびに画面をロックして読む。

pub mod ansi;
pub(crate) mod reflow;
pub mod render;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conductor_core::claude_log::LogEntry;
use conductor_core::config::Config;
use conductor_core::keymap::Action;
use conductor_core::theme::Theme;
use conductor_svc::pty::{Launch, PtyStore, SessionKind, Spawn};
use conductor_svc::watch::{CcState, WatchEvent};
use crossterm::event::KeyEvent;

use crate::click::ClickTracker;
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::panels::viewer::syntax::Highlighter;
use crate::task::Task;
use crate::workspace::{Ctx, Focus, StatusLevel};
use ratatui::layout::Rect;

use reflow::{Handled, Reflow};

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
    clicks: ClickTracker,
}

impl Pane {
    fn new(kind: SessionKind, size: (u16, u16)) -> Self {
        Self {
            kind,
            session: None,
            size,
            scroll: 0,
            clicks: ClickTracker::default(),
        }
    }

    /// 映すセッションを変える。スクロール位置は今のセッションのものなので落とす。
    fn show(&mut self, session: Option<String>) {
        self.session = session;
        self.scroll = 0;
    }
}

/// 1 ファイルに対して起こした $EDITOR。プロセスが終われば畳む。
struct Editor {
    session: String,
    /// 編集中の絶対パス。閉じたときに読み直す先。
    path: PathBuf,
    size: (u16, u16),
}

pub struct TerminalPanel {
    pty: PtyStore,
    claude: Pane,
    shell: Pane,
    /// Claude 区画に重ねているトランスクリプト。
    transcript: Option<Reflow>,
    editor: Option<Editor>,
    wants_clear: bool,
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
            transcript: None,
            editor: None,
            wants_clear: false,
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
        // 別の worktree のセッションを映したままトランスクリプトを残すと、
        // 他人のログを見せることになる。
        self.transcript = None;
        self.discard_editor();
        self.wants_clear = true;
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
        if self.reading(ctx.focus) {
            return self.transcript_action(action);
        }
        match action {
            Action::LeaveTerminal => return Some(vec![Effect::Focus(Focus::Explorer)]),
            Action::NextSession => self.cycle(ctx.focus, true),
            Action::PrevSession => self.cycle(ctx.focus, false),
            // ライブ表示の一番上でさらに上へ押したら、vt100 の行数で頭打ちになる
            // スクロールバックではなく .jsonl そのものを読むビューへ入る。
            Action::ScrollbackUp | Action::ScrollbackTop
                if ctx.focus == Focus::TerminalClaude && self.claude.scroll == 0 =>
            {
                let (opened, effects) = self.open_transcript();
                if !opened {
                    self.scroll(ctx.focus, action);
                }
                return Some(effects);
            }
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

    /// Claude 区画がトランスクリプトを映しているか。
    pub fn reading(&self, focus: Focus) -> bool {
        focus == Focus::TerminalClaude && self.transcript.is_some()
    }

    /// 読んでいる間の Action。持たないものは外の解釈へ落とす (パレット、区画のリサイズ)。
    fn transcript_action(&mut self, action: Action) -> Option<Vec<Effect>> {
        match action {
            Action::SnapToLive => return Some(self.close_transcript()),
            Action::LeaveTerminal => {
                let mut effects = self.close_transcript();
                effects.push(Effect::Focus(Focus::Explorer));
                return Some(effects);
            }
            _ => {}
        }
        let reflow = self.transcript.as_mut()?;
        let page = reflow.page() as isize;
        match action {
            Action::ScrollbackUp => reflow.scroll_by(-page),
            Action::ScrollbackDown => reflow.scroll_by(page),
            Action::ScrollbackTop => reflow.scroll_to_top(),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// 開けたかどうかも返す — 開けなければ呼び出し側は通常のスクロールバックに落ちる。
    fn open_transcript(&mut self) -> (bool, Vec<Effect>) {
        let unavailable = |message: &str| {
            (
                false,
                vec![Effect::Status(StatusLevel::Warning, message.to_string())],
            )
        };
        let Some(index) = self.index_of(self.claude.session.as_ref()) else {
            return unavailable("no Claude Code session in this panel");
        };
        let Some((working_dir, session_id, _)) = self.pty.claude_session_ref(index) else {
            return unavailable("this panel has not reported its session id yet");
        };
        self.transcript = Some(Reflow::opening(session_id.clone()));
        (
            true,
            vec![Effect::Spawn(Task::ReadTranscript {
                working_dir,
                session_id,
            })],
        )
    }

    /// 読み終えたログを載せる。空や読めないログではビューを畳んで理由を出す。
    pub fn install_transcript(
        &mut self,
        session_id: &str,
        entries: Result<Vec<LogEntry>, String>,
    ) -> Vec<Effect> {
        if self
            .transcript
            .as_ref()
            .is_none_or(|r| r.session() != session_id)
        {
            return Vec::new();
        }
        match entries {
            Ok(entries) if entries.is_empty() => {
                let mut effects = self.close_transcript();
                effects.push(Effect::Status(
                    StatusLevel::Info,
                    "the session log has nothing to show yet".into(),
                ));
                effects
            }
            Ok(entries) => {
                if let Some(reflow) = self.transcript.as_mut() {
                    reflow.install(entries);
                }
                Vec::new()
            }
            Err(e) => {
                let mut effects = self.close_transcript();
                effects.push(Effect::Status(StatusLevel::Warning, e));
                effects
            }
        }
    }

    /// ライブ PTY へ戻す。
    fn close_transcript(&mut self) -> Vec<Effect> {
        self.transcript = None;
        self.claude.scroll = 0;
        self.wants_clear = true;
        Vec::new()
    }

    pub fn transcript_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(reflow) = self.transcript.as_mut() else {
            return Vec::new();
        };
        match reflow.key(key) {
            Handled::Consumed => Vec::new(),
            Handled::Close => self.close_transcript(),
        }
    }

    pub fn transcript_scroll(&mut self, delta: isize) {
        if let Some(reflow) = self.transcript.as_mut() {
            reflow.scroll_by(delta);
        }
    }

    /// チップを踏んでいたら最新へ飛ぶ。踏んでいなければ何もしない。
    pub fn transcript_click(&mut self, panel: Rect, x: u16, y: u16) -> bool {
        let area = render::content_area(panel);
        let Some(reflow) = self.transcript.as_mut() else {
            return false;
        };
        let Some(rect) = reflow.badge_rect(area) else {
            return false;
        };
        let hit = x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
        if hit {
            reflow.jump_to_latest();
        }
        hit
    }

    /// 空の区画のクリック。2 回目で新しいセッションを起こす。1 回で起こすと、
    /// 区画へフォーカスを移すだけのつもりのクリックがそのままプロセスを増やす。
    pub fn click(&mut self, focus: Focus) -> Vec<Effect> {
        let Some(pane) = self.pane_mut(focus) else {
            return Vec::new();
        };
        if pane.session.is_some() || !pane.clicks.is_double(0) {
            return Vec::new();
        }
        vec![Effect::NewSession(pane.kind)]
    }

    /// 描く前に行を組み直す。トランスクリプトを開いていなければ何もしない。
    pub fn prepare(&mut self, theme: &Theme, highlighter: &Highlighter, overlay_open: bool) {
        let size = self.claude.size;
        if let Some(reflow) = self.transcript.as_mut() {
            reflow.prepare(theme, highlighter, size, overlay_open);
            self.wants_clear |= reflow.take_clear_request();
        }
    }

    /// エディタを PTY で起こす。返すのは区画の見出しに出すファイル名。
    /// どのエディタかは環境が決めるので、argv は呼び出し側から受け取る。
    pub fn open_editor(&mut self, path: &Path, worktree: &Path, argv: &[String]) -> Result<String> {
        anyhow::ensure!(self.editor.is_none(), "an editor is already open");
        let (program, args) = argv.split_first().context("the editor command is empty")?;
        let (rows, cols) = DEFAULT_PTY_SIZE;
        let worktree_name = worktree
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let index = self.pty.spawn(Spawn {
            launch: Launch::Editor {
                program,
                args,
                file: path,
            },
            worktree: &worktree_name,
            label: "ED",
            working_dir: worktree,
            rows,
            cols,
        })?;
        self.pty.activate_session(index);
        self.editor = Some(Editor {
            session: self.pty.sessions()[index].id.clone(),
            path: path.to_path_buf(),
            size: (rows, cols),
        });
        // 置き換える区画の上にエディタの代替画面を一から描かせる。
        self.wants_clear = true;
        Ok(self.editor_name().unwrap_or_default())
    }

    /// エディタの PTY を落として畳む。読み直しは呼び出し側がどのみち行う。
    fn discard_editor(&mut self) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        if let Some(index) = self.index_of(Some(&editor.session)) {
            let _ = self.pty.kill_session(index);
            self.pty.remove_session(index);
        }
    }

    /// エディタが終わっていれば畳んで、編集していたパスを返す。毎フレーム呼ぶ —
    /// 停止セッションの掃除タイマーを待つと、閉じた後も区画が残って見える。
    pub fn poll_editor_exit(&mut self) -> Option<PathBuf> {
        let session = self.editor.as_ref()?.session.clone();
        match self.index_of(Some(&session)) {
            Some(index) if self.pty.is_session_alive(index) => return None,
            Some(index) => self.pty.remove_session(index),
            None => {}
        }
        self.wants_clear = true;
        self.editor.take().map(|editor| editor.path)
    }

    /// 開いているファイルの名前。区画の見出しが読む。
    pub fn editor_name(&self) -> Option<String> {
        let path = &self.editor.as_ref()?.path;
        Some(path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ))
    }

    pub fn take_clear_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_clear)
    }

    pub(crate) fn transcript(&self) -> Option<&Reflow> {
        self.transcript.as_ref()
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

    /// フォーカス中の区画の PTY へキーを流す。
    pub fn forward_key(&self, key: KeyEvent, focus: Focus) {
        let session = match focus {
            Focus::TerminalClaude => self.claude.session.as_ref(),
            Focus::TerminalShell => self.shell.session.as_ref(),
            Focus::Editor => self.editor.as_ref().map(|e| &e.session),
            _ => return,
        };
        let Some(index) = self.index_of(session) else {
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
        if let (Some(rect), Some(editor)) = (layout.rect(Region::Editor), self.editor.as_mut()) {
            let content = render::editor_area(rect);
            let size = (content.height.max(1), content.width.max(1));
            if editor.size != size {
                editor.size = size;
                let session = editor.session.clone();
                if let Some(index) = self.index_of(Some(&session)) {
                    self.pty.resize_session(index, size.0, size.1);
                }
            }
        }
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

/// $EDITOR が見つからないときに使う。POSIX がどの環境にも置いている。
const EDITOR_FALLBACK: &str = "vi";

/// レイアウトが決まる前の暫定。次のフレームの [TerminalPanel::sync_sizes] が直す。
const DEFAULT_PTY_SIZE: (u16, u16) = (24, 80);

/// 環境が指すエディタ。
pub(crate) fn editor_argv() -> Vec<String> {
    editor_command(
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
        EDITOR_FALLBACK,
    )
}

/// $VISUAL → $EDITOR → 既定 の順。空白だけの値は空のコマンドを生まないよう飛ばす。
/// 分割は素朴で、シェル風のクォート解釈はしない (意図的な制限)。
fn editor_command(visual: Option<&str>, editor: Option<&str>, fallback: &str) -> Vec<String> {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(fallback);
    let parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        vec![fallback.to_string()]
    } else {
        parts
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
    use ratatui::layout::Rect;
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

    /// $VISUAL > $EDITOR > 既定。空白だけの値は飛ばし、分割は空白での素朴なもの。
    #[test]
    fn エディタのコマンドは優先順で選び素朴に分割する() {
        let cases: [(Option<&str>, Option<&str>, &[&str]); 10] = [
            (None, None, &["vi"]),
            (Some("vim"), Some("nano"), &["vim"]),
            (None, Some("nano"), &["nano"]),
            (Some("code -w"), None, &["code", "-w"]),
            (Some("code\t-w  -n"), None, &["code", "-w", "-n"]),
            (Some(""), None, &["vi"]),
            (Some("   "), None, &["vi"]),
            (Some(""), Some("nano"), &["nano"]),
            (Some("  vim  "), None, &["vim"]),
            (
                Some("vim -c 'set ft=rust'"),
                None,
                &["vim", "-c", "'set", "ft=rust'"],
            ),
        ];
        for (visual, editor, want) in cases {
            assert_eq!(
                editor_command(visual, editor, EDITOR_FALLBACK),
                want,
                "visual={visual:?} editor={editor:?}"
            );
        }
    }

    /// エディタはタブ行を持たないので、枠の 2 行 2 桁だけを引く。
    #[test]
    fn エディタの内容領域は枠だけを引く() {
        assert_eq!(
            render::editor_area(Rect::new(0, 0, 80, 40)),
            Rect::new(1, 1, 78, 38)
        );
        // 極小の区画でもアンダーフローしない。vt100 は 1 以上が要る。
        for w in 1..=3u16 {
            for h in 1..=3u16 {
                let area = render::editor_area(Rect::new(0, 0, w, h));
                assert!(area.width.max(1) >= 1 && area.height.max(1) >= 1, "{w}x{h}");
            }
        }
    }

    /// 読む先は pin した session id だけ。id が無ければディレクトリの中から選び直さない。
    #[test]
    fn セッションidが無ければトランスクリプトを開かない() {
        let mut ws = Workspace::for_test();
        let _dir = spawn_shell(&mut ws, "");
        let panel = &mut ws.panels.terminal;
        let (opened, effects) = panel.open_transcript();
        assert!(!opened && panel.transcript.is_none());
        assert!(matches!(
            effects[0],
            Effect::Status(StatusLevel::Warning, _)
        ));
    }

    /// /clear のローテーション中に届いた、開いているのとは別のセッションの結果は捨てる。
    #[test]
    fn 別のセッションの結果は捨てる() {
        let mut panel = TerminalPanel::new(&Config::default());
        panel.transcript = Some(Reflow::opening("session-a".into()));
        let entries = vec![LogEntry {
            role: conductor_core::claude_log::Role::User,
            blocks: vec![conductor_core::claude_log::DisplayBlock::Text(
                "other".into(),
            )],
        }];

        assert!(
            panel
                .install_transcript("session-b", Ok(entries))
                .is_empty()
        );
        let reflow = panel.transcript.as_ref().expect("ビューは開いたまま");
        assert!(reflow.is_loading(), "他人のログを載せてはいけない");
    }

    #[test]
    fn 空の区画は2回目のクリックでセッションを起こす() {
        let mut panel = TerminalPanel::new(&Config::default());
        assert!(panel.click(Focus::TerminalClaude).is_empty(), "1 回目");
        assert!(matches!(
            panel.click(Focus::TerminalClaude).as_slice(),
            [Effect::NewSession(SessionKind::ClaudeCode)]
        ));
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
