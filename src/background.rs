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

    /// 外部で作った受信側を使って開始する (送信側をライブラリの関数へ渡す場合)。
    #[allow(dead_code)]
    pub fn start_with_rx(&mut self, rx: mpsc::Receiver<T>) {
        self.rx = Some(rx);
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

    /// チャネルに溜まっている結果をすべて取り出す。
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
}
