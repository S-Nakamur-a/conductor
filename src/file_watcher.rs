//! File system watcher for auto-refresh on file changes.
//!
//! Uses the `notify` crate to watch worktree directories and sends events
//! through a channel to the main event loop.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Events sent from the file watcher to the main loop.
#[derive(Debug)]
pub enum FsEvent {
    /// One or more files changed.
    Changed,
}

/// File system watcher that monitors worktree directories.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<FsEvent>,
}

impl FileWatcher {
    /// Create a new file watcher monitoring the given paths.
    pub fn new(paths: &[PathBuf]) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();

        let sender = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    // Only notify on modifications (not access-only events).
                    if event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove() {
                        if let Some(path) = event.paths.first() {
                            // Skip changes inside .git/ directories — git
                            // operations (e.g. `git status`) touch index files
                            // and would otherwise trigger expensive refreshes.
                            if path.components().any(|c| {
                                c.as_os_str() == ".git" || c.as_os_str() == ".conductor"
                            }) {
                                return;
                            }
                            let _ = sender.send(FsEvent::Changed);
                        }
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

    /// Check for any pending file change events (non-blocking).
    pub fn poll(&self) -> Option<FsEvent> {
        self.rx.try_recv().ok()
    }
}
