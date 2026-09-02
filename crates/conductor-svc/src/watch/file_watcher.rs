//! worktree のディレクトリを監視するファイルシステムウォッチャ。

use std::path::PathBuf;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use super::WatchEvent;
use crate::EventSender;

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn new<P: Send + 'static>(
        paths: &[PathBuf],
        sender: EventSender<P>,
    ) -> anyhow::Result<Self> {
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                let Ok(event) = result else { return };
                if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove()) {
                    return;
                }
                // .git/ .conductor/ 配下の変更は無視する。git の操作 (git status 等) や
                // revidere の解析結果書き込みで高コストなリフレッシュを誘発しないため。
                // 全パスを見るのは rename イベントが (from, to) を持ち、.git から作業
                // ツリーへの移動では paths[0] が .git 側になり得るから。
                let real_path = event.paths.iter().find(|path| {
                    !path
                        .components()
                        .any(|c| matches!(c.as_os_str().to_str(), Some(".git" | ".conductor")))
                });
                if let Some(path) = real_path {
                    sender.send_watch(WatchEvent::FsChanged(path.clone()));
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )?;

        for path in paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive)?;
            }
        }

        Ok(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Services;

    fn recv_watch(svc: &Services<()>) -> Option<WatchEvent> {
        for _ in 0..200 {
            if let Some(event) = svc.try_recv()
                && let crate::EventKind::Watch(w) = event.kind
            {
                return Some(w);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn ファイル変更を検知する() {
        let dir = tempfile::tempdir().unwrap();
        // macOS では /var/folders/... が /private/var/folders/... へのシンボリック
        // リンクで、notify の FSEvents バックエンドは解決後のパスで通知してくる。
        // 素の dir.path() のまま比較すると一致しない。
        let root = dir.path().canonicalize().unwrap();
        let svc = Services::<()>::new();
        let _watcher = FileWatcher::new(std::slice::from_ref(&root), svc.sender()).unwrap();

        let file = root.join("a.txt");
        std::fs::write(&file, "x").unwrap();

        let event = recv_watch(&svc).expect("event should arrive");
        assert!(matches!(event, WatchEvent::FsChanged(p) if p == file));
    }

    #[test]
    fn gitとconductor配下の変更は無視する() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir(root.join(".conductor")).unwrap();
        let svc = Services::<()>::new();
        let _watcher = FileWatcher::new(std::slice::from_ref(&root), svc.sender()).unwrap();

        std::fs::write(root.join(".git").join("HEAD"), "x").unwrap();
        std::fs::write(root.join(".conductor").join("review.json"), "x").unwrap();
        // 監視自体が効いていることを確かめる対照として、無視対象外のファイルも書く。
        let visible = root.join("visible.txt");
        std::fs::write(&visible, "x").unwrap();

        let event = recv_watch(&svc).expect("visible.txt should still be reported");
        assert!(matches!(event, WatchEvent::FsChanged(p) if p == visible));
    }
}
