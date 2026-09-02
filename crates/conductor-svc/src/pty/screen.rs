//! vt100 画面の読み出し、リサイズとリフロー、オルタネート画面のナッジ、入力待ちの検出。

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;

use super::{PtyStore, SessionKind, lock};

impl PtyStore {
    /// idx のセッションの vt100 パーサ。UI が描画のあいだロックできるよう Arc を返す。
    pub fn screen(&self, idx: usize) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.sessions.get(idx).map(|s| Arc::clone(&s.io.screen))
    }

    /// idx のセッションの行バッファのスナップショット。
    pub fn output(&self, idx: usize) -> Vec<String> {
        self.sessions
            .get(idx)
            .map(|s| lock(&s.io.lines).clone())
            .unwrap_or_default()
    }

    /// 画面に空白以外が 1 文字でもあるか。
    pub fn has_visible_output(&self, idx: usize) -> bool {
        self.sessions.get(idx).is_some_and(|s| {
            let parser = lock(&s.io.screen);
            let screen = parser.screen();
            (0..screen.size().0).any(|row| !row_text(screen, row).trim().is_empty())
        })
    }

    /// idx でアプリケーションカーソルキーモード (DECCKM) が有効か。
    ///
    /// オルタネート画面のページャやエディタはこれを有効にし、以降は矢印キーを SS3
    /// (ESC O A) として期待して既定の CSI 形式を無視する。キー転送はこれを見る。
    pub fn application_cursor(&self, idx: usize) -> bool {
        self.sessions
            .get(idx)
            .is_some_and(|s| lock(&s.io.screen).screen().application_cursor())
    }

    /// PTY と vt100 パーサの両方をリサイズする。生バイトの再生でパーサを組み直した
    /// (新しい幅でリフローした) 場合に true。
    ///
    /// vt100 の set_size はリフローしない — 列が変わるとラップフラグを消して行を
    /// 切り詰め/パディングするだけなので、以前ラップされていた行は旧幅のまま残る。
    /// 生履歴を持つセッションは記録から組み直して再ラップさせ、それ以外は set_size に落ちる。
    pub fn resize_session(&mut self, idx: usize, rows: u16, cols: u16) -> bool {
        // vt100::Parser::new は非ゼロの寸法を要求する。
        let rows = rows.max(1);
        let cols = cols.max(1);
        let scrollback = self.inactive_scrollback;
        let Some(session) = self.sessions.get(idx) else {
            return false;
        };

        // 実 PTY のリサイズは SIGWINCH を配送し、子がライブ領域を描き直す。
        let _ = session.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });

        let mut parser = lock(&session.io.screen);
        let width_changed = parser.screen().size().1 != cols;
        let Some(history) = session.io.raw_history.as_ref().filter(|_| width_changed) else {
            parser.set_size(rows, cols);
            return false;
        };

        // screen のロックを持ったまま履歴を読む。同じロックの下で追記している reader
        // スレッドとの整合を保つため。
        *parser = rebuild_parser(&lock(history), rows, cols, scrollback);
        true
    }

    /// 最近オルタネート画面に入ったセッションへ、SIGWINCH のナッジを繰り返し送る。
    ///
    /// fzf のようなプログラムはリサイズシグナルを受けるまで初期 UI を描かず、1 回だけの
    /// ナッジでは相手の準備前に届いてしまうことがある。macOS の PTY バッファリングの癖を
    /// 避けるため、遷移から 500ms のあいだ 100ms ごとに送る。
    pub fn nudge_alt_screen_sessions(&mut self) {
        const NUDGE_WINDOW: Duration = Duration::from_millis(500);
        const NUDGE_INTERVAL: Duration = Duration::from_millis(100);

        for session in &mut self.sessions {
            if session.io.alt_screen_entered.swap(false, Ordering::Relaxed) {
                session.nudge_until = Some(Instant::now() + NUDGE_WINDOW);
                session.last_nudge = None;
            }
            let Some(until) = session.nudge_until else {
                continue;
            };
            if Instant::now() > until {
                session.nudge_until = None;
                continue;
            }
            if session
                .last_nudge
                .is_some_and(|t| t.elapsed() < NUDGE_INTERVAL)
            {
                continue;
            }

            session.last_nudge = Some(Instant::now());
            let (rows, cols) = lock(&session.io.screen).screen().size();
            // macOS はサイズが実際に変わったときにしか SIGWINCH を配送しないので、
            // 一瞬だけ 1 行縮めてから戻す。
            if rows > 1 {
                let _ = session.master.resize(PtySize {
                    rows: rows - 1,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            let _ = session.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    /// idx の Claude Code セッションがユーザー入力待ちに見えるか。
    ///
    /// 出力が 1.5 秒止まっていることと、カーソル行がプロンプトの見た目であることの両方。
    /// Claude Code はアイドルを名乗らないので、画面から読むしかない。
    pub fn is_waiting_for_input(&self, idx: usize) -> bool {
        const IDLE_THRESHOLD: Duration = Duration::from_millis(1500);

        let Some(session) = self.sessions.get(idx) else {
            return false;
        };
        if session.kind != SessionKind::ClaudeCode {
            return false;
        }
        if session.last_output().elapsed() < IDLE_THRESHOLD {
            return false;
        }

        let parser = lock(&session.io.screen);
        let screen = parser.screen();
        let cursor_row = screen.cursor_position().0;
        let line = row_text(screen, cursor_row);
        let line = line.trim();

        // 標準入力のプロンプト、またはツール許可のプロンプト。
        line.starts_with("> ") || line == ">" || line.contains("[Y/n]") || line.contains("[y/N]")
    }
}

fn row_text(screen: &vt100::Screen, row: u16) -> String {
    let cols = screen.size().1;
    let mut text = String::with_capacity(cols as usize);
    for col in 0..cols {
        match screen.cell(row, col) {
            Some(cell) => text.push_str(&cell.contents()),
            None => text.push(' '),
        }
    }
    text
}

/// 記録済みの生バイトを再生して、指定サイズのパーサを組み直す。
/// resize_session のリフロー経路の中核を、実 PTY 無しで試せるよう切り出したもの。
pub(super) fn rebuild_parser(
    history: &VecDeque<u8>,
    rows: u16,
    cols: u16,
    scrollback: usize,
) -> vt100::Parser {
    let mut parser = vt100::Parser::new(rows, cols, scrollback);
    let (front, back) = history.as_slices();
    parser.process(front);
    parser.process(back);
    parser
}
