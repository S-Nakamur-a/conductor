// 差分を解析して成果物にする。
//
// モデルの選択・タイムアウト・キャンセルはホスト側の関心で、ここに二重に
// 置くと設定が 2 か所に散る。だから AI は [Ai] として注入させる。

use crate::cache::{self, Cache};
use crate::{
    coverage, diff, git, parse, prompt,
    review::{Review, Scope},
};
use std::path::{Path, PathBuf};

/// プロンプトを補完テキストに変えるもの。ホストが実装する。
pub trait Ai {
    fn complete(&self, system: &str, user: &str) -> Result<String, String>;

    /// 呼び先の見分け。貯めた応答の鍵に混ざる。
    ///
    /// ここが固定値だと、モデルを替えても前のモデルの答えが返り続ける。
    fn identity(&self) -> String;
}

#[derive(Debug)]
pub enum AnalyzeError {
    Git(git::GitError),
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
    /// 比較のベース。None なら [git::guess_base] に推定させる。
    pub base: Option<String>,
    /// 貯めた応答を使うか。false でも結果は貯め直す。
    pub cache: bool,
    /// どの成果物として残すか。区間そのものを決めるのは base の方なので、
    /// [Scope::SincePrevious] にするなら base に前回の起点コミットを渡すこと。
    pub scope: Scope,
}

/// 差分を解析して `<root>/.conductor/review.json` を書き、その内容を返す。
///
/// 対象は毎回「ベースとの共通祖先から今の作業ツリーまで」で、前回の分類は
/// 引き継がない。コミットが進んでも戻っても force push されても、今あるもの
/// だけで作り直せば正しい。
///
/// 説明もれが残っていても成果物は書いて返す。読めるレビューを捨てないため。
/// 呼ぶ側は `review.coverage.is_complete()` で見分ける。
pub fn analyze(o: &Options, ai: &dyn Ai) -> Result<Review, AnalyzeError> {
    let root = git::root(&o.repo)?;
    let base_ref = match o.base.as_deref() {
        Some(b) => b.to_string(),
        None => git::guess_base(&root)?,
    };
    let base_oid = git::short_oid(&root, &git::merge_base(&root, &base_ref)?)?;
    let head_oid = git::short_oid(&root, "HEAD")?;
    // 前回の対象コミットは上書きする前の成果物にしか残っていない。git を引く
    // のもここ — AI を待つ数分の間に HEAD が動くと、下で書き出す head と
    // 一覧が別々の時点を指すことになる。
    let since_previous = (o.scope == Scope::Base)
        .then(|| previous_head(&crate::review::artifact_path(&root, Scope::Base), &head_oid))
        .flatten()
        .map(|previous| since_previous(&root, previous, &head_oid));

    let text = git::diff(&root, &base_oid)?;
    if text.trim().is_empty() {
        return Err(AnalyzeError::NoDiff(format!(
            "{base_oid} から作業ツリーまでに差分が無い"
        )));
    }
    let d = diff::parse(&text);
    let ledger = d.positions();
    log::info!(
        "revidere: {} {base_oid}..worktree (head {head_oid}) / {} files / {} changed positions",
        root.display(),
        d.files.len(),
        ledger.len()
    );

    let store = Cache::new(cache_dir(&root), o.cache);
    let identity = ai.identity();
    let ask = |user: &str| -> Result<String, AnalyzeError> {
        let key = cache::key(&identity, prompt::SYSTEM, user, &text);
        if let Some((raw, at)) = store.get(&key) {
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

    let raw = ask(&prompt::user(&base_oid, &d.ledger_summary()))?;

    let mut r = parse::review(&raw, &base_oid, &head_oid)?;
    r.coverage = coverage::check(&ledger, &r.sections);

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

    r.since_previous = since_previous;

    let out = crate::review::artifact_path(&root, o.scope);
    write_artifact(&out, &r)?;
    log::info!("revidere: wrote {}", out.display());
    Ok(r)
}

/// 比べる起点にする、前の HEAD コミット。初回は None。
///
/// スキーマ版が違うものは読まない。読めた文字列をコミット ID として扱うのは
/// 推測になる。
///
/// HEAD が動いていなければ起点も動かさない。解析し直すだけで起点が今になると、
/// 読む前に最新化した人から進みが消える。差分が動いていなければ AI も呼ばない
/// 空振りに見える操作なのに、成果物は上書きされていてもう戻せない。
fn previous_head(artifact: &Path, head: &str) -> Option<String> {
    let text = std::fs::read_to_string(artifact).ok()?;
    let r = Review::from_json(&text).ok()?;
    if r.schema != crate::review::SCHEMA_VERSION {
        return None;
    }
    if r.head == head {
        return r.since_previous.map(|s| s.previous_head);
    }
    Some(r.head)
}

fn since_previous(root: &Path, previous_head: String, head: &str) -> crate::review::SincePrevious {
    // 履歴から消えたコミットからでも diff は取れることが多い (rebase 直後の
    // ようにオブジェクトがまだ残っている場合)。取れたかどうかと、辿れるか
    // どうかは別に伝える。
    let history_rewritten = !git::is_ancestor_of_head(root, &previous_head);
    let files = git::changed_files(root, &previous_head).ok();
    crate::review::SincePrevious {
        previous_head,
        head: head.to_string(),
        files,
        history_rewritten,
    }
}

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
