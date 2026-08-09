// 設定ファイル。持っているのは「AI をどう呼ぶか」だけ。
//
// revidere は特定の AI CLI を知らないし、既定値も持たない。何を起動するかは
// 使う人が用意して、ここで読む。渡し方のプロトコルは ai.rs に書いてある。
//
// 探索順は次の通りで、最初に見つかったものだけを読む（マージしない）。
//   1. --config <path>
//   2. <repo>/.revidere/config.toml   そのリポジトリだけの設定
//   3. ~/.config/revidere/config.toml 常用の設定

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// AI の実時間上限の既定（秒）。差分を読んで語るには数分かかる。
pub const DEFAULT_TIMEOUT_SECS: u64 = 15 * 60;

/// 設定が無いときに出す雛形。そのまま貼れば動く形にしてある。
pub const TEMPLATE: &str = r#"[ai]
# プロンプトを受け取って補完を stdout に出すコマンドを argv で書く。
# シェルは介さないので、パイプやリダイレクトは書けない。
#
#   {prompt}   組み立て済みのプロンプトに置き換わる。書かなければ stdin で渡す
#   {workdir}  レビュー対象のリポジトリのパスに置き換わる
#
# 例（プロンプトを位置引数で受け取り、-w で対象ディレクトリを取るコマンド）:
#   command = ["your-ai-cli", "-w", "{workdir}", "{prompt}"]
# 例（stdin からプロンプトを読むコマンド）:
#   command = ["ollama", "run", "llama3"]
#
# レビューは対象を読むだけの作業なので、書き込めるツールを開けておく理由は無い。
# 権限を絞るならコマンド側で絞る。
command = []

# 実時間の上限（秒）。0 で無効。
timeout_secs = 900
"#;

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ai: AiConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// 起動する argv。空なら未設定。
    pub command: Vec<String>,
    /// 実時間の上限（秒）。0 で無効。
    pub timeout_secs: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

impl AiConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

/// 探索するパスを順に返す。--config が無いときの候補。
pub fn candidates(repo: &Path) -> Vec<PathBuf> {
    let mut v = vec![repo.join(revidere::review::DIR).join("config.toml")];
    if let Some(home) = home_dir() {
        v.push(home.join(".config").join("revidere").join("config.toml"));
    }
    v
}

/// 設定を読む。どのファイルを読んだかも返す（読まなかったなら None）。
///
/// --config で明示したパスが無いのはエラー。書いたのに黙って既定へ落ちるのは、
/// 設定が効いていないことに気付けない。候補側は「無ければ次へ」でよい。
pub fn load(
    explicit: Option<&Path>,
    repo: &Path,
) -> Result<(Config, Option<PathBuf>), ConfigError> {
    if let Some(p) = explicit {
        let cfg = read(p)?;
        return Ok((cfg, Some(p.to_path_buf())));
    }
    for p in candidates(repo) {
        if p.exists() {
            let cfg = read(&p)?;
            return Ok((cfg, Some(p)));
        }
    }
    Ok((Config::default(), None))
}

fn read(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
    toml::from_str(&text).map_err(|e| ConfigError(format!("{}: {e}", path.display())))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// AI コマンドが無いときの案内。どこに何を書けばいいかまで出す。
pub fn missing_command_error(repo: &Path, loaded: Option<&Path>) -> String {
    let where_to = match loaded {
        Some(p) => format!("{} に [ai] command が無い（空）。", p.display()),
        None => format!(
            "設定ファイルが無い。次のどちらかに置く。\n  {}",
            candidates(repo)
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    };
    format!(
        "AI コマンドが設定されていない。\n\n{where_to}\n\n\
         revidere は AI CLI を同梱しない。プロンプトを受け取って補完を stdout に\n\
         出すコマンドを自分で用意して、次のように指す。\n\n\
         [ai]\n\
         command = [\"your-ai-cli\", \"-w\", \"{{workdir}}\", \"{{prompt}}\"]\n\n\
         雛形は `revidere config` で出せる。1 回だけ差し替えるなら --ai でもよい。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_has_no_command_and_the_default_timeout() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.ai.command.is_empty());
        assert_eq!(c.ai.timeout_secs, DEFAULT_TIMEOUT_SECS);
    }

    #[test]
    fn reads_the_ai_section() {
        let c: Config = toml::from_str(
            r#"
            [ai]
            command = ["your-ai-cli", "-w", "{workdir}", "{prompt}"]
            timeout_secs = 60
            "#,
        )
        .unwrap();
        assert_eq!(c.ai.command.len(), 4);
        assert_eq!(c.ai.timeout(), Duration::from_secs(60));
    }

    /// 片方だけ書いた設定でも、もう片方は既定で埋まる。
    #[test]
    fn a_partial_ai_section_keeps_the_other_default() {
        let c: Config = toml::from_str("[ai]\ncommand = [\"cat\"]").unwrap();
        assert_eq!(c.ai.timeout_secs, DEFAULT_TIMEOUT_SECS);
    }

    /// 雛形はそのまま読める TOML でなければ、貼っても動かない。
    #[test]
    fn the_template_parses() {
        let c: Config = toml::from_str(TEMPLATE).unwrap();
        assert!(c.ai.command.is_empty());
    }

    #[test]
    fn candidates_start_with_the_repository_local_file() {
        let v = candidates(Path::new("/repo"));
        assert_eq!(v[0], Path::new("/repo/.revidere/config.toml"));
    }

    #[test]
    fn an_explicit_config_that_is_missing_is_an_error() {
        let e = load(Some(Path::new("/no/such/config.toml")), Path::new("/repo")).unwrap_err();
        assert!(e.0.contains("/no/such/config.toml"), "{}", e.0);
    }

    /// 候補が無いのは異常ではない。--config と違い、黙って既定へ落ちてよい。
    /// （常用の設定は $HOME 側にあり得るので、ここでは失敗しないことだけ見る）
    #[test]
    fn a_missing_candidate_is_not_an_error() {
        let dir = std::env::temp_dir().join("revidere-test-no-config");
        assert!(load(None, &dir).is_ok());
    }

    /// repo 側の設定があれば、$HOME 側に何が在っても repo が読まれる。
    #[test]
    fn the_repository_local_config_wins() {
        let dir = std::env::temp_dir().join(format!("revidere-config-repo-{}", std::process::id()));
        let rev = dir.join(revidere::review::DIR);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&rev).unwrap();
        std::fs::write(rev.join("config.toml"), "[ai]\ncommand = [\"repo-side\"]").unwrap();

        let (cfg, from) = load(None, &dir).unwrap();
        assert_eq!(cfg.ai.command, ["repo-side"]);
        assert_eq!(from.unwrap(), rev.join("config.toml"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_missing_command_error_points_at_a_path_and_the_way_in() {
        let e = missing_command_error(Path::new("/repo"), None);
        assert!(e.contains("/repo/.revidere/config.toml"), "{e}");
        assert!(e.contains("command = ["), "貼れる形を出す: {e}");
    }
}
