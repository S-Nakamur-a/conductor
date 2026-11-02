//! 一覧で畳んだ repo と worktree。次に開いた窓でも同じ畳みを出すため、config.toml の隣に書く。
//!
//! config.toml に入れないのは、config watcher が書き込みを読み直して「再起動が要る」と言うのと、
//! 人が書く設定ではないため。repo の root と main worktree は同じ path なので、集合を分けて持つ。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fold {
    pub repos: BTreeSet<PathBuf>,
    pub worktrees: BTreeSet<PathBuf>,
}

pub fn file_path() -> PathBuf {
    conductor_core::config::config_file_path().with_file_name("sessions-fold.json")
}

/// 無いか読めなければ、何も畳んでいない。
pub fn load(path: &Path) -> Fold {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Fold::default(),
        Err(e) => {
            log::warn!("could not read {}: {e}", path.display());
            return Fold::default();
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => Fold {
            repos: paths(&value, "repos"),
            worktrees: paths(&value, "worktrees"),
        },
        Err(e) => {
            log::warn!("ignoring {}: {e}", path.display());
            Fold::default()
        }
    }
}

fn paths(value: &Value, key: &str) -> BTreeSet<PathBuf> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .collect()
}

pub fn save(path: &Path, fold: &Fold) -> Result<()> {
    let text = |set: &BTreeSet<PathBuf>| -> Vec<String> {
        set.iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    let value = json!({ "repos": text(&fold.repos), "worktrees": text(&fold.worktrees) });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, value.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 書いた畳みを読み戻すと同じ集合になる() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("sessions-fold.json");
        let fold = Fold {
            repos: [PathBuf::from("/r/app")].into(),
            worktrees: [
                PathBuf::from("/r/app"),
                PathBuf::from("/r/app-worktrees/a b"),
            ]
            .into(),
        };
        save(&path, &fold).unwrap();
        assert_eq!(load(&path), fold);
    }

    #[test]
    fn 無いか壊れたファイルは何も畳まない() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions-fold.json");
        assert_eq!(load(&path), Fold::default());
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load(&path), Fold::default());
        std::fs::write(&path, r#"{"repos": "/r/app", "worktrees": [1, "/r/b"]}"#).unwrap();
        assert_eq!(
            load(&path),
            Fold {
                repos: BTreeSet::new(),
                worktrees: [PathBuf::from("/r/b")].into(),
            }
        );
    }
}
