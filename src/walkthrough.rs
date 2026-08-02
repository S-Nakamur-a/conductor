//! AI が生成する PR ウォークスルーのデータモデルと生成処理。
//!
//! ウォークスルーとは、モデルが書いたブランチ差分の順序付きツアーのこと
//! (レビューデータベースの walkthroughs と walkthrough_steps。永続化と問い合わせは
//! [crate::review_store::ReviewStore] 経由)。このモジュールは、ストアと UI ペインが
//! 共有する素のデータ型と、生成タスクそのものを持つ。
//!
//! 生成がどう走るか
//!
//! 設定可能な唯一の AI の継ぎ目 [crate::ai_caller] を通す。スマート worktree の
//! 命名とまったく同じで、プロンプトとパースは Conductor が持ち、どのモデルが
//! 答えるかは [api] provider / [api] command でユーザーが持つ。Conductor が
//! 自前で claude プロセスを起動することは決してない。この規則はコードベース全体に
//! 及んでおり、このモジュールが最後の例外だった。
//!
//! このタスクは本質的にエージェント的で、モデルは何かを語る前にブランチの差分と、
//! たいていはその周辺の呼び出し元・呼び出し先を読まねばならない。そのためコマンドは
//! 作業ディレクトリをレビュー対象の worktree に設定して実行され
//! ([crate::ai_caller] のプロトコルの説明を参照)、応答は 1 つの JSON オブジェクトとして
//! 返り、[parse_generated] がそれをステップへ変える。
//!
//! なぜ MCP のツール呼び出しではなく JSON で返すのか
//!
//! MCP の save_walkthrough ツールは今も存在し、外部の /conductor-walkthrough
//! コマンドは今もそれで保存する。この経路がそうしないのは、上の継ぎ目が素の
//! stdin/stdout のテキストプロトコルだから。Conductor が制御する argv が無いので、
//! --mcp-config を差し込む場所が無い。Conductor が JSON をパースして自分で行を書く。
//! これは同時に、形式の壊れた応答が generating のまま行を残すのではなく、
//! ここではっきり失敗することも意味する。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// ウォークスルーの生成状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughStatus {
    /// バックグラウンドの生成がステップを作っている最中。
    Generating,
    /// ステップが保存され、表示できる状態。
    Ready,
    /// 生成に失敗した。理由は Walkthrough::error が持つ。
    Failed,
}

impl WalkthroughStatus {
    /// データベースに格納する文字列表現へ変換する。
    pub fn as_str(&self) -> &'static str {
        match self {
            WalkthroughStatus::Generating => "generating",
            WalkthroughStatus::Ready => "ready",
            WalkthroughStatus::Failed => "failed",
        }
    }

    /// データベースに格納された文字列表現をパースする。
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "generating" => Some(WalkthroughStatus::Generating),
            "ready" => Some(WalkthroughStatus::Ready),
            "failed" => Some(WalkthroughStatus::Failed),
            _ => None,
        }
    }
}

impl std::fmt::Display for WalkthroughStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ウォークスルーのステップの種別。UI でのアイコンや強調を決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughStepKind {
    /// なぜこの変更があるのか。動機となった課題や要望。
    Intent,
    /// 変更の中心となる実装。
    Core,
    /// 中心の変更に伴う波及 (呼び出し箇所の更新、設定など)。
    Ripple,
    /// この変更のために追加・更新されたテスト。
    Test,
}

impl WalkthroughStepKind {
    /// データベースに格納する文字列表現へ変換する。
    pub fn as_str(&self) -> &'static str {
        match self {
            WalkthroughStepKind::Intent => "intent",
            WalkthroughStepKind::Core => "core",
            WalkthroughStepKind::Ripple => "ripple",
            WalkthroughStepKind::Test => "test",
        }
    }

    /// データベースに格納された文字列表現をパースする。
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "intent" => Some(WalkthroughStepKind::Intent),
            "core" => Some(WalkthroughStepKind::Core),
            "ripple" => Some(WalkthroughStepKind::Ripple),
            "test" => Some(WalkthroughStepKind::Test),
            _ => None,
        }
    }
}

impl std::fmt::Display for WalkthroughStepKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ブランチのウォークスルーのヘッダ行 (walkthroughs テーブル)。ブランチにつき 1 つで、
/// 再生成すると履歴を残さずこの行を削除して作り直す。
#[derive(Debug, Clone)]
pub struct Walkthrough {
    pub id: String,
    pub title: Option<String>,
    pub status: WalkthroughStatus,
    pub error: Option<String>,
    /// このウォークスルーを生成した対象のブランチ先端 (HEAD コミットの OID)。
    /// コミット追跡より前に作られた行では None。再生成の要求時に現在の HEAD が
    /// これと一致するなら飛ばす。差分が変わっていない = ウォークスルーも
    /// 変わらないため。
    pub head_commit: Option<String>,
}

/// ウォークスルーの順序付きステップ 1 つ (walkthrough_steps テーブル)。
/// ファイルと、任意で行範囲に紐づく。
#[derive(Debug, Clone)]
pub struct WalkthroughStep {
    pub id: String,
    pub file_path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub kind: WalkthroughStepKind,
    pub title: String,
    pub body: String,
}

/// 完成したウォークスルーを保存するときに渡すステップ。id も walkthrough_id も
/// 持たない (ストアが割り当てる。seq も同様にスライスの順序が示すので、ここには
/// 繰り返さない)。
///
/// スライスの順序がウォークスルーの順序であるのは意図的。MCP のツールはステップごとの
/// seq も受け取るが、それを信用すると、種別ごとに番号を振る呼び出し側
/// (intent 0,1 / core 0,1,2 / …) がツアーを入り乱れさせたまま成功を報告できてしまう。
/// ReviewStore::save_walkthrough を参照。
#[derive(Debug, Clone)]
pub struct NewWalkthroughStep {
    pub file_path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub kind: WalkthroughStepKind,
    pub title: String,
    pub body: String,
}

/// 生成 1 回あたりの実時間の予算。このタスクのタイムアウトとして AI の継ぎ目へ渡す
/// ので、[api] command_timeout_secs (スマート worktree の命名で数秒を想定した値)
/// に頭打ちにされない。ブランチの差分を読んで語るには数分かかる。これを超えたら
/// セッションが詰まっていると見なす。
pub const GENERATION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// システムプロンプト。ウォークスルーとは何か、そして応答の正確な形。
///
/// plugins/conductor/commands/conductor-walkthrough.md と対応している
/// (そちらはマーケットプレイスのプラグイン利用者向けに残してあり、MCP ツール経由で
/// 保存する) が、インストール済みのプラグインキャッシュがどのスラッシュコマンドを
/// 持っているかに関係なく生成が動くよう、ここに埋め込んである。
const GENERATION_SYSTEM_PROMPT: &str = r#"You build reviewer walkthroughs: an ordered tour of a branch's change that a reviewer follows step by step, each step anchored to a file and line range.

Use your tools freely: this task cannot be done without reading the repository in your working directory. Run git to see the diff, and read the changed files and the code around them. Only when you have finished exploring, answer with the JSON described below and nothing else.

Order the steps as a story: intent -> core -> ripple -> test.
- intent: what this change wanted to achieve (background, motivation).
- core: what was changed to achieve it, and its effect on existing code. Do NOT compare alternative designs — reviewers ask those questions themselves.
- ripple: knock-on changes (call-site updates, config/schema follow-ups).
- test: a summary of what behavior each test verifies, detailed enough that a reviewer can skip reading the full test diff.

There is no fixed step count — match the actual change.

Output ONLY a JSON object, no markdown fences and no explanation, with these fields:
- "title": a one-line title for the whole walkthrough.
- "summary": the overview of the change. This is stored as the branch's change summary and shown full-panel as Conductor's SUMMARY pseudo-file, so write it like a PR description — what the change is for, why these files are touched, and anything a reviewer should know up front (including what is deliberately out of scope). Markdown is rendered.
- "steps": an array, in tour order, of objects with:
    "file_path"  (string, repo-relative, e.g. "src/foo.rs" — never absolute, never prefixed with "a/" or "b/", never starting with "./")
    "line_start" (integer or null, 1-based line number on the NEW side)
    "line_end"   (integer or null, 1-based)
    "kind"       ("intent" | "core" | "ripple" | "test")
    "title"      (string, one line)
    "body"       (string, the explanation; Markdown is rendered)
- "comments": an array (possibly empty) of inline notes for the few spots that are genuinely hard to understand — tricky logic whose intent isn't obvious at a glance, a non-obvious tradeoff, or a subtle edge case a reviewer could miss. Each object has "file_path", "line_start", "line_end" (integer or null), and "body" (1-3 sentences explaining *why* it works / where the subtlety is). This is high-signal and low-frequency: a handful per change at most, and an empty array when nothing is genuinely tricky. Do NOT comment on self-evident changes (renames, boilerplate, formatting, imports).

Every file_path must be repo-relative: the reviewer's diff list matches these against git's own paths, so a step whose path is spelled any other way cannot be opened."#;

/// 実行ごとの指示。どのブランチか、どのベースか、どの言語か。
fn generation_prompt(branch: &str, base_ref: Option<&str>, language: Option<&str>) -> String {
    let base_hint = match base_ref {
        Some(b) => format!("The base branch is `{b}`."),
        None => "Determine the base branch (origin/HEAD, usually main).".to_string(),
    };
    let language_hint = match language {
        Some(lang) => format!(
            "\n\nWrite the walkthrough title, summary, and every step's title and body in {lang}."
        ),
        None => String::new(),
    };
    format!(
        "Read this branch's merge-base diff against its base branch and build a reviewer \
walkthrough of it.\n\
\n\
{base_hint} The branch under review is `{branch}`, checked out in your working directory. \
Use `git diff <base>...HEAD` (three-dot, merge-base) to see the change. Read not only the \
changed files but, where needed, their callers/callees so you understand the whole \
picture.\n\
\n\
When you have explored enough, reply with the JSON object described above and nothing \
else.{language_hint}"
    )
}

/// 生成が求めたインラインの注記 1 件。question コメントとして保存される。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GeneratedComment {
    pub file_path: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub body: String,
}

/// モデルから届いたままの、検証前のステップ。
#[derive(Debug, Clone, serde::Deserialize)]
struct GeneratedStep {
    file_path: String,
    #[serde(default)]
    line_start: Option<i64>,
    #[serde(default)]
    line_end: Option<i64>,
    kind: String,
    title: String,
    body: String,
}

/// 応答全体。検証前。
#[derive(Debug, Clone, serde::Deserialize)]
struct GeneratedWalkthrough {
    title: String,
    summary: String,
    steps: Vec<GeneratedStep>,
    #[serde(default)]
    comments: Vec<GeneratedComment>,
}

/// 検証済みの生成結果。[crate::review_store::ReviewStore] へ渡せる状態。
#[derive(Debug, Clone)]
pub struct Generated {
    pub title: String,
    pub summary: String,
    pub steps: Vec<NewWalkthroughStep>,
    pub comments: Vec<GeneratedComment>,
}

/// モデルの生の応答を保存できるウォークスルーに変えるか、どこがおかしいのかを説明する。
///
/// 外側の包み (Markdown のコードフェンス、JSON の前の一文) には寛容にする。
/// 指示に関わらずモデルがそれらを付けてくるから。一方で中身には厳しくする。
/// 未知の kind や、ファイルに紐づけられないパスをそのまま保存すると、
/// レビュアーが開けないステップとしてしか現れないため。同じデータが MCP 経由で
/// 届いたときに mcp_serve::tools::save_walkthrough が行う検査と対応させてあり、
/// 両方の入口が同じものを拒否する。
pub fn parse_generated(raw: &str) -> Result<Generated, String> {
    let json = extract_json_object(raw)
        .ok_or_else(|| format!("no JSON object in the model's reply\nRaw output: {raw}"))?;
    let parsed: GeneratedWalkthrough = serde_json::from_str(json)
        .map_err(|e| format!("could not parse the walkthrough JSON: {e}\nRaw output: {raw}"))?;

    if parsed.title.trim().is_empty() {
        return Err("the walkthrough has no title".to_string());
    }
    if parsed.summary.trim().is_empty() {
        return Err("the walkthrough has no summary".to_string());
    }
    if parsed.steps.is_empty() {
        return Err("the walkthrough has no steps".to_string());
    }

    let mut steps = Vec::with_capacity(parsed.steps.len());
    for (i, step) in parsed.steps.into_iter().enumerate() {
        let kind = WalkthroughStepKind::from_str(step.kind.trim())
            .ok_or_else(|| format!("step {i} has an unknown kind '{}'", step.kind))?;
        let file_path = crate::repo_path::normalize(&step.file_path);
        if file_path.is_empty() {
            return Err(format!("step {i} has no file_path"));
        }
        if file_path.starts_with('/') || file_path.split('/').any(|s| s == "..") {
            return Err(format!(
                "step {i} file_path must be repo-relative, got: {}",
                step.file_path
            ));
        }
        if step.title.trim().is_empty() {
            return Err(format!("step {i} ({file_path}) has no title"));
        }
        if step.body.trim().is_empty() {
            return Err(format!("step {i} ({file_path}) has no body"));
        }
        // 行番号は読み戻すどの場所でも 1 始まりで、逆転した範囲は何も下線を引かない。
        // 紐づけの細部でウォークスルー全体を失敗させるより、範囲のほうを捨てる。
        let (line_start, line_end) = sane_range(step.line_start, step.line_end);
        steps.push(NewWalkthroughStep {
            file_path,
            line_start,
            line_end,
            kind,
            title: step.title,
            body: step.body,
        });
    }

    // コメントはあくまで任意の付随物。おかしなものはログを 1 行残して捨て、
    // 他が問題ないウォークスルーを失敗させたりはしない。
    let comments = parsed
        .comments
        .into_iter()
        .filter_map(|mut c| {
            let path = crate::repo_path::normalize(&c.file_path);
            if path.is_empty()
                || path.starts_with('/')
                || path.split('/').any(|s| s == "..")
                || c.body.trim().is_empty()
                || c.line_start.is_none_or(|l| l == 0)
            {
                log::warn!("dropping malformed inline comment for {:?}", c.file_path);
                return None;
            }
            if let (Some(start), Some(end)) = (c.line_start, c.line_end)
                && end < start
            {
                c.line_end = None;
            }
            c.file_path = path;
            Some(c)
        })
        .collect();

    Ok(Generated {
        title: parsed.title,
        summary: parsed.summary,
        steps,
        comments,
    })
}

/// 1 始まりで逆転していない行範囲だけを残す。それ以外は誤った範囲ではなく
/// ファイル全体に紐づける。
fn sane_range(start: Option<i64>, end: Option<i64>) -> (Option<i64>, Option<i64>) {
    let start = start.filter(|s| *s >= 1);
    let end = end.filter(|e| *e >= 1);
    match (start, end) {
        (Some(s), Some(e)) if e < s => (Some(s), None),
        (None, Some(_)) => (None, None),
        pair => pair,
    }
}

/// コードフェンスで囲まれていたり地の文が前置されていたりする応答から、
/// JSON オブジェクトを見つける。
///
/// 最初の { を探してから波括弧の深さを追い、文字列リテラルの中に現れる波括弧は
/// 飛ばす。} を含む body (本文はコードを引用するので大いにあり得る) があると、
/// そうしなければ誤った位置でオブジェクトが切れ、ユーザーには対処のしようがない
/// パースエラーになる。
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 生成を 1 回最後まで走らせ、パース済みのウォークスルーを返す。
///
/// ブロッキング。呼び出し側はこれをバックグラウンドスレッドで実行し、結果を
/// チャネルで報告する (App::cmd_generate_walkthrough を参照)。cancel は
/// 下層の caller も見ているので、キャンセルされた生成はタイムアウトを待たずに
/// 子プロセスを kill する。
pub fn generate(
    api: &crate::config::ApiConfig,
    worktree_path: &Path,
    branch: &str,
    base_ref: Option<&str>,
    language: Option<&str>,
    cancel: &Arc<AtomicBool>,
) -> Result<Generated, String> {
    let env = crate::ai_caller::TaskEnv {
        timeout_secs: Some(GENERATION_TIMEOUT.as_secs()),
        working_dir: Some(worktree_path.to_path_buf()),
    };
    let caller = crate::ai_caller::build_caller(api, &env)?;
    let raw = caller.complete(
        GENERATION_SYSTEM_PROMPT,
        &generation_prompt(branch, base_ref, language),
        cancel,
    )?;
    parse_generated(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    // generation_prompt: ブランチ・ベース・言語

    #[test]
    fn generation_prompt_includes_the_branch_name() {
        let prompt = generation_prompt("pr-42", Some("main"), None);
        assert!(prompt.contains("pr-42"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn generation_prompt_falls_back_to_discovering_the_base() {
        let prompt = generation_prompt("pr-42", None, None);
        assert!(prompt.contains("Determine the base branch"));
    }

    #[test]
    fn generation_prompt_requests_the_configured_language() {
        let prompt = generation_prompt("pr-42", Some("main"), Some("日本語"));
        assert!(prompt.contains("in 日本語"));
        // 言語の設定が無ければ、言語の指示自体が出ない。
        let unconstrained = generation_prompt("pr-42", Some("main"), None);
        assert!(!unconstrained.contains("Write the walkthrough title"));
    }

    /// 代替設計を比較するなという指示と、ステップ順の筋書きは、いまはシステム
    /// プロンプトにある。ヘッドレスセッションから移行した際に失われていないことを
    /// 確かめる。
    #[test]
    fn system_prompt_keeps_the_walkthrough_contract() {
        assert!(GENERATION_SYSTEM_PROMPT.to_lowercase().contains("do not compare"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("intent -> core -> ripple -> test"));
        // ウォークスルーのステップが開けなかった原因はまさにパスの綴りだったので、
        // プロンプトはそこを明示しなければならない。
        assert!(GENERATION_SYSTEM_PROMPT.contains("repo-relative"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("never starting with \"./\""));
        // インラインの注記は MCP のツール呼び出しではなくここで要求するが、契約は
        // プラグインのコマンドが述べているものと同じ。本当に厄介な箇所にだけ、
        // 信号が強く頻度の低い形で注記する。
        assert!(GENERATION_SYSTEM_PROMPT.contains("hard to understand"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("\"comments\""));
    }

    /// スマート worktree の命名とは正反対の制約で、ユーザーが保守するラッパーでは
    /// なくここで述べる必要がある。何も指示されなかったエージェント型のコマンドは
    /// プロンプトだけで答えてしまうが、このタスクはリポジトリを読まずには不可能だから。
    #[test]
    fn system_prompt_tells_the_model_to_read_the_repo() {
        assert!(GENERATION_SYSTEM_PROMPT.contains("Use your tools freely"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("working directory"));
    }

    /// Conductor はどこでも claude を自分で起動してはならず、このモジュールが
    /// それをしていた最後の場所だった。プロンプトは CLI も MCP のツール呼び出しも
    /// 名指ししない。応答は [api] が指す先から JSON で返ってくる。
    #[test]
    fn generation_never_names_a_cli_to_run() {
        assert!(!GENERATION_SYSTEM_PROMPT.contains("claude -p"));
        assert!(!GENERATION_SYSTEM_PROMPT.contains("save_walkthrough"));
        let prompt = generation_prompt("pr-42", Some("main"), None);
        assert!(!prompt.contains("claude -p"));
        assert!(!prompt.contains("save_walkthrough"));
    }

    // parse_generated

    fn reply(steps: &str) -> String {
        format!(
            r#"{{"title":"T","summary":"S","steps":[{steps}]}}"#
        )
    }

    #[test]
    fn parses_a_well_formed_reply() {
        let raw = reply(
            r#"{"file_path":"src/a.rs","line_start":10,"line_end":12,"kind":"core","title":"t","body":"b"}"#,
        );
        let g = parse_generated(&raw).unwrap();
        assert_eq!(g.title, "T");
        assert_eq!(g.summary, "S");
        assert_eq!(g.steps.len(), 1);
        assert_eq!(g.steps[0].file_path, "src/a.rs");
        assert_eq!(g.steps[0].line_start, Some(10));
        assert_eq!(g.steps[0].line_end, Some(12));
        assert_eq!(g.steps[0].kind, WalkthroughStepKind::Core);
        assert!(g.comments.is_empty());
    }

    /// プロンプトに何と書いてあってもモデルは JSON をコードフェンスや前置きで
    /// 包むので、中身は厳しく見る一方で外側の包みは許容する。
    #[test]
    fn parses_through_fences_and_preamble() {
        let inner = reply(r#"{"file_path":"src/a.rs","kind":"intent","title":"t","body":"b"}"#);
        for wrapped in [
            format!("```json\n{inner}\n```"),
            format!("Here you go:\n{inner}\nHope that helps!"),
            format!("```\n{inner}\n```"),
        ] {
            let g = parse_generated(&wrapped).unwrap();
            assert_eq!(g.steps.len(), 1, "wrapped: {wrapped}");
        }
    }

    /// コードを引用する本文には波括弧が入る。対応する閉じ波括弧まで数えること、
    /// そして文字列内の波括弧を飛ばすことが、そうした応答が途中で切れて
    /// パースエラーになるのを防いでいる。
    #[test]
    fn parses_a_body_containing_braces() {
        let raw = r#"prose {"title":"T","summary":"S","steps":[{"file_path":"src/a.rs","kind":"core","title":"t","body":"fn main() { let x = {1}; }"}]} trailing"#;
        let g = parse_generated(raw).unwrap();
        assert_eq!(g.steps[0].body, "fn main() { let x = {1}; }");
    }

    /// 差分一覧が照合できない綴りをここで正規の形に揃える。生成されたステップが
    /// 必ず開けるようにするため。
    #[test]
    fn normalises_step_paths() {
        for spelling in ["./src/a.rs", "src//a.rs", "src/a.rs/", "  src/a.rs  "] {
            let raw = reply(&format!(
                r#"{{"file_path":"{spelling}","kind":"core","title":"t","body":"b"}}"#
            ));
            let g = parse_generated(&raw).unwrap();
            assert_eq!(g.steps[0].file_path, "src/a.rs", "spelling: {spelling}");
        }
    }

    #[test]
    fn rejects_paths_that_escape_the_repo() {
        for bad in ["/etc/passwd", "../secret", ""] {
            let raw = reply(&format!(
                r#"{{"file_path":"{bad}","kind":"core","title":"t","body":"b"}}"#
            ));
            assert!(parse_generated(&raw).is_err(), "path: {bad}");
        }
    }

    #[test]
    fn rejects_an_unknown_step_kind() {
        let raw = reply(r#"{"file_path":"src/a.rs","kind":"summary","title":"t","body":"b"}"#);
        let err = parse_generated(&raw).unwrap_err();
        assert!(err.contains("summary"), "should echo the bad kind: {err}");
    }

    #[test]
    fn rejects_empty_title_summary_and_steps() {
        assert!(parse_generated(r#"{"title":"","summary":"S","steps":[]}"#).is_err());
        assert!(
            parse_generated(
                r#"{"title":"T","summary":"S","steps":[{"file_path":"a.rs","kind":"core","title":"t","body":"b"}]}"#
            )
            .is_ok()
        );
        assert!(parse_generated(r#"{"title":"T","summary":"S","steps":[]}"#).is_err());
        assert!(parse_generated("no json here").is_err());
    }

    /// おかしな行範囲は、ウォークスルー全体を失敗させたり意味のない範囲に下線を
    /// 引いたりするのではなく、ステップをファイル全体に紐づける。
    #[test]
    fn sanitises_line_ranges() {
        assert_eq!(sane_range(Some(10), Some(5)), (Some(10), None));
        assert_eq!(sane_range(Some(0), Some(5)), (None, None));
        assert_eq!(sane_range(None, Some(5)), (None, None));
        assert_eq!(sane_range(Some(3), None), (Some(3), None));
        assert_eq!(sane_range(Some(3), Some(3)), (Some(3), Some(3)));
    }

    /// インラインコメントは任意の付随物。形式の壊れたものは捨てられ、
    /// 一緒に届いたウォークスルーはそのまま保存される。
    #[test]
    fn drops_malformed_comments_but_keeps_the_walkthrough() {
        let raw = r#"{"title":"T","summary":"S",
            "steps":[{"file_path":"src/a.rs","kind":"core","title":"t","body":"b"}],
            "comments":[
              {"file_path":"./src/a.rs","line_start":4,"line_end":6,"body":"why"},
              {"file_path":"/etc/passwd","line_start":1,"body":"escape"},
              {"file_path":"src/a.rs","line_start":0,"body":"zero line"},
              {"file_path":"src/a.rs","line_start":9,"body":"   "}
            ]}"#;
        let g = parse_generated(raw).unwrap();
        assert_eq!(g.steps.len(), 1);
        assert_eq!(g.comments.len(), 1);
        assert_eq!(g.comments[0].file_path, "src/a.rs");
        assert_eq!(g.comments[0].line_start, Some(4));
    }

    /// 逆転したコメントの範囲は、逆向きに描画される範囲として保存されるのではなく
    /// 1 行に潰れる。
    #[test]
    fn reversed_comment_range_collapses() {
        let raw = r#"{"title":"T","summary":"S",
            "steps":[{"file_path":"a.rs","kind":"core","title":"t","body":"b"}],
            "comments":[{"file_path":"a.rs","line_start":9,"line_end":2,"body":"why"}]}"#;
        let g = parse_generated(raw).unwrap();
        assert_eq!(g.comments[0].line_start, Some(9));
        assert_eq!(g.comments[0].line_end, None);
    }
}
