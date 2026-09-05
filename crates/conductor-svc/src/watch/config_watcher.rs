//! 設定ファイル (config.toml) の監視。
//!
//! ファイルそのものではなく親ディレクトリを非再帰で監視する。エディタの
//! 「一時ファイルへ書いてからリネーム」保存だと inode が入れ替わるので、
//! ファイル自体を監視の対象にすると保存のたびに監視が外れてしまう。

use std::ffi::OsStr;
use std::path::Path;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use super::WatchEvent;
use crate::EventSender;

pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    pub fn new<P: Send + 'static>(
        config_path: &Path,
        sender: EventSender<P>,
    ) -> anyhow::Result<Self> {
        let config_filename = config_path
            .file_name()
            .ok_or_else(|| {
                anyhow::anyhow!("config path has no filename: {}", config_path.display())
            })?
            .to_os_string();
        let watch_dir = config_path
            .parent()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "config path has no parent directory: {}",
                    config_path.display()
                )
            })?
            .to_path_buf();

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                let Ok(event) = result else { return };
                if (event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove())
                    && event
                        .paths
                        .iter()
                        .any(|p| matches_config_file(p, &config_filename))
                {
                    sender.send_watch(WatchEvent::ConfigChanged);
                }
            },
            Config::default(),
        )?;

        if watch_dir.exists() {
            watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
        }

        Ok(Self { _watcher: watcher })
    }
}

fn matches_config_file(path: &Path, config_filename: &OsStr) -> bool {
    path.file_name() == Some(config_filename)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Services;

    #[test]
    fn ファイル名が完全一致する() {
        let path = Path::new("/home/user/.config/conductor/config.toml");
        assert!(matches_config_file(path, OsStr::new("config.toml")));
    }

    #[test]
    fn 拡張子が付いていれば一致しない() {
        let path = Path::new("/home/user/.config/conductor/config.toml.tmp");
        assert!(!matches_config_file(path, OsStr::new("config.toml")));
    }

    #[test]
    fn 別のファイル名には一致しない() {
        let path = Path::new("/home/user/.config/conductor/other.toml");
        assert!(!matches_config_file(path, OsStr::new("config.toml")));
    }

    #[test]
    fn 親ディレクトリだけでは一致しない() {
        let path = Path::new("/home/user/.config/conductor");
        assert!(!matches_config_file(path, OsStr::new("config.toml")));
    }

    #[test]
    fn パスがファイル名だけでも一致する() {
        let path = Path::new("config.toml");
        assert!(matches_config_file(path, OsStr::new("config.toml")));
    }

    #[test]
    fn 設定ファイルの変更を検知する() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();

        let svc = Services::<()>::new();
        let _watcher = ConfigWatcher::new(&config_path, svc.sender()).unwrap();

        std::fs::write(&config_path, "[api]\n").unwrap();

        for _ in 0..200 {
            if let Some(event) = svc.try_recv()
                && matches!(
                    event.kind,
                    crate::EventKind::Watch(WatchEvent::ConfigChanged)
                )
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("ConfigChanged should arrive");
    }
}
