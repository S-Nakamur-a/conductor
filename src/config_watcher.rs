//! Configuration file watcher.
//!
//! Monitors the parent directory of `config.toml` for file-system events and
//! forwards only events that match the config file name. Using the parent
//! directory instead of the file directly avoids losing the watch when an
//! editor saves atomically via a write-then-rename (which replaces the inode).

use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Events sent from the config watcher to the main loop.
#[derive(Debug)]
pub enum ConfigEvent {
    /// The config file was created, modified, or replaced.
    Changed,
}

/// File system watcher for the conductor configuration file.
///
/// Watches the *parent directory* of the config file in non-recursive mode so
/// that write-then-rename atomic saves (common in editors like Vim, neovim,
/// and most $EDITOR wrappers) still deliver events after an inode swap.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<ConfigEvent>,
}

impl ConfigWatcher {
    /// Create a new watcher for `config_path`.
    ///
    /// The parent directory of `config_path` is monitored; only events whose
    /// path matches the config file name produce a [`ConfigEvent`].
    pub fn new(config_path: &Path) -> anyhow::Result<Self> {
        let config_filename = config_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("config path has no filename"))?
            .to_os_string();
        let watch_dir = config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("config path has no parent directory"))?
            .to_path_buf();

        let (tx, rx) = mpsc::channel();

        let sender = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result
                    && (event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove())
                    && event
                        .paths
                        .iter()
                        .any(|p| matches_config_file(p, &config_filename))
                {
                    let _ = sender.send(ConfigEvent::Changed);
                }
            },
            Config::default(),
        )?;

        if watch_dir.exists() {
            watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
        }

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Check for a pending config-change event (non-blocking).
    pub fn poll(&self) -> Option<ConfigEvent> {
        self.rx.try_recv().ok()
    }
}

/// Return `true` when `path` refers to the given config file name.
///
/// Extracted as a pure function so the filename-matching logic can be unit
/// tested independently of the file system.
fn matches_config_file(path: &Path, config_filename: &OsStr) -> bool {
    path.file_name() == Some(config_filename)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::matches_config_file;

    #[test]
    fn matches_exact_filename() {
        let filename = OsStr::new("config.toml");
        assert!(matches_config_file(
            Path::new("/home/user/.config/conductor/config.toml"),
            filename
        ));
    }

    #[test]
    fn does_not_match_with_extension() {
        let filename = OsStr::new("config.toml");
        assert!(!matches_config_file(
            Path::new("/home/user/.config/conductor/config.toml.tmp"),
            filename
        ));
    }

    #[test]
    fn does_not_match_different_filename() {
        let filename = OsStr::new("config.toml");
        assert!(!matches_config_file(
            Path::new("/home/user/.config/conductor/other.toml"),
            filename
        ));
    }

    #[test]
    fn does_not_match_parent_dir_only() {
        let filename = OsStr::new("config.toml");
        assert!(!matches_config_file(
            Path::new("/home/user/.config/conductor/"),
            filename
        ));
    }

    #[test]
    fn matches_when_path_is_just_filename() {
        let filename = OsStr::new("config.toml");
        assert!(matches_config_file(Path::new("config.toml"), filename));
    }
}
