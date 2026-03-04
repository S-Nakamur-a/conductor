//! Generic wrapper for background operations using mpsc channels.
//!
//! Replaces ad-hoc `Option<mpsc::Receiver<T>>` patterns throughout the
//! codebase with a unified `BackgroundOp<T>`.

use std::sync::mpsc;

/// A background operation that produces results of type `T` via an mpsc channel.
pub struct BackgroundOp<T> {
    rx: Option<mpsc::Receiver<T>>,
}

impl<T> Default for BackgroundOp<T> {
    fn default() -> Self {
        Self { rx: None }
    }
}

impl<T: Send + 'static> BackgroundOp<T> {
    /// Start a new background operation.
    ///
    /// Spawns a thread that executes `f` with a sender. The caller can then
    /// use `poll()` or `poll_all()` to retrieve results.
    pub fn start<F>(&mut self, f: F)
    where
        F: FnOnce(mpsc::Sender<T>) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || f(tx));
    }

    /// Start with an externally created receiver (for cases where the sender
    /// is passed to a library function).
    #[allow(dead_code)]
    pub fn start_with_rx(&mut self, rx: mpsc::Receiver<T>) {
        self.rx = Some(rx);
    }
}

impl<T> BackgroundOp<T> {
    /// Try to receive a single result. Returns `None` if no result is
    /// available or the channel is closed.
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

    /// Drain all available results from the channel.
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

    /// Whether a background operation is active (has a receiver).
    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Drop the receiver, effectively cancelling/ignoring remaining results.
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

        // Give the thread a moment to send
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
