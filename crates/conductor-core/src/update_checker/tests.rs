use std::time::Duration;

use super::*;

/// GitHub /releases/latest の応答から、読んでいる 4 つの鍵だけを残したもの。
const RELEASE_JSON: &str = r#"{
  "tag_name": "v0.28.0",
  "html_url": "https://github.com/S-Nakamur-a/conductor/releases/tag/v0.28.0",
  "assets": [
    {
      "name": "conductor-0.28.0-aarch64-apple-darwin.tar.gz",
      "browser_download_url": "https://example.com/aarch64-apple-darwin.tar.gz",
      "size": 12
    },
    {
      "name": "conductor-0.28.0-x86_64-unknown-linux-gnu.tar.gz",
      "browser_download_url": "https://example.com/x86_64-unknown-linux-gnu.tar.gz"
    }
  ]
}"#;

fn asset(name: &str) -> ReleaseAsset {
    ReleaseAsset {
        name: name.to_string(),
        download_url: format!("https://example.com/{name}"),
    }
}

fn info() -> UpdateInfo {
    UpdateInfo {
        latest_version: "0.28.0".into(),
        assets: vec![asset("conductor-0.28.0-aarch64-apple-darwin.tar.gz")],
    }
}

#[test]
fn バージョンの新しさを比べる() {
    let cases = [
        ("2.0.0", "1.9.9", true),
        ("1.1.0", "1.0.9", true),
        ("1.0.1", "1.0.0", true),
        ("1.0.0", "1.0.0", false),
        ("0.9.0", "1.0.0", false),
        ("abc", "1.0.0", false),
        ("1.0.0", "abc", false),
        ("1.0", "1.0.0", false),
        ("1.0.0.1", "1.0.0", false),
    ];
    for (latest, current, expected) in cases {
        assert_eq!(is_newer(latest, current), expected, "{latest} vs {current}");
    }
}

#[test]
fn リリースの応答からタグとアセットを読む() {
    let parsed = parse_release(RELEASE_JSON).unwrap();
    assert_eq!(parsed.latest_version, "0.28.0", "先頭の v を落とす");
    let names: Vec<&str> = parsed.assets.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "conductor-0.28.0-aarch64-apple-darwin.tar.gz",
            "conductor-0.28.0-x86_64-unknown-linux-gnu.tar.gz",
        ]
    );
    assert_eq!(
        parsed.assets[0].download_url,
        "https://example.com/aarch64-apple-darwin.tar.gz"
    );
}

#[test]
fn タグの無い応答は読めない() {
    for body in ["{}", "not json", r#"{"assets": []}"#] {
        assert!(parse_release(body).is_none(), "{body}");
    }
}

#[test]
fn いまのターゲットトリプルが取れる() {
    if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
        assert!(current_target_triple().is_some());
    }
}

#[test]
fn プラットフォームに合うアセットだけを選ぶ() {
    let Some(triple) = current_target_triple() else {
        return;
    };
    let cases: [(Vec<ReleaseAsset>, bool); 4] = [
        (
            vec![
                asset("conductor-0.28.0-x86_64-apple-darwin.tar.gz"),
                asset("conductor-0.28.0-aarch64-apple-darwin.tar.gz"),
                asset("conductor-0.28.0-x86_64-unknown-linux-gnu.tar.gz"),
                asset("conductor-0.28.0-aarch64-unknown-linux-gnu.tar.gz"),
            ],
            true,
        ),
        (
            vec![asset("conductor-0.28.0-s390x-unknown-linux-gnu.tar.gz")],
            false,
        ),
        (
            vec![asset(&format!("conductor-0.28.0-{triple}.zip"))],
            false,
        ),
        (Vec::new(), false),
    ];
    for (assets, expected) in cases {
        let found = find_binary_asset(&assets);
        assert_eq!(found.is_some(), expected, "{assets:?}");
        if let Some(found) = found {
            assert!(found.name.contains(triple), "{}", found.name);
        }
    }
}

#[test]
fn キャッシュは寿命を過ぎたら読まない() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update-check.json");
    write_cache(&path, &info());

    assert_eq!(read_cache(&path, Duration::from_secs(3600)), Some(info()));
    assert!(
        read_cache(&path, Duration::ZERO).is_none(),
        "寿命 0 は「必ず取り直す」の意味"
    );

    let aged = std::fs::read_to_string(&path)
        .unwrap()
        .replace(&format!("{}", now_epoch_secs()), "0");
    std::fs::write(&path, aged).unwrap();
    assert!(read_cache(&path, Duration::from_secs(3600)).is_none());
}

#[test]
fn 壊れたキャッシュも無いキャッシュも読めないだけ() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.json");
    assert!(read_cache(&missing, Duration::from_secs(3600)).is_none());

    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, "{ nope").unwrap();
    assert!(read_cache(&broken, Duration::from_secs(3600)).is_none());
}
