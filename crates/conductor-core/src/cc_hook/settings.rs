//! フックを宣言する settings ファイルと、通知ソケットのパス。
//!
//! フックを仕掛ける側 (PTY の spawn) と受ける側 (リスナ) が別 crate にいるので、
//! 綴りの一致が要る値はここに集める。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::git_engine::conductor_dir;

/// Claude Code のフックを宣言する settings を書き、`claude --settings` へ渡すパスを返す。
///
/// コマンドはどれも conductor 自身。シェルスクリプトや jq を挟まず
/// バイナリと同じ成果物に載せるのは、別リリースチャネルに置くとずれた組み合わせで黙って
/// 効かなくなるため。絶対パスで書くので claude の PATH に conductor が無くてもよい。
/// 起動のたびに書き直し、conductor の置き場所の変更に追随する。
pub fn install_settings(repo_root: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not locate the conductor executable")?;
    let exe = shell_quote(&exe.to_string_lossy());
    let dir = conductor_dir(repo_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = dir.join("claude-hooks.json");

    let hook = |sub: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("{exe} {sub}") }] }]);
    // active/waiting をプラグインでなく本体から宣言するのは、あれが別途インストール
    // した人にしか無いため。PostToolUse は承認ダイアログの waiting から抜ける唯一の口。
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": hook("cc-hook"),
            "UserPromptSubmit": hook("cc-signal active"),
            "PostToolUse": hook("cc-signal active"),
            "Stop": hook("cc-signal waiting"),
            "Notification": [{
                "matcher": "permission_prompt|elicitation_dialog|idle_prompt",
                "hooks": [{
                    "type": "command",
                    "command": format!("{exe} cc-signal waiting"),
                }],
            }],
        },
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&settings)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

/// sun_path の上限 (macOS 104 / Linux 108) から余裕を取った値。超えるパスは bind も
/// connect も通らないので、ソケットはリポジトリの中に置けない。
const SUN_PATH_MAX: usize = 100;

/// このリポジトリの cc-notify ソケット。
pub fn socket_path(repo_root: &Path) -> PathBuf {
    runtime_dir().join(format!("{}.sock", repo_key(repo_root)))
}

/// ソケットの居場所を書き残す先。環境変数を受け取れないクライアント (プラグインの
/// シェルスクリプト) はここを読んで宛先を知る。
pub fn socket_pointer_path(repo_root: &Path) -> PathBuf {
    conductor_dir(repo_root).join("cc-notify.path")
}

/// ソケットを置く自分専用のディレクトリ。共有の /tmp に落ちうるので 0700 を強制する。
pub fn ensure_runtime_dir() -> Result<PathBuf> {
    let dir = runtime_dir();
    ensure_private_dir(&dir)?;
    Ok(dir)
}

// 既にあるものは直さず断る。直そうとすると、他人が張った symlink を辿って
// その先の permission を書き換えてしまう。
fn ensure_private_dir(dir: &Path) -> Result<()> {
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("could not create {}", dir.display())),
    }

    let meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("could not inspect {}", dir.display()))?;
    let me = unsafe { libc::geteuid() };
    anyhow::ensure!(meta.is_dir(), "{} is not a directory", dir.display());
    anyhow::ensure!(
        meta.uid() == me,
        "{} belongs to uid {}, not {me}",
        dir.display(),
        meta.uid()
    );
    anyhow::ensure!(
        meta.permissions().mode() & 0o077 == 0,
        "{} is readable by others",
        dir.display()
    );
    Ok(())
}

// XDG_RUNTIME_DIR を先に見るのは、/tmp だと放置ファイルの掃除にライブのソケットを
// 持っていかれるため。
fn runtime_dir() -> PathBuf {
    let from_env = ["XDG_RUNTIME_DIR", "TMPDIR"]
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(PathBuf::from));
    runtime_dir_in(from_env)
}

fn runtime_dir_in(bases: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    let name = format!("conductor-{}", unsafe { libc::geteuid() });
    bases
        .into_iter()
        .map(|base| base.join(&name))
        // 相対パスは conductor の cwd に生えるので、別の cwd で走るフックから見えない。
        .find(|dir| dir.is_absolute() && dir.as_os_str().len() + SOCKET_NAME_MAX <= SUN_PATH_MAX)
        .unwrap_or_else(|| PathBuf::from("/tmp").join(&name))
}

/// "/" + 16 桁の鍵 + ".sock"。
const SOCKET_NAME_MAX: usize = 1 + 16 + 5;

// 鍵は conductor_dir 越しに採る。linked worktree もメイン側と同じ 1 本を指す必要があり、
// フックへパスを渡す spawn.rs も同じ解決をしないと行き違う。
// 自前の FNV-1a なのは、失敗しうるハッシュだと失敗時に全リポジトリが同じ名前へ落ちるため。
fn repo_key(repo_root: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in conductor_dir(repo_root).as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Claude Code はフックの command をシェルに渡すので、空白や引用符を含むパスは
/// そのままでは分割される。
fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 設定にsession_startフックが宣言される() {
        let repo = tempfile::tempdir().expect("tmp repo");
        let path = install_settings(repo.path()).expect("write settings");
        assert_eq!(
            path,
            repo.path().join(".conductor").join("claude-hooks.json")
        );

        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read back")).expect("valid json");
        let hook = &v["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(hook["type"], "command");
        // conductor 自身を呼ぶ。シェルスクリプトにも jq にも依存しない。
        assert!(
            hook["command"]
                .as_str()
                .expect("command")
                .ends_with(" cc-hook"),
            "{hook:?}"
        );
    }

    #[test]
    fn 設定にactiveとwaitingのフックが宣言される() {
        let repo = tempfile::tempdir().expect("tmp repo");
        let path = install_settings(repo.path()).expect("write settings");
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read back")).expect("valid json");

        let command = |event: &str| {
            v["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        assert!(command("UserPromptSubmit").ends_with(" cc-signal active"));
        assert!(command("Stop").ends_with(" cc-signal waiting"));
        assert!(command("PostToolUse").ends_with(" cc-signal active"));
        assert!(command("Notification").ends_with(" cc-signal waiting"));
        // 待ちの合図になる Notification だけを拾う。
        assert_eq!(
            v["hooks"]["Notification"][0]["matcher"],
            "permission_prompt|elicitation_dialog|idle_prompt"
        );
    }

    #[test]
    fn フック設定は起動のたびに書き直される() {
        let repo = tempfile::tempdir().expect("tmp repo");
        let path = install_settings(repo.path()).expect("first write");
        std::fs::write(&path, b"{}").expect("clobber");
        install_settings(repo.path()).expect("second write");

        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read back")).expect("valid json");
        assert!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"].is_string());
    }

    #[test]
    fn 扱いにくい実行パスでもフックのコマンドは1語のまま() {
        let cases = [
            ("/usr/bin/conductor", "'/usr/bin/conductor'"),
            (
                "/Users/me/my tools/conductor",
                "'/Users/me/my tools/conductor'",
            ),
            ("/tmp/it's here/conductor", r"'/tmp/it'\''s here/conductor'"),
        ];
        for (raw, want) in cases {
            assert_eq!(shell_quote(raw), want, "{raw}");
        }
    }

    /// 実測した worktree のパス。.conductor/ に置くと 123 バイトになり sun_path を超える。
    const DEEP_REPO: &str = "/Users/shunnaka/ghq/github.com/S-Nakamur-a/conductor-worktrees/show-claude-usage-limits-in-editor";

    #[test]
    fn 深いリポジトリでもソケットのパスはsun_pathに収まる() {
        let path = socket_path(Path::new(DEEP_REPO));
        assert!(
            path.as_os_str().len() <= SUN_PATH_MAX,
            "{} バイト: {}",
            path.as_os_str().len(),
            path.display()
        );
        assert!(!path.starts_with(DEEP_REPO), "{}", path.display());
    }

    #[test]
    fn ソケットのパスはリポジトリごとに違い同じリポジトリでは変わらない() {
        let (a, b) = (Path::new(DEEP_REPO), Path::new("/tmp/other"));
        assert_eq!(socket_path(a), socket_path(a));
        assert_ne!(socket_path(a), socket_path(b));
    }

    #[test]
    fn 宛先を書き残す先はリポジトリの中() {
        let repo = tempfile::tempdir().expect("tmp repo");
        assert_eq!(
            socket_pointer_path(repo.path()),
            repo.path().join(".conductor").join("cc-notify.path")
        );
    }

    #[test]
    fn 使える置き場を順に選び最後は必ずtmpへ落ちる() {
        let base = |s: &str| PathBuf::from(s);
        let long = base(&format!("/tmp/{}", "x".repeat(120)));

        // XDG_RUNTIME_DIR が先。実測の macOS TMPDIR も収まる。
        let xdg = base("/run/user/501");
        let tmpdir = base("/var/folders/n5/9k3sjy4s60l2zp0qcj8cnc2h0000gn/T");
        assert!(runtime_dir_in([xdg.clone(), tmpdir.clone()]).starts_with(&xdg));
        assert!(runtime_dir_in([tmpdir.clone()]).starts_with(&tmpdir));

        for unusable in [long.clone(), base("relative/dir"), base("")] {
            let dir = runtime_dir_in([unusable.clone(), tmpdir.clone()]);
            assert!(dir.starts_with(&tmpdir), "{}", dir.display());

            let alone = runtime_dir_in([unusable.clone()]);
            assert!(alone.starts_with("/tmp"), "{}", alone.display());
            assert!(alone.as_os_str().len() + SOCKET_NAME_MAX <= SUN_PATH_MAX);
        }
    }

    #[test]
    fn 先回りされたsymlinkは断り辿った先に触らない() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let victim = tmp.path().join("victim");
        std::fs::create_dir(&victim).expect("victim");
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let bait = tmp.path().join("bait");
        std::os::unix::fs::symlink(&victim, &bait).expect("symlink");

        assert!(ensure_private_dir(&bait).is_err(), "symlink を通した");
        assert_eq!(
            std::fs::metadata(&victim)
                .expect("victim")
                .permissions()
                .mode()
                & 0o777,
            0o755,
            "辿った先の permission を書き換えた"
        );
    }

    #[test]
    fn 自分の0700ディレクトリだけを受け入れる() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("runtime");

        ensure_private_dir(&dir).expect("create");
        let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        ensure_private_dir(&dir).expect("2 回目も通る");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(
            ensure_private_dir(&dir).is_err(),
            "他人に開いた dir を通した"
        );
        assert_eq!(mode(&dir), 0o755, "断ったのに書き換えた");

        let file = tmp.path().join("file");
        std::fs::write(&file, b"").expect("write");
        assert!(ensure_private_dir(&file).is_err(), "通常ファイルを通した");
    }

    #[test]
    fn 鍵は必ず16桁でリポジトリごとに違う() {
        let key = |s: &str| repo_key(Path::new(s));
        assert_eq!(key(DEEP_REPO).len(), 16);
        assert_eq!(key(DEEP_REPO), key(DEEP_REPO));
        assert_ne!(key(DEEP_REPO), key("/tmp/other"));
        // 空文字に落ちる経路があると、全リポジトリが 1 本のソケットを共有する。
        assert_eq!(key("").len(), 16);
    }
}
