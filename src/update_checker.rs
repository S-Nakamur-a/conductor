//! 起動時に GitHub Releases と照らしてバージョンを確認する。
//!
//! 起動時に curl で GET /repos/S-Nakamur-a/conductor/releases/latest を叩く
//! (追加の依存は無し)。結果は ~/.cache/conductor/update-check.json に
//! キャッシュするので、バックグラウンドで最新を取りに行っているあいだも
//! バッジをすぐ出せる。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// GitHub Release からダウンロードできるバイナリアセット。
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
}

/// 入手可能な最新リリースの情報。
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub release_url: String,
    pub tarball_url: String,
    /// リリースに添付されたビルド済みバイナリのアセット。
    pub assets: Vec<ReleaseAsset>,
}

/// キャッシュ用にシリアライズできる [ReleaseAsset] の形。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedAsset {
    name: String,
    download_url: String,
}

/// ディスク上のキャッシュの表現。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    updated_at: u64,
    latest_version: String,
    release_url: String,
    #[serde(default)]
    tarball_url: String,
    #[serde(default)]
    assets: Vec<CachedAsset>,
}

/// Cargo.toml にある現在のクレートのバージョンを返す。
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// キャッシュファイルのパスを返す: ~/.cache/conductor/update-check.json。
fn cache_path() -> Option<PathBuf> {
    Some(
        dirs::cache_dir()?
            .join("conductor")
            .join("update-check.json"),
    )
}

/// 現在の Unix タイムスタンプ (秒)。
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 鮮度を問わずキャッシュ済みの更新情報を読む。
///
/// バックグラウンドで最新を取得しているあいだ、起動時にバッジを即座に
/// 表示するために使う。
pub fn read_cache() -> Option<UpdateInfo> {
    let path = cache_path()?;
    let data = fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&data).ok()?;
    Some(UpdateInfo {
        latest_version: entry.latest_version,
        release_url: entry.release_url,
        tarball_url: entry.tarball_url,
        assets: entry
            .assets
            .into_iter()
            .map(|a| ReleaseAsset {
                name: a.name,
                download_url: a.download_url,
            })
            .collect(),
    })
}

/// キャッシュエントリをアトミックに書く。
fn write_cache(info: &UpdateInfo) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let entry = CacheEntry {
        updated_at: now_epoch_secs(),
        latest_version: info.latest_version.clone(),
        release_url: info.release_url.clone(),
        tarball_url: info.tarball_url.clone(),
        assets: info
            .assets
            .iter()
            .map(|a| CachedAsset {
                name: a.name.clone(),
                download_url: a.download_url.clone(),
            })
            .collect(),
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// curl で GitHub Releases API に問い合わせ、キャッシュを書いて結果を返す。
///
/// ネットワークエラー、404 (まだリリースが無い)、パース失敗のときは None を返す。
pub fn check_for_update() -> Option<UpdateInfo> {
    use std::process::Stdio;

    log::debug!("checking GitHub API for latest release");

    let output = match std::process::Command::new("curl")
        .args([
            "-sfL",
            "--max-time",
            "5",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            &format!("User-Agent: conductor/{}", current_version()),
            "https://api.github.com/repos/S-Nakamur-a/conductor/releases/latest",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            log::warn!("failed to run curl: {e}");
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!(
            "update check failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
        return None;
    }

    let text = String::from_utf8(output.stdout).ok()?;
    let val: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("failed to parse GitHub API response: {e}");
            return None;
        }
    };

    let tag = val.get("tag_name")?.as_str()?;
    let html_url = val
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let tarball_url = val
        .get("tarball_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // 先頭に 'v' があれば落とす (例: "v0.3.0" → "0.3.0")。
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();

    // リリースからバイナリアセットを読み取る。
    let assets = val
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let name = a.get("name")?.as_str()?.to_string();
                    let download_url = a.get("browser_download_url")?.as_str()?.to_string();
                    Some(ReleaseAsset { name, download_url })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    log::debug!(
        "latest release: {version} (current: {}), {} assets",
        current_version(),
        assets.len()
    );

    let info = UpdateInfo {
        latest_version: version,
        release_url: html_url,
        tarball_url,
        assets,
    };
    write_cache(&info);
    Some(info)
}

/// semver 形式の文字列 (major.minor.patch) を 2 つ比較する。
///
/// latest が current より厳密に新しければ true を返す。
/// パースできないバージョンは false を返す。
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<(u64, u64, u64)> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };

    let Some((lmaj, lmin, lpat)) = parse(latest) else {
        return false;
    };
    let Some((cmaj, cmin, cpat)) = parse(current) else {
        return false;
    };

    (lmaj, lmin, lpat) > (cmaj, cmin, cpat)
}

/// 現在のプラットフォームに対応する Rust のターゲットトリプルを返す。
///
/// (std::env::consts::OS, std::env::consts::ARCH) を、リリースのアセット名で
/// 使われるトリプル (例: aarch64-apple-darwin) へ対応づける。
pub fn current_target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// 現在のプラットフォームに合うビルド済みバイナリのアセットを探す。
///
/// 名前にターゲットトリプルを含み .tar.gz で終わるアセットを探す。
/// 見つかればそれを返す。
pub fn find_binary_asset(assets: &[ReleaseAsset]) -> Option<&ReleaseAsset> {
    let triple = current_target_triple()?;
    assets
        .iter()
        .find(|a| a.name.contains(triple) && a.name.ends_with(".tar.gz"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_major() {
        assert!(is_newer("2.0.0", "1.9.9"));
    }

    #[test]
    fn newer_minor() {
        assert!(is_newer("1.1.0", "1.0.9"));
    }

    #[test]
    fn newer_patch() {
        assert!(is_newer("1.0.1", "1.0.0"));
    }

    #[test]
    fn same_version() {
        assert!(!is_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn older_version() {
        assert!(!is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn invalid_latest() {
        assert!(!is_newer("abc", "1.0.0"));
    }

    #[test]
    fn invalid_current() {
        assert!(!is_newer("1.0.0", "abc"));
    }

    #[test]
    fn two_part_version() {
        assert!(!is_newer("1.0", "1.0.0"));
    }

    #[test]
    fn current_version_is_valid() {
        let v = current_version();
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "CARGO_PKG_VERSION should be major.minor.patch"
        );
    }

    #[test]
    fn current_target_triple_returns_some() {
        let triple = current_target_triple();
        if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
            assert!(
                triple.is_some(),
                "expected a target triple on this platform"
            );
        }
    }

    fn make_assets() -> Vec<ReleaseAsset> {
        vec![
            ReleaseAsset {
                name: "conductor-0.28.0-x86_64-apple-darwin.tar.gz".to_string(),
                download_url: "https://example.com/x86_64-apple-darwin.tar.gz".to_string(),
            },
            ReleaseAsset {
                name: "conductor-0.28.0-aarch64-apple-darwin.tar.gz".to_string(),
                download_url: "https://example.com/aarch64-apple-darwin.tar.gz".to_string(),
            },
            ReleaseAsset {
                name: "conductor-0.28.0-x86_64-unknown-linux-gnu.tar.gz".to_string(),
                download_url: "https://example.com/x86_64-unknown-linux-gnu.tar.gz".to_string(),
            },
        ]
    }

    #[test]
    fn find_binary_asset_matches_current_platform() {
        let assets = make_assets();
        let found = find_binary_asset(&assets);
        if current_target_triple().is_some() {
            assert!(found.is_some());
            let triple = current_target_triple().unwrap();
            assert!(found.unwrap().name.contains(triple));
        }
    }

    #[test]
    fn find_binary_asset_no_match() {
        let assets = vec![ReleaseAsset {
            name: "conductor-0.28.0-s390x-unknown-linux-gnu.tar.gz".to_string(),
            download_url: "https://example.com/s390x.tar.gz".to_string(),
        }];
        if current_target_triple().is_some() {
            assert!(find_binary_asset(&assets).is_none());
        }
    }

    #[test]
    fn find_binary_asset_ignores_non_tar_gz() {
        let triple = match current_target_triple() {
            Some(t) => t,
            None => return,
        };
        let assets = vec![ReleaseAsset {
            name: format!("conductor-0.28.0-{triple}.zip"),
            download_url: "https://example.com/zip".to_string(),
        }];
        assert!(find_binary_asset(&assets).is_none());
    }

    #[test]
    fn find_binary_asset_empty() {
        assert!(find_binary_asset(&[]).is_none());
    }
}
