//! Global file-based cache for ccusage results.
//!
//! Multiple Conductor instances share a single cache file so that only one
//! process actually runs `npx ccusage` at a time. The cache lives at
//! `~/.cache/conductor/ccusage-YYYYMMDD.json` (one file per day).

use std::fs::{self, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::CcusageInfo;

/// On-disk representation of cached ccusage data.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// Unix timestamp (seconds) when this cache was written.
    updated_at: u64,
    total_tokens: u64,
    total_cost: f64,
}

/// Return the cache file path for today: `~/.cache/conductor/ccusage-YYYYMMDD.json`.
fn cache_path() -> Option<PathBuf> {
    let cache_dir = dirs::cache_dir()?.join("conductor");
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    Some(cache_dir.join(format!("ccusage-{today}.json")))
}

/// Current Unix timestamp in seconds.
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Try to read the cache file and return its contents if the entry is fresh
/// enough (written within `max_age_secs` seconds ago).
pub fn read_if_fresh(max_age_secs: u64) -> Option<CcusageInfo> {
    let path = cache_path()?;
    let data = fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&data).ok()?;
    let age = now_epoch_secs().saturating_sub(entry.updated_at);
    if age <= max_age_secs {
        Some(CcusageInfo {
            total_tokens: entry.total_tokens,
            total_cost: entry.total_cost,
        })
    } else {
        None
    }
}

/// Read the cache regardless of freshness (for immediate startup display).
pub fn read_any() -> Option<CcusageInfo> {
    let path = cache_path()?;
    let data = fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&data).ok()?;
    Some(CcusageInfo {
        total_tokens: entry.total_tokens,
        total_cost: entry.total_cost,
    })
}

/// Write a cache entry atomically (write to temp file, then rename).
fn write_cache(info: &CcusageInfo) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let entry = CacheEntry {
        updated_at: now_epoch_secs(),
        total_tokens: info.total_tokens,
        total_cost: info.total_cost,
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    // Atomic write: write to a sibling temp file, then rename.
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Return the lock file path: `~/.cache/conductor/ccusage.lock`.
fn lock_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("conductor").join("ccusage.lock"))
}

/// Try to acquire an exclusive lock (create_new fails if file already exists).
/// Returns the path on success so the caller can remove it when done.
fn try_lock() -> Option<PathBuf> {
    let path = lock_path()?;
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Stale lock guard: if the lock file is older than 60 seconds, a previous
    // process likely crashed without cleaning up. Remove it so we can proceed.
    if let Ok(meta) = fs::metadata(&path) {
        if let Ok(modified) = meta.modified() {
            let age = SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default();
            if age.as_secs() > 60 {
                let _ = fs::remove_file(&path);
            }
        }
    }
    // create_new: atomic O_CREAT|O_EXCL — fails if another process holds the lock.
    File::create_new(&path).ok()?;
    Some(path)
}

fn release_lock(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

/// Run `npx ccusage` and return the parsed result, also writing it to cache.
///
/// Uses a lock file to prevent multiple Conductor instances from running
/// `npx ccusage` at the same time. If the lock is already held, returns
/// `None` (the caller should fall back to the existing cache).
pub fn fetch_and_cache() -> Option<CcusageInfo> {
    let lock = try_lock()?;

    let result = fetch_inner();

    release_lock(&lock);
    result
}

fn fetch_inner() -> Option<CcusageInfo> {
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let output = std::process::Command::new("npx")
        .args(["ccusage@17.1.3", "daily", "--json", "--since", &today])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let val: serde_json::Value = serde_json::from_str(&text).ok()?;
    let tokens = val
        .pointer("/totals/totalTokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cost = val
        .pointer("/totals/totalCost")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let info = CcusageInfo {
        total_tokens: tokens,
        total_cost: cost,
    };
    write_cache(&info);
    Some(info)
}
