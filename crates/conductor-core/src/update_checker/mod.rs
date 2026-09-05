//! GitHub Releases との照合と、ビルド済みバイナリの差し替え。
//!
//! ネットワークもプロセス起動も同期のまま。スレッドに載せるのは svc / tui の仕事。

mod install;
#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use install::{Progress, install};

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/S-Nakamur-a/conductor/releases/latest";
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// リリースに添付されたビルド済みバイナリ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
}

/// 入手できる最新リリース。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    updated_at: u64,
    #[serde(flatten)]
    info: UpdateInfo,
}

/// キャッシュが `max_age` 以内ならそれを、古ければ GitHub に問い合わせた結果を返す。
///
/// ネットワークエラーもリリース無しも `None`。呼び出し側は「まだ最新」と区別せず、
/// [is_newer] で改めて比べる。
pub fn check(max_age: Duration) -> Option<UpdateInfo> {
    let path = cache_path();
    if let Some(fresh) = path.as_deref().and_then(|p| read_cache(p, max_age)) {
        log::debug!("update check: using the cached answer");
        return Some(fresh);
    }
    let info = fetch()?;
    if let Some(path) = path.as_deref() {
        write_cache(path, &info);
    }
    Some(info)
}

fn cache_path() -> Option<PathBuf> {
    Some(
        dirs::cache_dir()?
            .join("conductor")
            .join("update-check.json"),
    )
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_cache(path: &Path, max_age: Duration) -> Option<UpdateInfo> {
    if max_age.is_zero() {
        return None;
    }
    let entry: CacheEntry = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let age = now_epoch_secs().saturating_sub(entry.updated_at);
    (age <= max_age.as_secs()).then_some(entry.info)
}

fn write_cache(path: &Path, info: &UpdateInfo) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let entry = CacheEntry {
        updated_at: now_epoch_secs(),
        info: info.clone(),
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn fetch() -> Option<UpdateInfo> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .ok()?;
    let response = client
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "conductor")
        .send()
        .inspect_err(|e| log::warn!("update check failed: {e}"))
        .ok()?;
    if !response.status().is_success() {
        log::warn!("update check failed: HTTP {}", response.status());
        return None;
    }
    let body = response.text().ok()?;
    parse_release(&body).inspect(|info| {
        log::debug!(
            "latest release: {} ({} assets)",
            info.latest_version,
            info.assets.len()
        );
    })
}

/// GitHub の release JSON からタグとアセットを取る。
fn parse_release(body: &str) -> Option<UpdateInfo> {
    let value: serde_json::Value = serde_json::from_str(body)
        .inspect_err(|e| log::warn!("could not parse the GitHub response: {e}"))
        .ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    let assets = value
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    Some(ReleaseAsset {
                        name: asset.get("name")?.as_str()?.to_string(),
                        download_url: asset.get("browser_download_url")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(UpdateInfo {
        latest_version: tag.strip_prefix('v').unwrap_or(tag).to_string(),
        assets,
    })
}

/// major.minor.patch を比べて latest が厳密に新しければ true。読めない側があれば false。
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parse(version: &str) -> Option<(u64, u64, u64)> {
        let parts: Vec<&str> = version.split('.').collect();
        let [major, minor, patch] = parts[..] else {
            return None;
        };
        Some((
            major.parse().ok()?,
            minor.parse().ok()?,
            patch.parse().ok()?,
        ))
    }
    match (parse(latest), parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// アセット名に使われている、いま動いているプラットフォームのターゲットトリプル。
pub fn current_target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// このプラットフォーム向けのビルド済みバイナリ。
pub fn find_binary_asset(assets: &[ReleaseAsset]) -> Option<&ReleaseAsset> {
    let triple = current_target_triple()?;
    assets
        .iter()
        .find(|a| a.name.contains(triple) && a.name.ends_with(".tar.gz"))
}
