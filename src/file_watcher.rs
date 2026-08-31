//! ファイル変更を検知して自動リフレッシュするためのファイルシステム監視。
//!
//! notify クレートで worktree のディレクトリを監視し、イベントを
//! チャネル経由でメインイベントループへ送る。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// ファイル監視からメインループへ送られるイベント。
#[derive(Debug)]
pub enum FsEvent {
    /// ファイルが変更された。パスは .git / .conductor を除いた最初の 1 件。
    ///
    /// パスを載せているのは意味索引の作り直し (`semantic_index`) のため。
    /// あちらは変更が索引に載るファイルかどうかを gitignore で判定しており、
    /// 判定できないと target/ の書き換えで静穏時間が永久に来なくなる。
    Changed(PathBuf),
}

/// worktree のディレクトリを監視するファイルシステムウォッチャ。
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<FsEvent>,
}

impl FileWatcher {
    pub fn new(paths: &[PathBuf]) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();

        let sender = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    // 変更系のイベントだけ通知する (アクセスのみのイベントは無視)。
                    if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove())
                    {
                        return;
                    }
                    // .git/ .conductor/ 内の変更は無視する。git の操作
                    // (git status など) はインデックスファイルに触るので、そのままだと
                    // 高コストなリフレッシュが走ってしまう。.conductor はレビューの
                    // 解析が貯めた応答を書き続けるので同じ理由で外す (成果物ができた
                    // ことは解析の終了で分かるので、監視から知る必要が無い)。最初の 1 件
                    // だけでなくイベント内の全パスを見るのは、リネームイベントが
                    // (from, to) を持つため — .git から作業ツリーへの移動は、
                    // paths[0] が .git 側であっても実際の変更だから。
                    let real_path = event.paths.iter().find(|path| {
                        !path
                            .components()
                            .any(|c| matches!(c.as_os_str().to_str(), Some(".git" | ".conductor")))
                    });
                    if let Some(path) = real_path {
                        let _ = sender.send(FsEvent::Changed(path.clone()));
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
