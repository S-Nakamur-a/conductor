//! conductor の副作用の実行者。
//!
//! UI は `Services::spawn` に閉包を渡して RequestId を受け取り、あとは `try_recv` で
//! 届いた Event を消費するだけ。結果の語彙 `P` は UI が定義する。ここは実行の仕組み
//! (スレッド、世代、watcher) だけを持ち、何を運ぶかは知らない。

pub mod pty;
pub mod watch;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// worktree 切替のたびに進む番号。古い世代の結果は `try_recv` が捨てる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Generation(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

#[derive(Debug)]
pub struct Event<P> {
    pub generation: Generation,
    pub req: Option<RequestId>,
    pub kind: EventKind<P>,
}

#[derive(Debug)]
pub enum EventKind<P> {
    Task(P),
    Watch(watch::WatchEvent),
}

pub struct Services<P> {
    tx: Sender<Event<P>>,
    rx: Receiver<Event<P>>,
    /// 送信口と共有する。EventSender が値を写し取ると、世代を進めた瞬間に
    /// watcher の合図が永久に捨てられる。
    generation: Arc<AtomicU64>,
    next_req: u64,
}

impl<P: Send + 'static> Default for Services<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Send + 'static> Services<P> {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            generation: Arc::new(AtomicU64::new(0)),
            next_req: 0,
        }
    }

    pub fn generation(&self) -> Generation {
        Generation(self.generation.load(Ordering::Relaxed))
    }

    /// 世代を進める。進める前に投げた Task の結果は届いても捨てられる。
    pub fn bump_generation(&mut self) -> Generation {
        Generation(self.generation.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// 閉包をワーカースレッドで走らせ、結果を今の世代の Event として届ける。
    pub fn spawn<T, F>(&mut self, work: F, into: fn(T) -> P) -> RequestId
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let req = RequestId(self.next_req);
        self.next_req += 1;
        let generation = self.generation();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let event = Event {
                generation,
                req: Some(req),
                kind: EventKind::Task(into(work())),
            };
            let _ = tx.send(event);
        });
        req
    }

    /// watcher など自前でスレッドを持つ源に渡す送信口。世代は送る時点のものを刻む。
    pub fn sender(&self) -> EventSender<P> {
        EventSender {
            tx: self.tx.clone(),
            generation: Arc::clone(&self.generation),
        }
    }

    /// 今の世代の Event を 1 つ返す。古い世代のものはここで捨てる。
    pub fn try_recv(&self) -> Option<Event<P>> {
        loop {
            let event = self.rx.try_recv().ok()?;
            if event.generation == self.generation() {
                return Some(event);
            }
        }
    }
}

#[derive(Clone)]
pub struct EventSender<P> {
    tx: Sender<Event<P>>,
    generation: Arc<AtomicU64>,
}

impl<P> EventSender<P> {
    pub fn send_watch(&self, event: watch::WatchEvent) {
        self.send(EventKind::Watch(event));
    }

    /// 途中経過を返す仕事のための送信口。1 回で終わる仕事は [Services::spawn] を使う。
    pub fn send_task(&self, payload: P) {
        self.send(EventKind::Task(payload));
    }

    fn send(&self, kind: EventKind<P>) {
        let _ = self.tx.send(Event {
            generation: Generation(self.generation.load(Ordering::Relaxed)),
            req: None,
            kind,
        });
    }
}

/// 終了時の join を期限付きにする。外部リソース待ちのスレッドで固まると端末を戻す前に
/// プロセスが止まり Ctrl+Q で抜けられなくなるので、後始末より抜けられることを優先する。
pub fn join_or_abandon(thread: thread::JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !thread.is_finished() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    thread.join().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recv_blocking<P: Send + 'static>(svc: &Services<P>) -> Option<Event<P>> {
        for _ in 0..200 {
            if let Some(e) = svc.try_recv() {
                return Some(e);
            }
            thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn 結果は投げた時の世代とrequest_idを持つ() {
        let mut svc = Services::<u32>::new();
        let req = svc.spawn(|| 41u32, |n| n + 1);
        let event = recv_blocking(&svc).unwrap();
        assert_eq!(event.req, Some(req));
        assert_eq!(event.generation, svc.generation());
        assert!(matches!(event.kind, EventKind::Task(42)));
    }

    #[test]
    fn 世代を進めると前の世代の結果は捨てられる() {
        let mut svc = Services::<u32>::new();
        svc.spawn(|| 1u32, |n| n);
        svc.bump_generation();
        svc.spawn(|| 2u32, |n| n);
        let event = recv_blocking(&svc).unwrap();
        assert!(matches!(event.kind, EventKind::Task(2)));
        assert!(svc.try_recv().is_none());
    }

    #[test]
    fn 世代を進めても_watcherの合図は届く() {
        let mut svc = Services::<u32>::new();
        let sender = svc.sender();
        svc.bump_generation();
        sender.send_watch(watch::WatchEvent::ConfigChanged);
        assert!(matches!(
            recv_blocking(&svc).map(|e| e.kind),
            Some(EventKind::Watch(watch::WatchEvent::ConfigChanged))
        ));
    }

    #[test]
    fn 世代を進めても途中経過は届く() {
        let mut svc = Services::<u32>::new();
        let sender = svc.sender();
        svc.bump_generation();
        sender.send_task(7);
        assert!(matches!(
            recv_blocking(&svc).map(|e| e.kind),
            Some(EventKind::Task(7))
        ));
    }

    #[test]
    fn join_or_abandonは期限で諦める() {
        let stuck = thread::spawn(|| thread::sleep(Duration::from_secs(5)));
        assert!(!join_or_abandon(stuck, Duration::from_millis(20)));
        let quick = thread::spawn(|| ());
        assert!(join_or_abandon(quick, Duration::from_millis(500)));
    }
}
