use std::time::{Duration, SystemTime};

use super::*;

/// Claude Code 2.1.310 が statusLine に渡した stdin (2026-10-25 に本物で採った)。パスの値だけ差し替えた。
const MEASURED: &str = r#"{"session_id": "b5d5b659-ff4e-4a5c-af98-2a7e885c8729", "transcript_path": "/tmp/claude/projects/-tmp-repo/b5d5b659-ff4e-4a5c-af98-2a7e885c8729.jsonl", "cwd": "/tmp/repo", "scratchpad_dir": "/tmp/claude/scratchpad", "prompt_id": "f2be8f37-d9b9-4f1d-bf05-3759694146c2", "effort": {"level": "medium"}, "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"}, "workspace": {"current_dir": "/tmp/repo", "project_dir": "/tmp/repo", "added_dirs": []}, "version": "2.1.310", "output_style": {"name": "default"}, "cost": {"total_cost_usd": 0.3665348, "total_duration_ms": 14282, "total_api_duration_ms": 1603, "total_lines_added": 0, "total_lines_removed": 0}, "context_window": {"total_input_tokens": 71815, "total_output_tokens": 4, "context_window_size": 1000000, "current_usage": {"input_tokens": 2, "output_tokens": 4, "cache_creation_input_tokens": 45139, "cache_read_input_tokens": 26674}, "used_percentage": 7, "remaining_percentage": 93}, "exceeds_200k_tokens": false, "prompt_cache": {"warm": true, "caching_observed": true, "ttl": "1h", "expires_at": 1792942056, "requests": 1, "misses": 0, "expected_rebuilds": 0, "hit_ratio": 0.3714265821903502, "cache_write_tokens": 45139, "miss_recache_tokens": 0, "last_miss_at": null, "last_miss_cause": null, "miss_causes": {}, "recache_tokens_if_cold": 71815}, "fast_mode": false, "thinking": {"enabled": true}, "rate_limits": {"five_hour": {"used_percentage": 3, "resets_at": 1792955400}, "seven_day": {"used_percentage": 39, "resets_at": 1793354400}}}"#;
const SESSION: &str = "b5d5b659-ff4e-4a5c-af98-2a7e885c8729";

/// 採った時点の時刻 (5h の resets_at より前)。
fn measured_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_942_000)
}

fn with(session: &str, json: &str) -> String {
    let mut value: Value = serde_json::from_str(json).unwrap();
    value["session_id"] = Value::from(session);
    value.to_string()
}

#[test]
fn 写しはセッションの_id_の名前で置き_読むと文脈の使用率と_5h_7d_が出る() {
    let dir = tempfile::tempdir().unwrap();
    let place = dir.path().join("statusline");
    record(&place, MEASURED.as_bytes()).unwrap();
    assert!(place.join(format!("{SESSION}.json")).is_file());
    let usage = read(&place, measured_at());
    assert_eq!(usage.context.get(SESSION), Some(&7.0));
    assert_eq!(
        usage.five_hour,
        Some(Limit {
            used_percentage: 3.0,
            resets_at: 1_792_955_400
        })
    );
    assert_eq!(usage.seven_day.map(|l| l.used_percentage), Some(39.0));
}

#[test]
fn 置き場は他人に読めない形で作る() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let place = dir.path().join("statusline");
    record(&place, MEASURED.as_bytes()).unwrap();
    let mode = std::fs::metadata(&place).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "{mode:o}");
}

#[test]
fn json_でないものと_id_の形でないものは置かない() {
    let dir = tempfile::tempdir().unwrap();
    assert!(record(dir.path(), b"not json").is_err());
    assert!(record(dir.path(), with("../x", MEASURED).as_bytes()).is_err());
    assert!(record(dir.path(), b"{}").is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn リセット時刻を過ぎた枠は出さず_時刻ちょうども出さない() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), MEASURED.as_bytes()).unwrap();
    let at = |secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    assert!(read(dir.path(), at(1_792_955_399)).five_hour.is_some());
    assert!(read(dir.path(), at(1_792_955_400)).five_hour.is_none());
    let later = read(dir.path(), at(1_792_955_401));
    assert!(later.five_hour.is_none());
    assert!(later.seven_day.is_some());
}

#[test]
fn 枠ごとに_その枠を持つ写しのうち一番新しいものを読む() {
    let dir = tempfile::tempdir().unwrap();
    let older = with("00000000-0000-0000-0000-000000000001", MEASURED);
    record(dir.path(), older.as_bytes()).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let mut newer: Value =
        serde_json::from_str(&with("00000000-0000-0000-0000-000000000002", MEASURED)).unwrap();
    newer["rate_limits"]["five_hour"]["used_percentage"] = Value::from(50);
    newer["rate_limits"]
        .as_object_mut()
        .unwrap()
        .remove("seven_day");
    newer["context_window"]["used_percentage"] = Value::Null;
    record(dir.path(), newer.to_string().as_bytes()).unwrap();
    std::fs::write(dir.path().join("broken.json"), b"{").unwrap();

    let usage = read(dir.path(), measured_at());
    assert_eq!(usage.five_hour.map(|l| l.used_percentage), Some(50.0));
    assert_eq!(
        usage.seven_day.map(|l| l.used_percentage),
        Some(39.0),
        "7d を持つのは古い方だけ"
    );
    assert_eq!(
        usage.context.get("00000000-0000-0000-0000-000000000002"),
        None,
        "null の使用率は 0 % にしない"
    );
}

#[test]
fn ユーザの_statusline_は_local_project_user_の順に探す() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let write = |path: PathBuf, command: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = format!(r#"{{"statusLine":{{"type":"command","command":"{command}"}}}}"#);
        std::fs::write(path, json).unwrap();
    };
    assert_eq!(user_command(project.path(), config.path()), None);
    write(config.path().join("settings.json"), "user");
    assert_eq!(
        user_command(project.path(), config.path()).as_deref(),
        Some("user")
    );
    write(project.path().join(".claude/settings.json"), "project");
    assert_eq!(
        user_command(project.path(), config.path()).as_deref(),
        Some("project")
    );
    write(project.path().join(".claude/settings.local.json"), "local");
    assert_eq!(
        user_command(project.path(), config.path()).as_deref(),
        Some("local")
    );
}

#[test]
fn 中継はユーザのコマンドに同じ入力と印を渡し_その終了コードを返す() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let command = format!(
        "cat > {seen}; echo \"$CONDUCTOR_IN_STATUSLINE\" >> {seen}; exit 3",
        seen = seen.display()
    );
    assert_eq!(relay(&command, MEASURED.as_bytes()).unwrap(), 3);
    let got = std::fs::read_to_string(&seen).unwrap();
    assert_eq!(got, format!("{MEASURED}1\n"));
}

#[test]
fn 写せなくてもユーザのコマンドは走り_その終了コードを返す() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"").unwrap();
    assert_eq!(
        forward(MEASURED.as_bytes(), Some(&file), Some("exit 3"), false).unwrap(),
        3
    );
}

#[test]
fn 中継の印があれば写すだけでユーザのコマンドは走らせない() {
    let dir = tempfile::tempdir().unwrap();
    let ran = dir.path().join("ran");
    let command = format!("touch {}", ran.display());
    assert_eq!(
        forward(MEASURED.as_bytes(), Some(dir.path()), Some(&command), true).unwrap(),
        0
    );
    assert!(!ran.exists());
    assert!(dir.path().join(format!("{SESSION}.json")).is_file());
}

#[test]
fn ユーザの_statusline_が無ければ何も走らせず_写しは作る() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        forward(MEASURED.as_bytes(), Some(dir.path()), None, false).unwrap(),
        0
    );
    assert!(dir.path().join(format!("{SESSION}.json")).is_file());
}

#[test]
fn project_は_workspace_の_project_dir_を先に_無ければ_cwd_を見る() {
    assert_eq!(project_of(MEASURED.as_bytes()), PathBuf::from("/tmp/repo"));
    let mut value: Value = serde_json::from_str(MEASURED).unwrap();
    value["workspace"]["project_dir"] = Value::from("/tmp/project");
    value["cwd"] = Value::from("/tmp/cwd");
    assert_eq!(
        project_of(value.to_string().as_bytes()),
        PathBuf::from("/tmp/project")
    );
    value.as_object_mut().unwrap().remove("workspace");
    assert_eq!(
        project_of(value.to_string().as_bytes()),
        PathBuf::from("/tmp/cwd")
    );
}

#[test]
fn user_settings_の置き場は_claude_config_dir_を先に_空なら_home_を見る() {
    let home = Some(PathBuf::from("/home/me"));
    assert_eq!(
        config_dir_from(Some("/c".into()), home.clone()),
        Some(PathBuf::from("/c"))
    );
    assert_eq!(
        config_dir_from(Some("".into()), home.clone()),
        Some(PathBuf::from("/home/me/.claude"))
    );
    assert_eq!(
        config_dir_from(None, home),
        Some(PathBuf::from("/home/me/.claude"))
    );
}

#[test]
fn ユーザの_statusline_は_stdin_の_project_dir_の_settings_から書き換えのたびに引き直す() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let input = format!(
        r#"{{"workspace":{{"project_dir":"{}"}}}}"#,
        project.path().display()
    );
    let local = project.path().join(".claude/settings.local.json");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    let write = |command: &str| {
        let json = format!(r#"{{"statusLine":{{"type":"command","command":"{command}"}}}}"#);
        std::fs::write(&local, json).unwrap();
    };
    write("a");
    assert_eq!(
        user_for(input.as_bytes(), config.path()).as_deref(),
        Some("a")
    );
    write("b");
    assert_eq!(
        user_for(input.as_bytes(), config.path()).as_deref(),
        Some("b")
    );
    std::fs::remove_file(&local).unwrap();
    assert_eq!(user_for(input.as_bytes(), config.path()), None);
}
