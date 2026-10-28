//! Claude Code の statusLine の中継。5h / 7d の使用量はディスクのどこにも無く、statusLine の
//! stdin にしか来ないので、conductor が起こした Claude の statusLine を受けて置き場に写し、
//! ユーザ自身の statusLine へ同じ入力を渡す。ユーザの statusLine は描画のたびに settings から読み直す
//! — spawn で固めると、後から書き換えた statusLine が conductor の区画にだけ出ない。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde_json::Value;

/// 写しの置き場。窓が読む場所と同じにするため、spawn が渡す。
pub const DIR_ENV: &str = "CONDUCTOR_STATUSLINE_DIR";
/// 中継した子に付ける印。ユーザのコマンドが conductor を呼び返しても二重に中継しない。
pub const RELAYING_ENV: &str = "CONDUCTOR_IN_STATUSLINE";

/// ユーザのコマンドの終了コード。無ければ 0。
pub fn run() -> anyhow::Result<i32> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let dir = std::env::var_os(DIR_ENV).map(PathBuf::from);
    let relayed = std::env::var_os(RELAYING_ENV).is_some();
    let user = claude_config_dir().and_then(|config| user_for(&input, &config));
    forward(&input, dir.as_deref(), user.as_deref(), relayed)
}

fn user_for(input: &[u8], config_dir: &Path) -> Option<String> {
    user_command(&project_of(input), config_dir)
}

fn forward(
    input: &[u8],
    dir: Option<&Path>,
    user: Option<&str>,
    relayed: bool,
) -> anyhow::Result<i32> {
    if let Some(dir) = dir
        && let Err(e) = record(dir, input)
    {
        log::debug!("cc-statusline: could not record: {e:#}");
    }
    match user {
        Some(command) if !relayed => relay(command, input),
        _ => Ok(0),
    }
}

/// Claude が project の settings を読む dir。
fn project_of(input: &[u8]) -> PathBuf {
    let value: Option<Value> = serde_json::from_slice(input).ok();
    value
        .as_ref()
        .and_then(|v| {
            v.pointer("/workspace/project_dir")
                .or_else(|| v.get("cwd"))
                .and_then(Value::as_str)
        })
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

fn relay(command: &str, input: &[u8]) -> anyhow::Result<i32> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env(RELAYING_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    Ok(child.wait()?.code().unwrap_or(1))
}

/// 同じセッションの statusLine が 2 本重なっても、読み手が書きかけを見ないよう rename で置く。
pub fn record(dir: &Path, input: &[u8]) -> anyhow::Result<()> {
    let value: Value = serde_json::from_slice(input)?;
    let session = value
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| is_session_id(id))
        .ok_or_else(|| anyhow::anyhow!("no usable session_id"))?;
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let tmp = dir.join(format!(".{session}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, input)?;
    std::fs::rename(&tmp, dir.join(format!("{session}.json")))?;
    Ok(())
}

fn is_session_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Claude Code の user settings の置き場。CLAUDE_CONFIG_DIR を先に見て、無ければ ~/.claude。
pub fn claude_config_dir() -> Option<PathBuf> {
    config_dir_from(std::env::var_os("CLAUDE_CONFIG_DIR"), dirs::home_dir())
}

fn config_dir_from(var: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    var.filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".claude")))
}

/// その dir で起こす Claude が使う statusLine のコマンド。local → project → user の順。
pub fn user_command(project: &Path, config_dir: &Path) -> Option<String> {
    [
        project.join(".claude/settings.local.json"),
        project.join(".claude/settings.json"),
        config_dir.join("settings.json"),
    ]
    .iter()
    .find_map(|path| {
        let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let line = value.get("statusLine")?;
        (line.get("type")?.as_str()? == "command")
            .then(|| line.get("command")?.as_str().map(str::to_string))?
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    pub used_percentage: f64,
    /// Unix 秒。
    pub resets_at: i64,
}

impl Limit {
    pub fn secs_left(&self, now: SystemTime) -> i64 {
        let now = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        self.resets_at - now
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Usage {
    /// 会話の id → 文脈の使用率 (%)。
    pub context: HashMap<String, f64>,
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
}

/// 写しを読む。5h と 7d は別々に、その枠を持つ写しのうち一番新しいもの。リセット時刻を過ぎた枠は出さない。
pub fn read(dir: &Path, now: SystemTime) -> Usage {
    let mut usage = Usage::default();
    let mut newest: [Option<(SystemTime, Limit)>; 2] = [None, None];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return usage;
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()).ok() else {
            continue;
        };
        let Some(value) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        if let (Some(id), Some(pct)) = (
            value.get("session_id").and_then(Value::as_str),
            value
                .pointer("/context_window/used_percentage")
                .and_then(Value::as_f64),
        ) {
            usage.context.insert(id.to_string(), pct);
        }
        for (slot, key) in newest.iter_mut().zip(["five_hour", "seven_day"]) {
            let Some(limit) = limit_at(&value, key).filter(|l| l.secs_left(now) > 0) else {
                continue;
            };
            if slot.is_none_or(|(seen, _)| mtime > seen) {
                *slot = Some((mtime, limit));
            }
        }
    }
    let [five_hour, seven_day] = newest.map(|slot| slot.map(|(_, limit)| limit));
    usage.five_hour = five_hour;
    usage.seven_day = seven_day;
    usage
}

fn limit_at(value: &Value, key: &str) -> Option<Limit> {
    let limit = value.get("rate_limits")?.get(key)?;
    Some(Limit {
        used_percentage: limit.get("used_percentage")?.as_f64()?,
        resets_at: limit.get("resets_at")?.as_i64()?,
    })
}

/// 窓が写しを読む場所。spawn が [DIR_ENV] で渡すのも同じ場所。
pub fn dir() -> PathBuf {
    super::settings::runtime_dir().join("statusline")
}

#[cfg(test)]
mod tests;
