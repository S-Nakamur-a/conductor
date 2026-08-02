//! vt100 画面へのアクセス、リサイズ/リフロー、オルタネート画面のナッジ、
//! Claude Code の入力待ち検出。

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;

use super::{PtyManager, SessionKind};

impl PtyManager {
    /// idx のセッションが何か表示可能な出力を生成したか(vt100 画面が完全に
    /// 空白ではないか)を確認する。
    pub fn session_has_visible_output(&self, idx: usize) -> bool {
        self.sessions.get(idx).is_some_and(|s| {
            let parser = s.screen.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            let cols = screen.size().1;
            for row in 0..screen.size().0 {
                let row_text = Self::extract_row_text(screen, row, cols);
                if !row_text.trim().is_empty() {
                    return true;
                }
            }
            false
        })
    }

    /// 指定インデックスのセッションの出力バッファのスナップショットを取得する。
    pub fn get_output(&self, idx: usize) -> Vec<String> {
        self.sessions
            .get(idx)
            .map(|s| {
                let buf = s.output_buffer.lock().unwrap_or_else(|e| e.into_inner());
                buf.clone()
            })
            .unwrap_or_default()
    }

    /// idx のセッションでアプリケーションカーソルキーモード(DECCKM)が
    /// 有効かどうか。オルタネート画面上のフルスクリーンプログラム —
    /// ページャ(less、bat)、エディタ(vim)— は通常これを有効にし、
    /// 以降は矢印キーを SS3 (ESC O A) として期待し、デフォルトの CSI
    /// (ESC [ A) 形式を無視する。キー転送はこれを見て、矢印キーが実際に
    /// 効くようにしている。
    pub fn session_application_cursor(&self, idx: usize) -> bool {
        self.sessions.get(idx).is_some_and(|s| {
            let parser = s.screen.lock().unwrap_or_else(|e| e.into_inner());
            parser.screen().application_cursor()
        })
    }

    /// 指定インデックスのセッションの vt100 画面パーサを取得する。
    ///
    /// UI が描画のためにロックできるよう Arc のクローンを返す。
    pub fn get_screen(&self, idx: usize) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.sessions.get(idx).map(|s| Arc::clone(&s.screen))
    }

    /// idx のセッションについて、実際の PTY と vt100 パーサの両方をリサイズする。
    ///
    /// vt100 パーサが生バイト履歴の再生によって再構築された(新しい幅で
    /// 内容がリフローされた)場合に true を返す。行数のみの変更や、
    /// 生バイト履歴を記録していないセッションでは false を返す。
    ///
    /// vt100 の set_size はリフローしない: 列数の変更時、各行のラップ
    /// フラグをクリアし、その場で行を切り詰め/パディングするだけなので、
    /// 以前ラップされていた行は旧幅のままラップされ続ける。旧い
    /// (自動折り返しされた)内容を新しい幅に追従させるため、記録済みの
    /// 生バイトストリームからパーサを再構築し、再パース時に再ラップさせる。
    /// この経路を通るのは raw_history を持つセッション(シェル — フィールド
    /// のドキュメントを参照)だけであり、それ以外は set_size へフォール
    /// バックする。これは、Claude Code のようなその場描画型アプリに対して
    /// 実際の端末が行うのとまったく同じ挙動である(PTY リサイズが配送する
    /// SIGWINCH を受けて、自分の現在のフレームを再描画する)。
    pub fn resize_session(&mut self, idx: usize, rows: u16, cols: u16) -> bool {
        // vt100::Parser::new は非ゼロの寸法を要求するため、呼び出し側の
        // 規律に関わらず頑健であるよう防御的にクランプする。
        let rows = rows.max(1);
        let cols = cols.max(1);
        let scrollback = self.inactive_scrollback;
        let Some(session) = self.sessions.get(idx) else {
            return false;
        };

        // 実際の PTY をリサイズする(SIGWINCH を配送し、子プロセスがライブ
        // 領域を再描画する)。
        let _ = session.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });

        let mut parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
        let old_cols = parser.screen().size().1;

        // リフローが適用されるのは幅が変わった場合のみで、かつ生バイト
        // 履歴を記録しているセッションに限る。行数のみの変更、または記録を
        // オプトアウトしているセッション(Claude、editor)は set_size で
        // その場処理する。
        let reflow = old_cols != cols && session.raw_history.is_some();
        if !reflow {
            parser.set_size(rows, cols);
            return false;
        }

        // 幅が変わった — 生バイト履歴を再生してパーサを新しい幅で再構築する。
        // screen ロックを保持し続けることで、raw_history に追記しつつ
        // 同じロック下でパーサへ処理を行っている reader スレッドとの整合を保つ。
        let history = session
            .raw_history
            .as_ref()
            .expect("reflow implies raw_history is Some")
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *parser = Self::rebuild_parser(&history, rows, cols, scrollback);
        true
    }

    /// 最近オルタネート画面モードに入ったセッションへ、定期的に SIGWINCH
    /// ナッジを送る。fzf のようなプログラムは、リサイズシグナルを受け取る
    /// まで初期 UI を描画しないことがあり、1回だけのナッジではプログラムの
    /// 準備が整う前に届いてしまうことがある。このメソッドは遷移後 500 ms の
    /// 間、約100msごとにナッジを送り、macOS の PTY バッファリングの癖を
    /// 回避する。
    pub fn nudge_alt_screen_sessions(&mut self) {
        const NUDGE_WINDOW: Duration = Duration::from_millis(500);
        const NUDGE_INTERVAL: Duration = Duration::from_millis(100);

        for session in &mut self.sessions {
            // reader スレッドが新しいオルタネート画面への突入を検出したか確認する。
            if session.alt_screen_entered.swap(false, Ordering::Relaxed) {
                session.alt_screen_nudge_until = Some(Instant::now() + NUDGE_WINDOW);
                session.last_nudge_time = None;
            }

            // ウィンドウ内である間、定期的にナッジを送る。
            let Some(until) = session.alt_screen_nudge_until else {
                continue;
            };
            if Instant::now() > until {
                session.alt_screen_nudge_until = None;
                continue;
            }

            let should_nudge = match session.last_nudge_time {
                None => true,
                Some(t) => t.elapsed() >= NUDGE_INTERVAL,
            };
            if should_nudge {
                session.last_nudge_time = Some(Instant::now());
                let (rows, cols) = {
                    let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
                    parser.screen().size()
                };
                // macOS はサイズが実際に変わったときにしか SIGWINCH を配送
                // しないため、一瞬だけ1行縮めてから本来のサイズへ戻す。
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
    }

    // 入力待ちの検出

    /// idx の Claude Code セッションがユーザー入力待ち(アイドルプロンプト
    /// またはツール許可プロンプト)に見えるかを確認する。
    ///
    /// 次の**両方**の条件を満たしたときに true を返す。
    /// 1. 少なくとも 1.5 秒間 PTY 出力を受け取っていない。
    /// 2. vt100 画面のカーソル行が既知のプロンプトパターンに一致する。
    pub fn is_waiting_for_input(&self, idx: usize) -> bool {
        let session = match self.sessions.get(idx) {
            Some(s) => s,
            None => return false,
        };

        // Claude Code セッションにのみ適用する。
        if session.kind != SessionKind::ClaudeCode {
            return false;
        }

        // 条件1: 出力が少なくとも 1.5 秒間安定している必要がある。
        const IDLE_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(1500);
        {
            let t = session
                .last_output_time
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if t.elapsed() < IDLE_THRESHOLD {
                return false;
            }
        }

        // 条件2: カーソル行がプロンプトパターンに一致する。
        let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let cursor_row = screen.cursor_position().0;
        let cols = screen.size().1;
        let row_text = Self::extract_row_text(screen, cursor_row, cols);
        let trimmed = row_text.trim();

        // マッチ: "> " プロンプト(Claude Code の標準入力)
        if trimmed.starts_with("> ") || trimmed == ">" {
            return true;
        }

        // マッチ: [Y/n] または [y/N] を含むツール許可プロンプト
        if trimmed.contains("[Y/n]") || trimmed.contains("[y/N]") {
            return true;
        }

        false
    }

    /// vt100 画面から1行分のテキスト内容を抽出する。
    fn extract_row_text(screen: &vt100::Screen, row: u16, cols: u16) -> String {
        let mut text = String::with_capacity(cols as usize);
        for col in 0..cols {
            let cell = screen.cell(row, col);
            if let Some(cell) = cell {
                text.push_str(&cell.contents());
            } else {
                text.push(' ');
            }
        }
        text
    }

    /// 記録済みの生バイト履歴を再生することで、指定サイズの新しい vt100
    /// パーサを組み立て、内容を新しい幅で再ラップする。これは
    /// resize_session のリフロー経路の中核部分を、実際の PTY を起動せずに
    /// 単体テストできるよう切り出したもの。
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
}
