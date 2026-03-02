//! Startup version check against GitHub Releases.
//!
//! On startup, checks `GET /repos/S-Nakamur-a/conductor/releases/latest` via
//! `curl` (no extra dependencies). Results are cached at
//! `~/.cache/conductor/update-check.json` so the badge can appear instantly
//! while a fresh background fetch runs.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Information about the latest available release.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub release_url: String,
    pub tarball_url: String,
}

/// On-disk cache representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    updated_at: u64,
    latest_version: String,
    release_url: String,
    #[serde(default)]
    tarball_url: String,
}

/// Return the current crate version from `Cargo.toml`.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Return the cache file path: `~/.cache/conductor/update-check.json`.
fn cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("conductor").join("update-check.json"))
}

/// Current Unix timestamp in seconds.
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Read cached update info regardless of age.
///
/// Used for instant badge display on startup while a fresh background
/// fetch is in progress.
pub fn read_cache() -> Option<UpdateInfo> {
    let path = cache_path()?;
    let data = fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&data).ok()?;
    Some(UpdateInfo {
        latest_version: entry.latest_version,
        release_url: entry.release_url,
        tarball_url: entry.tarball_url,
    })
}

/// Write cache entry atomically.
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
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Query GitHub Releases API via `curl`, write cache, and return the result.
///
/// Returns `None` on network errors, 404 (no releases yet), or parse failures.
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

    // Strip leading 'v' if present (e.g. "v0.3.0" → "0.3.0").
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();

    log::debug!("latest release: {version} (current: {})", current_version());

    let info = UpdateInfo {
        latest_version: version,
        release_url: html_url,
        tarball_url,
    };
    write_cache(&info);
    Some(info)
}

/// Compare two semver strings (`major.minor.patch`).
///
/// Returns `true` if `latest` is strictly newer than `current`.
/// Non-parseable versions return `false`.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        assert_eq!(parts.len(), 3, "CARGO_PKG_VERSION should be major.minor.patch");
    }
}
