// 差分を解析して成果物にする。入口はここ 1 つ。
//
// AI をどう呼ぶかは持たない。ホストが [Ai] を実装して渡す。モデルの選択・
// タイムアウト・キャンセルはホスト側の関心で、ここに二重に置くと設定が
// 2 か所に散る。

use crate::cache::{self, Cache};
use crate::{coverage, diff, git, parse, prompt, review::Review};
use std::path::{Path, PathBuf};

/// プロンプトを補完テキストに変えるもの。ホストが実装する。
pub trait Ai {
    fn complete(&self, system: &str, user: &str) -> Result<String, String>;

    /// 呼び先の見分け。貯めた応答の鍵に混ざる。
    ///
    /// モデルを替えれば答えも変わるので、これが同じなら同じ答えでよい、と
    /// 言える粒度で返すこと。ここが固定値だと、モデルを替えても前のモデルの
    /// 答えが返り続ける。
    fn identity(&self) -> String;
}

#[derive(Debug)]
pub enum AnalyzeError {
    Git(git::GitError),
    /// 比較する差分が無い。
    NoDiff(String),
    /// AI の呼び出しそのものが失敗した。
    Ai(String),
    /// AI の応答が Review として読めない。
    Answer(parse::ParseError),
    Json(serde_json::Error),
    Io(String),
}

impl std::fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalyzeError::Git(e) => write!(f, "{e}"),
            AnalyzeError::NoDiff(s) | AnalyzeError::Ai(s) | AnalyzeError::Io(s) => write!(f, "{s}"),
            AnalyzeError::Answer(e) => write!(f, "{e}"),
            AnalyzeError::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AnalyzeError {}

impl From<git::GitError> for AnalyzeError {
    fn from(e: git::GitError) -> Self {
        AnalyzeError::Git(e)
    }
}

impl From<parse::ParseError> for AnalyzeError {
    fn from(e: parse::ParseError) -> Self {
        AnalyzeError::Answer(e)
    }
}

impl From<serde_json::Error> for AnalyzeError {
    fn from(e: serde_json::Error) -> Self {
        AnalyzeError::Json(e)
    }
}

/// 何を解析するか。
pub struct Options {
    /// 対象リポジトリ。ルートでなくてもよい（囲っているルートまで登る）。
    pub repo: PathBuf,
    /// 比較のベース。None なら origin/HEAD → main → master の順に推定する。
    pub base: Option<String>,
    /// 比較の先端。[git::WORKTREE] を渡すと、コミット間ではなく作業ツリーを見る。
    pub head: String,
    /// 貯めた応答を使うか。false なら AI に聞き直す（結果は貯め直す）。
    pub cache: bool,
}

/// 差分を解析して `<root>/.conductor/review.json` を書き、その内容を返す。
///
/// 説明もれが残っていても成果物は書いて返す。読めるレビューを捨てないため。
/// 呼ぶ側は `review.coverage.is_complete()` で見分ける。
pub fn analyze(o: &Options, ai: &dyn Ai) -> Result<Review, AnalyzeError> {
    let root = git::root(&o.repo)?;
    let base = match (o.base.as_deref(), o.head.as_str()) {
        (Some(b), _) => b.to_string(),
        // 作業ツリーを見るときのベースは HEAD しかあり得ない。
        (None, git::WORKTREE) => "HEAD".to_string(),
        (None, _) => git::guess_base(&root)?,
    };
    let head = o.head.clone();
    let base_oid = git::short_oid(&root, &base).unwrap_or_else(|_| base.clone());
    let head_oid = git::short_oid(&root, &head).unwrap_or_else(|_| head.clone());

    if head != git::WORKTREE && git::is_dirty(&root).unwrap_or(false) {
        log::warn!(
            "{} has uncommitted changes; the review covers {base}...{head}, \
             which may differ from what is on screen",
            root.display()
        );
    }

    let text = git::diff(&root, &base, &head)?;
    if text.trim().is_empty() {
        return Err(AnalyzeError::NoDiff(format!(
            "{base}...{head} に差分が無い"
        )));
    }
    let d = diff::parse(&text);
    let ledger = d.positions();
    log::info!(
        "revidere: {} {base}...{head} / {} files / {} changed positions",
        root.display(),
        d.files.len(),
        ledger.len()
    );

    // 1 回の抽出に数分かかる。同じ差分を同じ AI に聞き直す理由は無い。
    let store = Cache::new(cache_dir(&root), o.cache);
    let identity = ai.identity();
    let ask = |user: &str| -> Result<String, AnalyzeError> {
        let key = cache::key(&identity, prompt::SYSTEM, user, &text);
        if let Some((raw, at)) = store.get(&key) {
            // AI が動いていないことは必ず言う。黙って前の答えを返すのが
            // 一番たちが悪い。
            log::info!(
                "revidere: reusing a stored answer (no AI call): {}",
                at.display()
            );
            return Ok(raw);
        }
        let raw = ai
            .complete(prompt::SYSTEM, user)
            .map_err(AnalyzeError::Ai)?;
        if let Err(e) = store.put(&key, &raw) {
            log::warn!("revidere: could not store the answer (using it anyway): {e}");
        }
        Ok(raw)
    };

    // プロンプトには解決済みの ID を入れる。同じ範囲を HEAD~2 と呼んでも
    // コミット ID で呼んでも、同じ問いになって貯めた応答に当たる。
    let raw = ask(&prompt::user(&base_oid, &head_oid, &d.ledger_summary()))?;

    let mut r = parse::review(&raw, &base_oid, &head_oid)?;
    r.coverage = coverage::check(&ledger, &r.sections);

    // 説明の無い変更が残ったら、残りだけを渡して差し戻す。全部やり直させると
    // 正しく分類できていた部分まで揺れる。
    if !r.coverage.unclassified.is_empty() {
        log::info!(
            "revidere: {} changed positions are unexplained; asking again for those only",
            r.coverage.unclassified.len()
        );
        let gaps = coverage::gap_summary(&r.coverage.unclassified);
        let previous = serde_json::to_string(&r)?;
        match ask(&prompt::repair(&previous, &gaps)) {
            Ok(raw2) => match parse::review(&raw2, &base_oid, &head_oid) {
                Ok(mut r2) => {
                    r2.coverage = coverage::check(&ledger, &r2.sections);
                    // 悪化したら採らない。差し戻しで壊れることは起きる。
                    if r2.coverage.unclassified.len() < r.coverage.unclassified.len() {
                        r = r2;
                    } else {
                        log::info!("revidere: the retry did not improve; keeping the first answer");
                    }
                }
                Err(e) => {
                    log::warn!("revidere: the retry was unreadable, keeping the first answer: {e}")
                }
            },
            Err(e) => log::warn!("revidere: the retry failed, keeping the first answer: {e}"),
        }
    }

    let out = crate::review::artifact_path(&root);
    write_artifact(&out, &r)?;
    log::info!("revidere: wrote {}", out.display());
    Ok(r)
}

/// 貯めた応答の置き場。成果物と同じディレクトリの下。
fn cache_dir(root: &Path) -> PathBuf {
    root.join(crate::review::DIR).join("review-cache")
}

fn write_artifact(path: &Path, r: &Review) -> Result<(), AnalyzeError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AnalyzeError::Io(format!("{}: {e}", dir.display())))?;
    }
    let json = r.to_json()?;
    std::fs::write(path, json).map_err(|e| AnalyzeError::Io(format!("{}: {e}", path.display())))
}

#[cfg(test)]
#[path = "analyze_tests.rs"]
mod tests;
