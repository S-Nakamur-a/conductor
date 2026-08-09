// サブコマンドごとの引数の型。
//
// 全サブコマンドの旗を 1 つの型に集めると、そのサブコマンドが読まない旗
// （verify に --out など）を渡しても黙って無視される。使う旗だけを持つ型に
// 分けて、知らない旗・余分な位置引数はその場でエラーにする。

use crate::error::CliError;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug)]
pub struct AnalyzeArgs {
    pub repo: PathBuf,
    pub base: Option<String>,
    pub head: String,
    pub out: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub ai: Option<Vec<String>>,
    pub timeout: Option<Duration>,
    pub repair: bool,
    pub cache: bool,
}

/// verify・ledger・prompt はどれも「対象の差分をどう選ぶか」だけを持つ。
#[derive(Debug)]
pub struct DiffArgs {
    pub repo: PathBuf,
    pub base: Option<String>,
    pub head: String,
}

#[derive(Debug)]
pub struct ConfigArgs {
    pub repo: PathBuf,
    pub config: Option<PathBuf>,
    pub ai: Option<Vec<String>>,
    pub timeout: Option<Duration>,
    pub cache: bool,
}

#[derive(Debug)]
pub struct CheckArgs {
    pub repo: PathBuf,
    pub file: PathBuf,
}

impl AnalyzeArgs {
    pub fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut a = AnalyzeArgs {
            repo: current_dir()?,
            base: None,
            head: "HEAD".to_string(),
            out: None,
            config: None,
            ai: None,
            timeout: None,
            repair: true,
            cache: true,
        };
        let mut cur = Cursor::new(args);
        while let Some(flag) = cur.current() {
            match flag {
                "--repo" => a.repo = PathBuf::from(cur.value(flag)?),
                "--base" => a.base = Some(cur.value(flag)?),
                "--head" => a.head = cur.value(flag)?,
                "--out" => a.out = Some(PathBuf::from(cur.value(flag)?)),
                "--config" => a.config = Some(PathBuf::from(cur.value(flag)?)),
                "--ai" => a.ai = Some(split_ai(cur.value(flag)?)),
                "--timeout" => a.timeout = Some(parse_timeout(cur.value(flag)?)?),
                "--no-repair" => a.repair = false,
                "--no-cache" => a.cache = false,
                other => return Err(unexpected(other)),
            }
            cur.advance();
        }
        Ok(a)
    }
}

impl DiffArgs {
    pub fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut a = DiffArgs {
            repo: current_dir()?,
            base: None,
            head: "HEAD".to_string(),
        };
        let mut cur = Cursor::new(args);
        while let Some(flag) = cur.current() {
            match flag {
                "--repo" => a.repo = PathBuf::from(cur.value(flag)?),
                "--base" => a.base = Some(cur.value(flag)?),
                "--head" => a.head = cur.value(flag)?,
                other => return Err(unexpected(other)),
            }
            cur.advance();
        }
        Ok(a)
    }
}

impl ConfigArgs {
    pub fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut a = ConfigArgs {
            repo: current_dir()?,
            config: None,
            ai: None,
            timeout: None,
            cache: true,
        };
        let mut cur = Cursor::new(args);
        while let Some(flag) = cur.current() {
            match flag {
                "--repo" => a.repo = PathBuf::from(cur.value(flag)?),
                "--config" => a.config = Some(PathBuf::from(cur.value(flag)?)),
                "--ai" => a.ai = Some(split_ai(cur.value(flag)?)),
                "--timeout" => a.timeout = Some(parse_timeout(cur.value(flag)?)?),
                "--no-cache" => a.cache = false,
                other => return Err(unexpected(other)),
            }
            cur.advance();
        }
        Ok(a)
    }
}

impl CheckArgs {
    pub fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut repo = current_dir()?;
        let mut file = None;
        let mut cur = Cursor::new(args);
        while let Some(flag) = cur.current() {
            match flag {
                "--repo" => repo = PathBuf::from(cur.value(flag)?),
                other if other.starts_with('-') => return Err(unexpected(other)),
                other => file = Some(PathBuf::from(other)),
            }
            cur.advance();
        }
        let file = file.ok_or_else(|| {
            CliError::Usage("成果物のパスを渡してほしい: revidere check <file>".into())
        })?;
        Ok(CheckArgs { repo, file })
    }
}

/// 残りの引数を、位置を進めながら読む小さなカーソル。
struct Cursor<'a> {
    args: &'a [String],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, i: 0 }
    }

    fn current(&self) -> Option<&'a str> {
        self.args.get(self.i).map(String::as_str)
    }

    fn advance(&mut self) {
        self.i += 1;
    }

    /// 今見ている旗の値。読んだあとは位置がその値を指す（呼び出し側が advance する）。
    fn value(&mut self, flag: &str) -> Result<String, CliError> {
        self.advance();
        self.args
            .get(self.i)
            .cloned()
            .ok_or_else(|| CliError::Usage(format!("{flag} に値が無い")))
    }
}

fn current_dir() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|e| CliError::Io(e.to_string()))
}

fn parse_timeout(s: String) -> Result<Duration, CliError> {
    let secs: u64 = s
        .parse()
        .map_err(|_| CliError::Usage("--timeout は秒数".into()))?;
    Ok(Duration::from_secs(secs))
}

fn split_ai(s: String) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// 型が持たない旗、または余分な位置引数。
fn unexpected(arg: &str) -> CliError {
    if arg.starts_with('-') {
        CliError::Usage(format!("知らない引数: {arg}"))
    } else {
        CliError::Usage(format!("余分な引数: {arg}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn analyze_defaults_have_repair_and_cache_on() {
        let a = AnalyzeArgs::parse(&argv(&[])).unwrap();
        assert!(a.repair);
        assert!(a.cache);
        assert_eq!(a.head, "HEAD");
    }

    #[test]
    fn a_flag_only_analyze_understands_is_rejected_by_diff_args() {
        let e = DiffArgs::parse(&argv(&["--out", "x"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(_)));
    }

    #[test]
    fn check_requires_a_file_argument() {
        let e = CheckArgs::parse(&argv(&[])).unwrap_err();
        assert!(matches!(e, CliError::Usage(_)));
    }

    #[test]
    fn check_takes_the_bare_positional_as_the_file() {
        let a = CheckArgs::parse(&argv(&["review.json"])).unwrap();
        assert_eq!(a.file, PathBuf::from("review.json"));
    }

    #[test]
    fn a_flag_missing_its_value_is_a_usage_error() {
        let e = AnalyzeArgs::parse(&argv(&["--timeout"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(_)));
    }

    #[test]
    fn timeout_must_be_a_number_of_seconds() {
        let e = AnalyzeArgs::parse(&argv(&["--timeout", "soon"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(_)));
    }

    #[test]
    fn ai_command_is_split_on_whitespace() {
        let a = AnalyzeArgs::parse(&argv(&["--ai", "your-ai-cli -w {workdir}"])).unwrap();
        assert_eq!(a.ai.unwrap(), vec!["your-ai-cli", "-w", "{workdir}"]);
    }

    #[test]
    fn a_stray_positional_argument_is_rejected() {
        let e = DiffArgs::parse(&argv(&["extra"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(_)));
    }
}
