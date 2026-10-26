//! worktree のディレクトリを監視するファイルシステムウォッチャ。

use std::path::{Path, PathBuf};
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
                if event.paths.iter().any(|path| is_git_state(path)) {
                    sender.send_watch(WatchEvent::GitChanged);
                    return;
                }
                // .git/ .conductor/ 配下のその他の変更は無視する。objects/ の書き込みや
                // .conductor/ の成果物書き込みで高コストなリフレッシュを誘発しないため。
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

/// .git 配下のうち commit / checkout / fetch / add で動くもの。objects/ や logs/ は
/// 1 回の操作で何十件も書かれるので拾わない。linked worktree の HEAD と index は
/// main 側の `.git/worktrees/<name>/` にある。
fn is_git_state(path: &Path) -> bool {
    const STATE: [&str; 5] = ["HEAD", "ORIG_HEAD", "index", "packed-refs", "refs"];
    let mut inside = path
        .components()
        .map(|c| c.as_os_str())
        .skip_while(|c| *c != ".git")
        .skip(1);
    let entry = match inside.next() {
        Some(first) if first == "worktrees" => inside.nth(1),
        first => first,
    };
    entry
        .and_then(|c| c.to_str())
        .is_some_and(|name| STATE.contains(&name))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Services;

    #[test]
    fn gitの状態ファイルを見分ける() {
        for (path, want) in [
            ("/r/.git/HEAD", true),
            ("/r/.git/index", true),
            ("/r/.git/index.lock", false),
            ("/r/.git/packed-refs", true),
            ("/r/.git/refs/heads/main", true),
            ("/r/.git/refs/remotes/origin/main", true),
            ("/r/.git/worktrees/wt/HEAD", true),
            ("/r/.git/worktrees/wt/index", true),
            ("/r/.git/worktrees/wt/refs/bisect/bad", true),
            ("/r/.git/worktrees/wt/logs/HEAD", false),
            ("/r/.git/objects/ab/cdef", false),
            ("/r/.git/logs/HEAD", false),
            ("/r/.git/COMMIT_EDITMSG", false),
            ("/r/.git", false),
            ("/r/src/HEAD", false),
        ] {
            assert_eq!(is_git_state(Path::new(path)), want, "{path}");
        }
    }

    #[test]
    fn gitの状態が動いたら_git_changed_を送る() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git").join("worktrees").join("wt")).unwrap();
        let svc = Services::<()>::new();
        let _watcher = FileWatcher::new(std::slice::from_ref(&root), svc.sender()).unwrap();

        std::fs::write(
            root.join(".git").join("worktrees").join("wt").join("index"),
            "x",
        )
        .unwrap();

        assert_eq!(recv_watch(&svc), Some(WatchEvent::GitChanged));
    }

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
    fn gitのobjectsとconductor配下の変更は無視する() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git").join("objects")).unwrap();
        std::fs::create_dir(root.join(".conductor")).unwrap();
        let svc = Services::<()>::new();
        let _watcher = FileWatcher::new(std::slice::from_ref(&root), svc.sender()).unwrap();

        std::fs::write(root.join(".git").join("objects").join("ab"), "x").unwrap();
        std::fs::write(root.join(".conductor").join("review.json"), "x").unwrap();
        // 監視自体が効いていることを確かめる対照として、無視対象外のファイルも書く。
        let visible = root.join("visible.txt");
        std::fs::write(&visible, "x").unwrap();

        let event = recv_watch(&svc).expect("visible.txt should still be reported");
        assert!(matches!(event, WatchEvent::FsChanged(p) if p == visible));
    }
}
