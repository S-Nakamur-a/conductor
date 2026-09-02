//! 差し替え可能な AI 呼び出しの継ぎ目。
//!
//! どのプロバイダも (system_prompt, user_message) -> String を満たすだけでよい。
//! プロンプトの組み立ても応答のパースも呼び出し側が持つので、出力形式がプロバイダの
//! 境界を越えることがない。
//!
//! 組み込みは [GeminiCaller] ひとつ。他のモデルへの経路は [CommandCaller] で、
//! Conductor が特定の CLI を自前で起動することはない。
//!
//! 外部コマンドのプロトコル。ここは AI ツールそのものを指す設定で、タスクごとの
//! 振る舞い (出力形式、ツールの可否、見るディレクトリ) は補完を求める機能側が
//! プロンプトで決める。だからラッパースクリプトは要らない。
//!
//! - 起動は設定された argv の直接実行 (シェルを介さない)。
//! - どの引数にも {prompt} (組み立て済みのプロンプト) と {workdir} (タスクの
//!   ディレクトリ) を書ける。
//! - {prompt} があればプロンプトはその引数に入り stdin は空のまま閉じる。無ければ
//!   プロンプトを stdin へ書いて閉じる。
//! - 子プロセスはタスクのディレクトリで動くので、パスを相対に解決するツールは
//!   {workdir} 無しでも正しい場所に着地する。
//! - stdout がモデルの補完そのもの。整形も JSON 抽出もコマンド側でしない。
//! - 終了コード 0 が成功。非ゼロは stderr をエラーに載せる。stderr は診断のみ。
//! - タイムアウトとキャンセルでは子プロセスを kill する。通知はしない。

mod command;
mod gemini;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub use command::CommandCaller;
pub use gemini::GeminiCaller;

use crate::config::ApiConfig;

/// プロンプトを生の補完テキストに変えるプロバイダ。
///
/// 下層の呼び出しを中断できるなら cancel を尊重すること。
pub trait AiCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String>;
}

/// タスクがプロンプト以外に AI へ渡すもの。
///
/// どちらもプロバイダ単位ではなくタスク単位。数秒で終わる worktree の命名を想定した
/// [api] command_timeout_secs に、数分かかるレビューが頭打ちにされては困る。
#[derive(Debug, Clone, Default)]
pub struct TaskEnv {
    /// 設定されていれば [api] command_timeout_secs を上書きする。0 で無効。
    pub timeout_secs: Option<u64>,
    /// 外部コマンドを実行するディレクトリ。タスクが対象とする worktree。
    pub working_dir: Option<PathBuf>,
}

/// タスク向けに、設定された AI 呼び出しを組み立てる。
///
/// プロバイダ同士は独立していて、失敗しても黙って別へフォールバックしない。
///
/// 組み込みの claude プロバイダはあえて存在しない。Conductor の中から claude CLI を
/// 起動することはこのコードベースのどこでも許していない。Claude を動かすためにあるのが
/// provider = "command" で、ユーザーが直接 CLI を指せば背後のモデルを知る必要がない。
///
/// gemini は素の HTTP なのでリポジトリを読めない。リポジトリを読ませるタスクは、
/// エージェント型の CLI を指した command の下でのみ動く。
pub fn build_caller(api: &ApiConfig, env: &TaskEnv) -> Result<Box<dyn AiCaller>, String> {
    match api.provider.trim().to_lowercase().as_str() {
        "gemini" => Ok(Box::new(GeminiCaller {
            model: api.model.clone(),
            max_tokens: gemini::MAX_TOKENS,
        })),
        "command" => {
            if api.command.is_empty() {
                return Err(
                    "provider = \"command\" but [api] command is empty; set command = [\"...\"]"
                        .to_string(),
                );
            }
            Ok(Box::new(CommandCaller {
                cmd: api.command.clone(),
                timeout_secs: env.timeout_secs.unwrap_or(api.command_timeout_secs),
                working_dir: env.working_dir.clone(),
            }))
        }
        other => Err(format!(
            "unknown AI provider '{other}' (expected: gemini, command)"
        )),
    }
}

#[cfg(test)]
mod tests;
