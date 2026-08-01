//! ファイル変更を検知して自動リフレッシュするためのファイルシステム監視。
//!
//! `notify` クレートで worktree のディレクトリを監視し、イベントを
//! チャネル経由でメインイベントループへ送る。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// ファイル監視からメインループへ送られるイベント。
#[derive(Debug)]
pub enum FsEvent {
    /// 1 つ以上のファイルが変更された。
    Changed,
}

/// worktree のディレクトリを監視するファイルシステムウォッチャ。
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<FsEvent>,
}

impl FileWatcher {
    /// 指定したパス群を監視するウォッチャを作る。
    pub fn new(paths: &[PathBuf]) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();

        let sender = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    // 変更系のイベントだけ通知する (アクセスのみのイベントは無視)。
                    if !(event.kind.is_modify()
                        || event.kind.is_create()
                        || event.kind.is_remove())
                    {
                        return;
                    }
                    // .git/ と .conductor ディレクトリ内の変更は無視する。git の操作
                    // (`git status` など) はインデックスファイルに触るので、そのままだと
                    // 高コストなリフレッシュが走ってしまう。最初の 1 件だけでなく
                    // イベント内の全パスを見るのは、リネームイベントが (from, to) を
                    // 持つため — `.git` から作業ツリーへの移動は、`paths[0]` が `.git`
                    // 側であっても実際の変更だから。
                    let any_real_path = event.paths.iter().any(|path| {
                        !path
                            .components()
                            .any(|c| c.as_os_str() == ".git" || c.as_os_str() == ".conductor")
                    });
                    if any_real_path {
                        let _ = sender.send(FsEvent::Changed);
                    }
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )?;

        for path in paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive)?;
            }
        }

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// 未処理のファイル変更イベントがあれば取り出す (ノンブロッキング)。
    pub fn poll(&self) -> Option<FsEvent> {
        self.rx.try_recv().ok()
    }
}
