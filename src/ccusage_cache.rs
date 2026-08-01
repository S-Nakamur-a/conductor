//! ccusage の結果をファイルに置くグローバルなキャッシュ。
//!
//! 複数の Conductor インスタンスで 1 つのキャッシュファイルを共有し、実際に
//! `npx ccusage` を走らせるのが同時に 1 プロセスだけになるようにする。
//! キャッシュは `~/.cache/conductor/ccusage-YYYYMMDD.json` に置く (1 日 1 ファイル)。

use std::fs::{self, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::CcusageInfo;

/// キャッシュした ccusage データのディスク上の表現。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// このキャッシュを書いた時刻 (Unix タイムスタンプ、秒)。
    updated_at: u64,
    total_tokens: u64,
    total_cost: f64,
}

/// 今日のキャッシュファイルのパスを返す: `~/.cache/conductor/ccusage-YYYYMMDD.json`。
fn cache_path() -> Option<PathBuf> {
    let cache_dir = dirs::cache_dir()?.join("conductor");
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    Some(cache_dir.join(format!("ccusage-{today}.json")))
}

/// 現在の Unix タイムスタンプ (秒)。
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// キャッシュファイルを読み、エントリが十分に新しければ (`max_age_secs` 秒以内に
/// 書かれていれば) その内容を返す。
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

/// 鮮度を問わずキャッシュを読む (起動直後にすぐ表示するため)。
pub fn read_any() -> Option<CcusageInfo> {
    let path = cache_path()?;
    let data = fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&data).ok()?;
    Some(CcusageInfo {
        total_tokens: entry.total_tokens,
        total_cost: entry.total_cost,
    })
}

/// キャッシュエントリをアトミックに書く (一時ファイルに書いてからリネーム)。
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
    // アトミックな書き込み: 隣に一時ファイルを作ってからリネームする。
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// ロックファイルのパスを返す: `~/.cache/conductor/ccusage.lock`。
fn lock_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("conductor").join("ccusage.lock"))
}

/// 排他ロックの取得を試みる (create_new はファイルが既にあると失敗する)。
/// 成功したら、呼び出し側が終了時に消せるようパスを返す。
fn try_lock() -> Option<PathBuf> {
    let path = lock_path()?;
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // 取り残されたロックへの備え: ロックファイルが 60 秒より古ければ、前のプロセスが
    // 後始末をせずにクラッシュした可能性が高い。消して先へ進む。
    if let Ok(meta) = fs::metadata(&path)
        && let Ok(modified) = meta.modified()
    {
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age.as_secs() > 60 {
            let _ = fs::remove_file(&path);
        }
    }
    // create_new はアトミックな O_CREAT|O_EXCL。他プロセスがロックを持っていれば失敗する。
    File::create_new(&path).ok()?;
    Some(path)
}

fn release_lock(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

/// `npx ccusage` を実行し、パースした結果を返すとともにキャッシュへ書く。
///
/// ロックファイルを使って、複数の Conductor インスタンスが同時に `npx ccusage`
/// を走らせないようにしている。既にロックが取られている場合は `None` を返す
/// (呼び出し側は既存のキャッシュにフォールバックすること)。
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
