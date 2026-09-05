//! 2 つの索引の持ち場と、その作り直しを 1 周進める場所。
//!
//! 判断は core 側の状態機械が持っている。ここがやるのは、重い仕事を Task へ出し、
//! 帰ってきたものを取り込み、結果を画面の語彙 (ステータス・起動演出のバー) に直すこと。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use conductor_core::semantic_index::{Reading, Regenerated, SemanticIndex, Survey};
use conductor_core::symbol_index::SymbolIndex;
use sheaf_core::Store;

use crate::effect::Effect;
use crate::fx::{Kind, Target};
use crate::layout::Region;
use crate::task::{Task, TaskResult};
use crate::workspace::{StatusLevel, Workspace};

/// tree-sitter の索引を作り直すまでの静穏時間。編集 1 打ごとにツリーを歩かない。
const SYMBOLS_QUIET: Duration = Duration::from_millis(500);

pub struct Index {
    /// 名前で引く構文層。意味索引が答えられない位置がここへ落ちる。
    pub symbols: SymbolIndex,
    /// SCIP の意味層。確信度つきで答える。
    pub semantic: SemanticIndex,
    /// 走っている調査。1 本に絞る — ツリーを歩くのは実測で列挙 149ms、鍵 1 本 110ms。
    surveying: bool,
    building_symbols: bool,
    /// 生成が置いた成果物をまだ読んでいない。調査の側からは「変わっていない」ように
    /// 見えるので、読み直しの理由はここに持つ。
    reload: bool,
    /// 変更が入った時刻。
    symbols_dirty_since: Option<Instant>,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            symbols: SymbolIndex::new(PathBuf::new()),
            semantic: SemanticIndex::default(),
            surveying: false,
            building_symbols: false,
            reload: false,
            symbols_dirty_since: None,
        }
    }
}

/// 背景の 1 回で済ませた調査と読み込み。
///
/// `requested` は頼んだ時点のツリー。読んでいる間に worktree が動いていたら
/// [`SemanticIndex::accept`] が取り込みを拒む。
pub struct Load {
    pub requested: PathBuf,
    pub survey: Survey,
    pub store: Option<Store>,
}

/// `Store` は Debug を持たないので、件数だけ名乗る。
impl std::fmt::Debug for Load {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Load")
            .field("requested", &self.requested)
            .field("roots", &self.survey.roots.len())
            .field("documents", &self.store.as_ref().map(Store::len))
            .finish()
    }
}

/// 作り直しの引き金は索引ごとに違うので、両方へ伝える。
pub fn note_change(ws: &mut Workspace, path: &Path) {
    let tree = ws.panels.viewer.root().to_path_buf();
    ws.index.semantic.note_change(path, &tree);
    ws.index
        .symbols_dirty_since
        .get_or_insert_with(Instant::now);
}

/// 毎フレーム 1 周進める。
///
/// 開いているファイルを毎周告げるのは、ファイルを開く経路が 1 つに絞れないため。
/// 前と同じファイルなら core 側が即座に返す。
pub fn tick(ws: &mut Workspace) -> Vec<Effect> {
    let tree = ws.panels.viewer.root().to_path_buf();
    if tree.as_os_str().is_empty() {
        return Vec::new();
    }
    let repo = ws.repo.root.clone();
    let reading = ws.panels.viewer.active_path().map(PathBuf::from);

    let mut effects = build_symbols(ws, &tree);
    let wanted = ws.index.semantic.needs_survey(&tree);
    if !ws.index.surveying
        && let Some(wanted) = wanted.or_else(|| ws.index.reload.then(Vec::new))
    {
        ws.index.surveying = true;
        ws.index.reload = false;
        ws.index.semantic.invalidate_if_retargeted(&tree);
        effects.push(Effect::Spawn(Task::SurveyIndex {
            repo_root: repo.clone(),
            tree_root: tree.clone(),
            reading: reading.clone(),
            wanted,
        }));
    }

    if let Some(rel) = &reading {
        let answer = ws.index.semantic.note_open(rel, &repo, &tree);
        let building = answer == Reading::Building;
        let viewer = Target::Region(Region::Viewer);
        if building != ws.fx.is_playing(&Kind::Busy, viewer) {
            if building {
                ws.fx.play(Kind::Busy, viewer);
            } else {
                ws.fx.stop(&Kind::Busy, viewer);
                ws.fx.play(Kind::Flash, viewer);
            }
        }
        // 索引がこのファイルを説明できないと黙って構文層に落ちる。言わないと
        // 「ジャンプが甘い」としか見えないので、開いたときに 1 度だけ出す。
        if answer == Reading::Stale {
            effects.push(Effect::Status(
                StatusLevel::Warning,
                "Code index does not cover this file \u{2014} Repo \u{25b8} Rebuild Code Index"
                    .into(),
            ));
        }
    }

    effects.extend(finish_regeneration(ws, &repo, &tree));
    effects
}

/// 静穏が明けていれば tree-sitter の索引を作り直させる。
///
/// 走っているビルドは置き換えない。worktree の一覧はスクロールできる速さで動くので、
/// 置き換えると 10 個通過するだけで 10 本のツリー走査が並ぶ。
fn build_symbols(ws: &mut Workspace, tree: &Path) -> Vec<Effect> {
    ws.index.symbols.set_root(tree.to_path_buf());
    if ws.index.building_symbols {
        return Vec::new();
    }
    let quiet = ws
        .index
        .symbols_dirty_since
        .is_some_and(|at| at.elapsed() >= SYMBOLS_QUIET);
    if !quiet && ws.index.symbols.is_available() {
        return Vec::new();
    }
    ws.index.symbols_dirty_since = None;
    ws.index.building_symbols = true;
    vec![Effect::Spawn(Task::BuildSymbols(ws.index.symbols.clone()))]
}

fn finish_regeneration(ws: &mut Workspace, repo: &Path, tree: &Path) -> Vec<Effect> {
    let Some(finished) = ws.index.semantic.tick_regeneration(repo, tree) else {
        return Vec::new();
    };
    let manual = finished.manual;
    match finished.outcome {
        // 1 世代が作るのは索引ルート 1 本ぶんで、画面が引くのは全ルートを畳んだもの。
        // 受け取らずに読み直すのは、そのままでは他のルートの索引が黙って落ちるため。
        Regenerated::Ready { documents } => {
            log::info!("semantic index regenerated: {documents} documents");
            let first = ws.index.semantic.store(tree).is_none();
            ws.index.reload = true;
            // 作り直しは編集が収まるたびに走る。毎回出すとステータスがそれで埋まる。
            if manual || first {
                let unit = if documents == 1 { "file" } else { "files" };
                vec![Effect::Status(
                    StatusLevel::Success,
                    format!("Code index ready ({documents} {unit})"),
                )]
            } else {
                Vec::new()
            }
        }
        // 待機に戻すのは sheaf 側。ここで頼むと二重に走る。
        Regenerated::Busy => Vec::new(),
        Regenerated::Failed(why) => {
            log::warn!("semantic index regeneration failed: {why}");
            if manual {
                vec![Effect::Status(
                    StatusLevel::Error,
                    format!("Could not rebuild the code index: {why}"),
                )]
            } else {
                Vec::new()
            }
        }
        Regenerated::Unavailable(why) => {
            log::info!("semantic index disabled: {why}");
            Vec::new()
        }
    }
}

/// 背景の調査と読み込みを取り込む。
///
/// 取り込めなかったときに読み直しを頼まないのは、[`SemanticIndex::needs_survey`] が
/// 次の周に同じことを言うため。
pub fn accept_load(ws: &mut Workspace, load: Load) {
    ws.index.surveying = false;
    let current = ws.panels.viewer.root().to_path_buf();
    ws.index.semantic.install(load.survey, &current);
    let documents = load.store.as_ref().map(Store::len);
    if ws
        .index
        .semantic
        .accept(&load.requested, &current, load.store)
    {
        log::info!("semantic index loaded: {documents:?} documents");
    }
}

pub fn accept_symbols(ws: &mut Workspace, count: usize) {
    ws.index.building_symbols = false;
    log::info!("symbol index built: {count} symbols");
}

/// svc から届いた結果のうち、索引のものを取り込む。
pub fn accept(ws: &mut Workspace, result: TaskResult) {
    match result {
        TaskResult::IndexLoaded(load) => accept_load(ws, *load),
        TaskResult::SymbolsBuilt(count) => accept_symbols(ws, count),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;
    use crate::workspace::Workspace;

    fn tree(name: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::with_prefix(name).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        dir
    }

    /// `viewer` の根がそのまま索引の向き先になる。
    fn workspace_at(root: &Path) -> Workspace {
        let mut ws = Workspace::for_test();
        ws.repo.root = root.to_path_buf();
        ws.panels.viewer.set_root(root.to_path_buf());
        ws
    }

    fn surveys(effects: &[Effect]) -> usize {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Spawn(Task::SurveyIndex { .. })))
            .count()
    }

    /// 飛んでいる間にもう 1 本頼むと、フレームごとにワーカーが積み上がる。
    #[test]
    fn 調査は同時に1本しか頼まない() {
        let dir = tree("survey-once");
        let mut ws = workspace_at(dir.path());

        assert_eq!(surveys(&tick(&mut ws)), 1);
        assert_eq!(surveys(&tick(&mut ws)), 0, "飛んでいる間に 2 本目を頼んだ");

        let survey = conductor_core::semantic_index::survey(dir.path(), None, None, &[]);
        accept_load(
            &mut ws,
            Load {
                requested: dir.path().to_path_buf(),
                survey,
                store: None,
            },
        );
        assert_eq!(surveys(&tick(&mut ws)), 0, "取り込んだのに調べ直した");
    }

    /// 調べている間に worktree が動いていたら、その結果は前のツリーのもの。
    /// 取り込むと、いま見ていないツリーの鍵で生成を始めることになる。
    #[test]
    fn 別のツリーの調査結果は取り込まず調べ直す() {
        let (before, after) = (tree("left"), tree("right"));
        let mut ws = workspace_at(after.path());
        tick(&mut ws);

        let stale = conductor_core::semantic_index::survey(before.path(), None, None, &[]);
        accept_load(
            &mut ws,
            Load {
                requested: before.path().to_path_buf(),
                survey: stale,
                store: None,
            },
        );

        assert!(
            ws.index.semantic.needs_survey(after.path()).is_some(),
            "前のツリーの調査を今のツリーのものとして取り込んだ"
        );
        assert_eq!(surveys(&tick(&mut ws)), 1, "調べ直しに行かない");
    }

    #[test]
    fn 根が無いうちは索引を動かさない() {
        let mut ws = Workspace::for_test();
        ws.panels.viewer.set_root(PathBuf::new());
        assert!(tick(&mut ws).is_empty());
    }

    #[test]
    fn 変更のあとは静穏を待ってから作り直す() {
        let dir = tree("quiet");
        let mut ws = workspace_at(dir.path());
        let builds = |effects: &[Effect]| {
            effects
                .iter()
                .filter(|e| matches!(e, Effect::Spawn(Task::BuildSymbols(_))))
                .count()
        };

        // 最初の 1 本は索引がまだ無いので静穏を待たない。
        assert_eq!(builds(&tick(&mut ws)), 1);
        accept_symbols(&mut ws, 1);
        ws.index.symbols.build();

        note_change(&mut ws, &dir.path().join("src/lib.rs"));
        assert_eq!(builds(&tick(&mut ws)), 0, "静穏を待たずに走った");

        ws.index.symbols_dirty_since = Some(Instant::now() - SYMBOLS_QUIET);
        assert_eq!(builds(&tick(&mut ws)), 1);
    }
}
