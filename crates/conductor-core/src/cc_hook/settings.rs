//! フックを宣言する settings ファイルと、通知ソケットのパス。
//!
//! フックを仕掛ける側 (PTY の spawn) と受ける側 (リスナ) が別 crate にいるので、
//! 綴りの一致が要る値はここに集める。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::git_engine::conductor_dir;

/// SessionStart フックを宣言する settings を書き、`claude --settings` へ渡すパスを返す。
///
/// フックのコマンドは conductor 自身 (conductor cc-hook)。シェルスクリプトや jq を挟まず
/// バイナリと同じ成果物に載せるのは、別リリースチャネルに置くとずれた組み合わせで黙って
/// 効かなくなるため。絶対パスで書くので claude の PATH に conductor が無くてもよい。
/// 起動のたびに書き直し、conductor の置き場所の変更に追随する。
pub fn install_settings(repo_root: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not locate the conductor executable")?;
    let dir = conductor_dir(repo_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = dir.join("claude-hooks.json");

    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{} cc-hook", shell_quote(&exe.to_string_lossy())),
                }],
            }],
        },
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&settings)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

/// このリポジトリの cc-notify ソケット。
///
/// リスナはリポジトリにつき 1 つなので、linked worktree から起動してもメイン側を指す
/// [conductor_dir] に置く。フックへパスを渡す側も同じ解決をしなければ行き違う。
pub fn socket_path(repo_root: &Path) -> PathBuf {
    conductor_dir(repo_root).join("cc-notify.sock")
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

    #[test]
    fn ソケットは設定と同じディレクトリに置く() {
        let repo = tempfile::tempdir().expect("tmp repo");
        assert_eq!(
            socket_path(repo.path()),
            repo.path().join(".conductor").join("cc-notify.sock")
        );
    }
}
