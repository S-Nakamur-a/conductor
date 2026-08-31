//! 設定ファイルの監視。
//!
//! config.toml の親ディレクトリのファイルシステムイベントを監視し、設定
//! ファイル名に一致するイベントだけを転送する。ファイルそのものではなく親
//! ディレクトリを監視するのは、エディタが「書いてリネーム」でアトミックに
//! 保存したとき (inode が入れ替わる) に監視が外れないようにするため。

use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// 設定ファイル監視からメインループへ送られるイベント。
#[derive(Debug)]
pub enum ConfigEvent {
    /// 設定ファイルが作成・変更・置換された。
    Changed,
}

/// conductor の設定ファイル用のファイルシステムウォッチャ。
///
/// 設定ファイルの親ディレクトリを非再帰モードで監視する。こうすると、
/// 「書いてリネーム」によるアトミック保存 (Vim, neovim をはじめ大半の $EDITOR
/// ラッパーでよくある) で inode が入れ替わったあともイベントが届く。
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<ConfigEvent>,
}

impl ConfigWatcher {
    /// config_path を対象にしたウォッチャを作る。
    ///
    /// 監視するのは config_path の親ディレクトリで、[ConfigEvent] を出すのは
    /// パスが設定ファイル名に一致したイベントだけ。
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

    /// 未処理の設定変更イベントがあれば取り出す (ノンブロッキング)。
    pub fn poll(&self) -> Option<ConfigEvent> {
        self.rx.try_recv().ok()
    }
}

/// ファイルシステムと切り離してテストできるよう純粋関数にしてある。
fn matches_config_file(path: &Path, config_filename: &OsStr) -> bool {
    path.file_name() == Some(config_filename)
}

// テスト

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::matches_config_file;

    #[test]
    fn ファイル名が完全一致する() {
        let filename = OsStr::new("config.toml");
        assert!(matches_config_file(
            Path::new("/home/user/.config/conductor/config.toml"),
            filename
        ));
    }

    #[test]
    fn 拡張子が付いていれば一致しない() {
        let filename = OsStr::new("config.toml");
        assert!(!matches_config_file(
            Path::new("/home/user/.config/conductor/config.toml.tmp"),
            filename
        ));
    }

    #[test]
    fn 別のファイル名には一致しない() {
        let filename = OsStr::new("config.toml");
        assert!(!matches_config_file(
            Path::new("/home/user/.config/conductor/other.toml"),
            filename
        ));
    }

    #[test]
    fn 親ディレクトリだけでは一致しない() {
        let filename = OsStr::new("config.toml");
        assert!(!matches_config_file(
            Path::new("/home/user/.config/conductor/"),
            filename
        ));
    }

    #[test]
    fn パスがファイル名だけでも一致する() {
        let filename = OsStr::new("config.toml");
        assert!(matches_config_file(Path::new("config.toml"), filename));
    }
}
