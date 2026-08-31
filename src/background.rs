//! mpsc チャネルを使うバックグラウンド処理の汎用ラッパー。
//!
//! コードベース各所にあった場当たり的な Option<mpsc::Receiver<T>> を、
//! 統一された BackgroundOp<T> に置き換える。

use std::sync::mpsc;

/// mpsc チャネル経由で T 型の結果を生むバックグラウンド処理。
pub struct BackgroundOp<T> {
    rx: Option<mpsc::Receiver<T>>,
}

impl<T> Default for BackgroundOp<T> {
    fn default() -> Self {
        Self { rx: None }
    }
}

impl<T: Send + 'static> BackgroundOp<T> {
    /// バックグラウンド処理を開始する。
    ///
    /// 送信側を渡して f を実行するスレッドを立てる。呼び出し側はその後
    /// poll() か poll_all() で結果を取り出す。
    pub fn start<F>(&mut self, f: F)
    where
        F: FnOnce(mpsc::Sender<T>) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || f(tx));
    }
}

impl<T> BackgroundOp<T> {
    /// 結果を 1 件受け取ろうとする。結果が無いかチャネルが閉じている場合は
    /// None を返す。
    pub fn poll(&mut self) -> Option<T> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                None
            }
        }
    }

    pub fn poll_all(&mut self) -> Vec<T> {
        let mut results = Vec::new();
        let Some(ref rx) = self.rx else {
            return results;
        };
        loop {
            match rx.try_recv() {
                Ok(v) => results.push(v),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            }
        }
        results
    }

    /// バックグラウンド処理が動いているか (受信側を持っているか)。
    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// 受信側を捨てる。残りの結果は実質的にキャンセル・無視される。
    pub fn clear(&mut self) {
        self.rx = None;
    }
}

/// スレッドの終了を待つ。ただし期限を切って、超えたら見捨てる。
///
/// 終了時の join が外部リソース待ちのスレッドで固まると、端末を戻す前に
/// プロセスごと止まってユーザーが Ctrl+Q で抜けられなくなる。後始末より
/// 抜けられることを優先する。
pub fn join_or_abandon(thread: std::thread::JoinHandle<()>, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !thread.is_finished() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    thread.join().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_single() {
        let mut op: BackgroundOp<i32> = BackgroundOp::default();
        assert!(!op.is_running());
        assert!(op.poll().is_none());

        op.start(|tx| {
            tx.send(42).unwrap();
        });
        assert!(op.is_running());

        // スレッドが送信するのを少し待つ
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(op.poll(), Some(42));
    }

    #[test]
    fn test_poll_all() {
        let mut op: BackgroundOp<i32> = BackgroundOp::default();
        op.start(|tx| {
            for i in 0..5 {
                tx.send(i).unwrap();
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(10));
        let results = op.poll_all();
        assert_eq!(results, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_clear() {
        let mut op: BackgroundOp<i32> = BackgroundOp::default();
        op.start(|tx| {
            tx.send(1).unwrap();
        });
        assert!(op.is_running());
        op.clear();
        assert!(!op.is_running());
    }

    #[test]
    fn join_or_abandon_gives_up_on_a_wedged_thread() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let wedged = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        let start = std::time::Instant::now();
        assert!(!super::join_or_abandon(
            wedged,
            std::time::Duration::from_millis(100)
        ));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        drop(tx);
    }

    #[test]
    fn join_or_abandon_waits_for_a_thread_that_finishes() {
        let t = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
        });
        assert!(super::join_or_abandon(t, std::time::Duration::from_secs(5)));
    }
}
