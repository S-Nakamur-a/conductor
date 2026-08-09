// CLI 全体のエラーと、終了コードの割り当て。
//
// git.rs・ai.rs・parse.rs・config.rs はそれぞれ自分の失敗を型で持っている。
// ここではそれを 1 つに集めて、失敗の種類ごとに終了コードを 1 箇所で決める。
// String に潰さないのは、呼ぶ側が理由で振り分けられなくなるため。

use crate::{ai::AiError, config::ConfigError, parse::ParseError};
use revidere::git::GitError;

#[derive(Debug)]
pub enum CliError {
    /// 引数・サブコマンドが読めない。使い方を直せば済む。
    Usage(String),
    Git(GitError),
    Config(ConfigError),
    Ai(AiError),
    /// AI の応答が Review として読めない。
    Answer(ParseError),
    /// 成果物 JSON の読み書き。
    Json(serde_json::Error),
    /// ファイルの読み書き。
    Io(String),
    /// 上記に当てはまらない、その場限りの理由（差分が無い、など）。
    Message(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Usage(s) | CliError::Io(s) | CliError::Message(s) => write!(f, "{s}"),
            CliError::Git(e) => write!(f, "{e}"),
            CliError::Config(e) => write!(f, "{e}"),
            CliError::Ai(e) => write!(f, "{e}"),
            CliError::Answer(e) => write!(f, "{e}"),
            CliError::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<GitError> for CliError {
    fn from(e: GitError) -> Self {
        CliError::Git(e)
    }
}

impl From<ConfigError> for CliError {
    fn from(e: ConfigError) -> Self {
        CliError::Config(e)
    }
}

impl From<AiError> for CliError {
    fn from(e: AiError) -> Self {
        CliError::Ai(e)
    }
}

impl From<ParseError> for CliError {
    fn from(e: ParseError) -> Self {
        CliError::Answer(e)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::Json(e)
    }
}

/// 失敗を stderr に出し、終了コードを決めて返す。呼ぶのは [crate::run] だけ。
pub fn exit_code(result: Result<bool, CliError>) -> u8 {
    if let Err(e) = &result {
        eprintln!("失敗: {e}");
    }
    code(&result)
}

/// 0 成功 / 1 失敗 / 2 処理は通ったが充足検査に落ちた。
///
/// 2 を分けるのは、ホストが「非ゼロは失敗」と握り潰さず、検査落ちだけを
/// 別に扱えるようにするため。表示副作用を持たせずここだけで判定できるように
/// 分けてあり、下のテストはこの関数だけを見ている。
fn code(result: &Result<bool, CliError>) -> u8 {
    match result {
        Ok(true) => 0,
        Ok(false) => 2,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_code_zero() {
        assert_eq!(code(&Ok(true)), 0);
    }

    #[test]
    fn a_failed_coverage_check_is_code_two_not_one() {
        assert_eq!(code(&Ok(false)), 2);
    }

    #[test]
    fn an_error_is_code_one() {
        assert_eq!(code(&Err(CliError::Usage("x".into()))), 1);
    }
}
