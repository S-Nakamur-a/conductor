// 差分を解析して成果物にする。入口はここ 1 つ。
//
// AI をどう呼ぶかは持たない。ホストが [Ai] を実装して渡す。モデルの選択・
// タイムアウト・キャンセルはホスト側の関心で、ここに二重に置くと設定が
// 2 か所に散る。

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
    /// 貯めた応答を使うか。false なら AI に聞き直す（結果は貯め直す）。
    pub cache: bool,
    /// どちらの区間のレビューとして書き出すか。置き場がこれで決まる。
    ///
    /// [Scope::SincePrevious] を選ぶなら base に前回の起点コミットを渡すこと。
    /// ここは「どの成果物として残すか」だけを決め、区間そのものは base が決める。
    pub scope: Scope,
}

/// 差分を解析して `<root>/.conductor/review.json` を書き、その内容を返す。
///
/// 対象は毎回「ベースとの共通祖先から今の作業ツリーまで」で、成果物があっても
/// 無くても同じ。前回の分類は一切引き継がない — コミットが進んでも、戻っても、
/// force push で履歴ごと変わっても、今あるものだけで作り直せば正しい。
/// 前回からの進みは [Review::since_previous] に別途持たせる。
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
    // 前回からの進みを持つのはブランチ全体のレビューだけ。前回からの差分を
    // 見ているレビューにとっては、それ自体が進みなので入れ子になる。
    //
    // 上書きする前に読む。前回の対象コミットはこの成果物にしか残っていない。
    // git を引くのもここ — AI を待つ数分の間に HEAD が動くと、下で書き出す
    // head と一覧が別々の時点を指すことになる。
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
    let raw = ask(&prompt::user(&base_oid, &d.ledger_summary()))?;

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

    r.since_previous = since_previous;

    let out = crate::review::artifact_path(&root, o.scope);
    write_artifact(&out, &r)?;
    log::info!("revidere: wrote {}", out.display());
    Ok(r)
}

/// 比べる起点にする、前の HEAD コミット。無ければ（初回なら）None。
///
/// スキーマ版が違うものは読まない。head の意味が版によって変わりうる以上、
/// 読めた文字列をコミット ID として扱うのは推測になる。
///
/// HEAD が動いていなければ起点も動かさない。解析し直すだけで起点が今になると、
/// 読む前に最新化した人から進みが消える。差分が動いていなければ AI を呼ばずに
/// 即座に返る操作なので、ただの空振りに見えて実際には成果物を上書きしていて、
/// 前の起点はもうどこにも残っていない。
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

/// 前回の HEAD から今の作業ツリーまでの進み。
fn since_previous(root: &Path, previous_head: String, head: &str) -> crate::review::SincePrevious {
    // 辿れないコミットからでも diff は取れることが多い（rebase 直後のように
    // オブジェクトがまだ残っている場合）。取れなければファイル一覧は None に
    // して、履歴が変わったことだけを伝える。
    let history_rewritten = !git::is_ancestor_of_head(root, &previous_head);
    let files = git::changed_files(root, &previous_head).ok();
    crate::review::SincePrevious {
        previous_head,
        head: head.to_string(),
        files,
        history_rewritten,
    }
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
