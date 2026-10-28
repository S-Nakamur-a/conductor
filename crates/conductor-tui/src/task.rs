//! svc に投げる仕事と、その結果の語彙。svc は中身を知らない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use conductor_core::cc_hook::statusline;
use conductor_core::claude_log::{self, LogEntry};
use conductor_core::claude_sessions::{ClaudeHome, ResumableSession};
use conductor_core::config::{self, ApiConfig, LayoutConfig};
use conductor_core::diff_state::{DiffSource, DiffState};
use conductor_core::git_engine::status_map::GitStatusMap;
use conductor_core::git_engine::{
    ChangeCounts, CommitInfo, GitEngine, GrabState, WorktreeInfo, conductor_dir,
};
use conductor_core::grep_search::{self, GrepMatch};
use conductor_core::pr_intake::{self, IntakenPr};
use conductor_core::review_publish::{self, PublishComment, PublishOutcome, PublishRequest};
use conductor_core::review_store::{
    Author, CommentKind, CommentStatus, NewReview, ReviewStore, SessionHistory,
};
use conductor_core::semantic_index::{self, IndexRoot};
use conductor_core::smart_worktree;
use conductor_core::update_checker::{self, UpdateInfo};
use conductor_svc::Services;

use crate::panels::explorer::tree;
use crate::panels::viewer::{content, media};
use crate::review::Snapshot;

const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Task が git を触るのに要るもの。Workspace が持っているので、Task 自身は運ばない。
#[derive(Debug, Clone)]
pub struct TaskEnv {
    pub root: PathBuf,
    pub version: &'static str,
    pub main_branch: String,
    pub worktree_dir: Option<PathBuf>,
    pub word_diff: bool,
    pub tab_width: usize,
    /// レビュー DB の行を引くキー。
    pub branch: String,
    /// 読む Claude Code の config dir ([Config::claude_dirs](conductor_core::config::Config::claude_dirs))。
    pub claude_dirs: Vec<PathBuf>,
}

impl TaskEnv {
    fn claude_home(&self) -> ClaudeHome {
        ClaudeHome::of(self.claude_dirs.clone())
    }
}

#[derive(Debug)]
pub enum Task {
    ListWorktrees,
    /// マシン上の Claude の状態と、会話ログの題。`known` は前に読んだ会話ログの mtime で、
    /// 変わっていないものは読み直さない。`own` は状態ファイルが無くても題を読みたいこの窓の分。
    /// `dirs` はこの窓のセッションの dir、`roots` はセッションが無くても木を引く repo。
    FleetStatus {
        known: std::collections::HashMap<String, std::time::SystemTime>,
        own: Vec<(PathBuf, String)>,
        dirs: Vec<PathBuf>,
        roots: Vec<PathBuf>,
    },
    /// この窓の外の Claude を止める。`started` は一覧に載せたときの起動時刻、`sessions_dir` は
    /// その写しを読んだ dir。
    StopSession {
        pid: u32,
        started: std::time::SystemTime,
        sessions_dir: PathBuf,
    },
    /// 1 つの worktree の変更件数。一覧とは別に走らせる (理由は WorktreeInfo::counts)。
    WorktreeCounts {
        path: PathBuf,
    },
    CreateWorktree {
        branch: String,
    },
    /// 自由記述のタスクから worktree を組み立てる。ブランチ名とセッション名は
    /// AI が決め、Claude へは説明文をそのまま流す。`repo` は作る先の repo。
    SmartWorktree {
        description: String,
        api: ApiConfig,
        repo: PathBuf,
    },
    DeleteWorktree {
        path: PathBuf,
        branch: String,
    },
    /// 一覧から消す worktree を調べる。`own` はこの窓のセッション、`window_root` は窓の repo。
    InspectFleetWorktree {
        repo: PathBuf,
        path: PathBuf,
        label: String,
        own: Vec<crate::panels::fleet::OwnPty>,
        window_root: PathBuf,
    },
    /// 中の Claude が終わったのを確かめ、点検のときから増えていなければ消す。
    DeleteFleetWorktree(Box<crate::panels::fleet::Inspection>),
    /// 止めた Claude が終わるのを待つ。開き直すのは結果を受けた側。
    AwaitReopen(crate::panels::fleet::Reopen),
    LoadTree {
        root: PathBuf,
        expanded: Vec<String>,
    },
    /// ファイル名検索が引く一覧。
    LoadFileIndex {
        root: PathBuf,
    },
    /// ツリーの色付けに使う全体の git status。
    LoadGitStatus {
        root: PathBuf,
    },
    ComputeDiff {
        worktree: PathBuf,
        source: DiffSource,
    },
    /// コミット一覧で目を通しているコミットの中身。開くのではなく見せるだけなので、
    /// 結果は Git Changes を差し替えず preview として持つ。
    CommitOverview {
        worktree: PathBuf,
        source: DiffSource,
    },
    HeadLog {
        worktree: PathBuf,
        skip: usize,
        limit: usize,
    },
    /// `seq` は結果の照合に使う。同じファイルを続けて開いても古い結果が新しい本文を
    /// 上書きしない。
    LoadFile {
        root: PathBuf,
        path: String,
        seq: u64,
        diff_of: Option<DiffSource>,
    },
    /// 画像を升目に描く。鍵は (パス, 桁, 行) で、区画の大きさが変われば描き直す。
    RenderMedia {
        root: PathBuf,
        path: String,
        cols: u16,
        rows: u16,
    },
    LoadReview,
    /// 書いたあとは必ず読み直して返す。
    WriteReview(ReviewWrite),
    /// `seq` は結果の照合に使う。打鍵の途中で追い越された検索を捨てる。
    Grep {
        root: PathBuf,
        query: String,
        regex: bool,
        case_sensitive: bool,
        seq: u64,
        cancel: Arc<AtomicBool>,
    },
    /// パネルが pin している Claude セッションの .jsonl。数 MB になるので UI では読まない。
    ReadTranscript {
        working_dir: PathBuf,
        session_id: String,
    },
    /// resume できる Claude セッション。`all` ならこのリポジトリの外も含める。
    ListSessions {
        all: bool,
    },
    /// 起動時の復帰のために、worktree ごとの最新セッションを探す。
    FindResumable {
        paths: Vec<PathBuf>,
    },
    /// 保存済みのターミナル出力。query が空なら新しい順に一覧する。
    ListHistory {
        query: String,
    },
    PersistConfig(Persist),
    /// ターミナル出力を保存し、保存後の一覧を返す。
    SaveHistory {
        session_id: String,
        worktree: String,
        label: String,
        kind: &'static str,
        output: String,
    },

    /// origin のブランチ。`fetch` なら先に取ってくる。
    ListRemoteBranches {
        fetch: bool,
    },
    CreateWorktreeFromRemote {
        remote_branch: String,
    },
    /// wt-grab を読む。
    LoadGrabState,
    Grab {
        branch: String,
        source: PathBuf,
    },
    Ungrab,
    ListStaleWorktrees,
    PruneWorktrees {
        names: Vec<String>,
    },
    ListBranchCommits {
        branch: String,
    },
    CherryPick {
        worktree: PathBuf,
        commit: String,
    },
    PullWorktree {
        worktree: PathBuf,
    },
    MergeToMain {
        branch: String,
    },
    ResetMainToOrigin,
    OpenPullRequest {
        branch: String,
    },
    /// 窓の repo ではなく dir の repo を見る。一覧には他の repo の行も並ぶ。
    OpenBranchPage {
        dir: PathBuf,
    },
    /// 一覧で選んだ行のブランチの変更。base は窓の repo ではなく dir の repo の既定ブランチ。
    FleetChanges {
        dir: PathBuf,
    },
    /// PR 番号か URL を worktree にする。取り込めたら PR のメタデータも書く。
    IntakePr {
        input: String,
    },
    /// 設定したリポジトリの、自分にレビューが依頼された PR を 1 件 worktree にする。
    /// `window_root` は取り込み先が窓のリポジトリかを見分けるためだけに使う。
    ReviewRequests {
        repos: Vec<PathBuf>,
        tried: std::collections::HashSet<(PathBuf, u64)>,
        window_root: PathBuf,
        worktree_dir: Option<PathBuf>,
    },
    /// 未公開コメントと投稿先。
    /// 差分の外のコメントを落とすので、いまの worktree の差分をここで取る。
    /// Explorer が見ている差分は出どころを替えられるので使えない。
    LoadPublishable {
        worktree: PathBuf,
    },
    Publish(Box<Publishable>),

    /// GitHub の最新リリース。`max_age` 以内のキャッシュがあればそれで済ませる。
    CheckForUpdate {
        max_age: Duration,
        /// 結果がどうであれステータスに出すか。起動時のチェックは黙って済ませる。
        announce: bool,
    },
    /// ビルド済みバイナリを取ってきて入れ替える。途中経過を何度も返す。
    DownloadUpdate(Box<UpdateInfo>),

    /// 索引ルートを調べ直し、置いてある索引を読む。どちらもツリーを歩くので背景で。
    ///
    /// `reading` を渡すのは、そのファイルの索引ルートだけは索引が無くても鍵を
    /// 出しておくため。渡さないと、まだ索引の無いルートは生成が始まらない。
    SurveyIndex {
        repo_root: PathBuf,
        tree_root: PathBuf,
        reading: Option<PathBuf>,
        /// 鍵を失ったルート。名指ししないと調査に選ばれず、毎周調査を頼み続ける。
        wanted: Vec<IndexRoot>,
    },
}

/// Smart Worktree が作ったものと、そこで起こす Claude に渡すもの。
#[derive(Debug)]
pub struct SmartWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub prompt: String,
    pub session_name: Option<String>,
}

/// 公開の確認と実行に要るもの。
#[derive(Debug, Clone)]
pub struct Publishable {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comments: Vec<PublishComment>,
    /// 差分の外にあって落としたコメントの数。
    pub skipped: usize,
}

/// 走っているものと比べた結果。届かなかったことを「最新だった」と混ぜない。
#[derive(Debug)]
pub enum UpdateCheck {
    Newer(Box<UpdateInfo>),
    UpToDate,
    Unreachable,
}

/// 自己更新の進み具合。
#[derive(Debug)]
pub enum UpdateStage {
    Step(update_checker::Progress),
    /// 入れ替えが済んだ。あとは再起動するだけ。
    Installed,
    Failed(String),
}

/// grab / ungrab が返すもの。
#[derive(Debug)]
pub struct GrabDone {
    pub message: String,
    pub state: Option<GrabState>,
    /// 移行した Claude セッションと、それを開き直す worktree と、会話を走らせた config dir。
    pub resume: Option<(String, PathBuf, PathBuf)>,
}

/// 設定ファイルに書き戻す 1 項目。
#[derive(Debug)]
pub enum Persist {
    Theme(String),
    HighContrast(bool),
    Layout(Box<LayoutConfig>),
}

/// レビュー DB への 1 回の書き込み。
#[derive(Debug)]
pub enum ReviewWrite {
    AddComment {
        file_path: String,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: String,
    },
    EditComment {
        id: String,
        body: String,
    },
    DeleteComment {
        id: String,
    },
    SetStatus {
        id: String,
        status: CommentStatus,
    },
    AddReply {
        comment_id: String,
        body: String,
    },
    EditReply {
        id: String,
        body: String,
    },
    DeleteReply {
        id: String,
    },
    SetViewed {
        file_path: String,
        viewed: bool,
    },
}

#[derive(Debug)]
pub enum TaskResult {
    Worktrees(Result<Vec<WorktreeInfo>, String>),
    FleetStatus(Box<crate::panels::fleet::FleetStatus>),
    SessionStopped(conductor_core::claude_sessions::registry::Stopped),
    BranchPageOpened(Result<String, String>),
    FleetChanges {
        dir: PathBuf,
        diff: Box<DiffState>,
        status: GitStatusMap,
    },
    /// 数え終わった worktree と、その件数。数えられなければ None のままにする。
    WorktreeCounts {
        path: PathBuf,
        counts: Option<ChangeCounts>,
    },
    WorktreeCreated(Result<Created, String>),
    SmartWorktreeCreated(Result<SmartWorktree, String>),
    WorktreeDeleted(Result<String, String>),
    /// 開き直す Claude が止まったか。止まっていれば選んだアカウントで開き直す。
    Reopened {
        reopen: crate::panels::fleet::Reopen,
        ended: bool,
    },
    FleetWorktreeInspected {
        path: PathBuf,
        outcome: crate::panels::fleet::Inspected,
    },
    /// `in_window_repo` なら worktree パネルの一覧も読み直す。
    FleetWorktreeDeleted {
        path: PathBuf,
        in_window_repo: bool,
        outcome: Result<String, String>,
    },
    Tree(Box<tree::Snapshot>),
    /// `root` は照合用。走査中に worktree を切り替えていたら捨てる。
    FileIndex {
        root: PathBuf,
        files: Vec<String>,
    },
    /// 読めなければ None。空のマップを入れるとツリーが全て Tracked に見え、
    /// 「変更なし」を黙って主張する。
    GitStatus {
        root: PathBuf,
        status: Option<Box<GitStatusMap>>,
    },
    /// DiffState は失敗の理由を自分の中に持つので Result にしない。
    Diff {
        root: PathBuf,
        diff: Box<DiffState>,
    },
    /// どのコミットの結果かは DiffState の source が持つ。
    CommitOverview(Box<DiffState>),
    /// `skip` は log 側の照合用にそのまま返す。
    HeadLog {
        skip: usize,
        commits: Result<Vec<CommitInfo>, String>,
    },
    FileLoaded {
        seq: u64,
        loaded: Result<content::Loaded, String>,
    },
    MediaRendered {
        key: media::Key,
        rendered: Result<Box<media::Rendered>, String>,
    },
    Review(Result<Box<Snapshot>, String>),
    Grep {
        seq: u64,
        found: Result<Vec<GrepMatch>, String>,
    },
    Sessions(Result<Vec<ResumableSession>, String>),
    /// worktree ごとの最新セッション (キーは canonicalize したパス) と、main に
    /// 使う grab 済みのセッション id。
    Resumable {
        sessions: HashMap<PathBuf, ResumableSession>,
        main_grabbed: Option<(String, PathBuf)>,
    },
    /// `session_id` は照合用。読んでいる間に /clear でローテーションしていたら捨てる。
    Transcript {
        session_id: String,
        entries: Result<Vec<LogEntry>, String>,
    },
    Persisted(Result<(), String>),
    History {
        /// 直前に保存したかどうか。ステータスに出す。
        saved: bool,
        records: Result<Vec<SessionHistory>, String>,
    },

    /// git 操作 1 回。成功したときの文言はそのまま Status に出す。どれも
    /// worktree の姿を変えうるので、受け取った側は一覧を取り直す。
    GitDone(Result<String, String>),
    /// wt-grab の今の中身。
    GrabState(Result<Option<GrabState>, String>),
    Grab(Result<Box<GrabDone>, String>),
    RemoteBranches(Result<Vec<String>, String>),
    StaleWorktrees(Result<Vec<String>, String>),
    Commits(Result<Vec<CommitInfo>, String>),
    PrIntake(Result<Box<IntakenPr>, String>),
    ReviewRound {
        round: Box<pr_intake::ReviewRound>,
        in_window_repo: bool,
    },
    Publishable(Result<Box<Publishable>, String>),
    Published(PublishOutcome),
    UpdateCheck {
        outcome: UpdateCheck,
        announce: bool,
    },
    UpdateProgress(UpdateStage),
    IndexLoaded(Box<crate::index::Load>),
}

impl Task {
    pub fn spawn(self, svc: &mut Services<TaskResult>, env: &TaskEnv) {
        let env = env.clone();
        match self {
            Task::ListWorktrees => {
                svc.spawn(move || list_worktrees(&env), TaskResult::Worktrees);
            }
            Task::FleetStatus {
                known,
                own,
                dirs,
                roots,
            } => {
                svc.spawn(
                    move || Box::new(fleet_status(&known, own, dirs, &roots, &env.claude_dirs)),
                    TaskResult::FleetStatus,
                );
            }
            Task::StopSession {
                pid,
                started,
                sessions_dir,
            } => {
                use conductor_core::claude_sessions::registry;
                svc.spawn(
                    move || {
                        registry::stop(
                            &sessions_dir,
                            pid,
                            started,
                            &registry::SystemProcesses,
                            STOP_GRACE,
                        )
                    },
                    TaskResult::SessionStopped,
                );
            }
            Task::WorktreeCounts { path } => {
                svc.spawn(
                    move || {
                        let counts = GitEngine::change_counts_at(&path)
                            .inspect_err(|e| {
                                log::warn!("could not count changes in {}: {e:#}", path.display());
                            })
                            .ok();
                        (path, counts)
                    },
                    |(path, counts)| TaskResult::WorktreeCounts { path, counts },
                );
            }
            Task::CreateWorktree { branch } => {
                svc.spawn(
                    move || create_worktree(&env, &branch),
                    TaskResult::WorktreeCreated,
                );
            }
            Task::SmartWorktree {
                description,
                api,
                repo,
            } => {
                svc.spawn_unscoped(
                    move || smart_worktree(&env, &repo, &description, &api),
                    TaskResult::SmartWorktreeCreated,
                );
            }
            Task::DeleteWorktree { path, branch } => {
                svc.spawn(
                    move || delete_worktree(&env, &path, &branch),
                    TaskResult::WorktreeDeleted,
                );
            }
            Task::InspectFleetWorktree {
                repo,
                path,
                label,
                own,
                window_root,
            } => {
                svc.spawn(
                    move || {
                        use conductor_core::claude_sessions::registry;
                        let live = registry::read_every(
                            &registry::sessions_dirs(&env.claude_dirs),
                            &registry::SystemProcesses,
                        );
                        let outcome =
                            fleet_delete::inspect(&repo, &path, label, &own, &window_root, &live);
                        (path, outcome)
                    },
                    |(path, outcome)| TaskResult::FleetWorktreeInspected { path, outcome },
                );
            }
            // repo を切り替えても開き直す。止めたのはもう済んでいる。
            Task::AwaitReopen(reopen) => {
                svc.spawn_unscoped(
                    move || {
                        use conductor_core::claude_sessions::registry;
                        let ended = registry::ended_within(
                            reopen.pid,
                            reopen.started,
                            &registry::SystemProcesses,
                            fleet_delete::OWN_WAIT,
                        );
                        (reopen, ended)
                    },
                    |(reopen, ended)| TaskResult::Reopened { reopen, ended },
                );
            }
            // repo を切り替えても、消したかどうかは知らせる。
            Task::DeleteFleetWorktree(inspection) => {
                svc.spawn_unscoped(
                    move || {
                        let in_window_repo = inspection.in_window_repo;
                        let path = inspection.path.clone();
                        use conductor_core::claude_sessions::registry;
                        let outcome = fleet_delete::run(
                            &inspection,
                            &registry::sessions_dirs(&env.claude_dirs),
                            &registry::SystemProcesses,
                            STOP_GRACE,
                        );
                        (path, in_window_repo, outcome)
                    },
                    |(path, in_window_repo, outcome)| TaskResult::FleetWorktreeDeleted {
                        path,
                        in_window_repo,
                        outcome,
                    },
                );
            }
            Task::LoadTree { root, expanded } => {
                svc.spawn(
                    move || Box::new(tree::survey(&root, &expanded)),
                    TaskResult::Tree,
                );
            }
            Task::LoadFileIndex { root } => {
                svc.spawn(
                    move || {
                        let files = conductor_core::file_index::list(&root);
                        (root, files)
                    },
                    |(root, files)| TaskResult::FileIndex { root, files },
                );
            }
            Task::LoadGitStatus { root } => {
                svc.spawn(
                    move || {
                        // git 管理外のディレクトリでは正当にここで失敗する。
                        let status = GitStatusMap::load(&root)
                            .inspect_err(|e| log::warn!("git status unavailable: {e}"))
                            .ok();
                        (root, status)
                    },
                    |(root, status)| TaskResult::GitStatus {
                        root,
                        status: status.map(Box::new),
                    },
                );
            }
            Task::ComputeDiff { worktree, source } => {
                svc.spawn(
                    move || {
                        let mut diff = DiffState::new(source);
                        diff.load(&worktree, env.word_diff, env.tab_width);
                        (worktree, Box::new(diff))
                    },
                    |(root, diff)| TaskResult::Diff { root, diff },
                );
            }
            Task::CommitOverview { worktree, source } => {
                svc.spawn(
                    move || {
                        let mut diff = DiffState::new(source);
                        diff.load(&worktree, env.word_diff, env.tab_width);
                        Box::new(diff)
                    },
                    TaskResult::CommitOverview,
                );
            }
            Task::HeadLog {
                worktree,
                skip,
                limit,
            } => {
                svc.spawn(
                    move || {
                        let commits = GitEngine::open(&worktree)
                            .and_then(|git| git.head_log(skip, limit))
                            .map_err(|e| format!("{e:#}"));
                        (skip, commits)
                    },
                    |(skip, commits)| TaskResult::HeadLog { skip, commits },
                );
            }
            Task::ReadTranscript {
                working_dir,
                session_id,
            } => {
                svc.spawn(
                    move || {
                        let entries = read_transcript(&env, &working_dir, &session_id);
                        (session_id, entries)
                    },
                    |(session_id, entries)| TaskResult::Transcript {
                        session_id,
                        entries,
                    },
                );
            }
            Task::LoadFile {
                root,
                path,
                seq,
                diff_of,
            } => {
                svc.spawn(
                    move || {
                        let loaded = match diff_of {
                            Some(source) => {
                                content::read_new_side(&root, &path, &source, env.tab_width)
                            }
                            None => content::read(&root, &path, env.tab_width),
                        };
                        (seq, loaded)
                    },
                    |(seq, loaded)| TaskResult::FileLoaded { seq, loaded },
                );
            }
            Task::RenderMedia {
                root,
                path,
                cols,
                rows,
            } => {
                svc.spawn(
                    move || {
                        let rendered = media::render(&root.join(&path), cols, rows);
                        TaskResult::MediaRendered {
                            key: (path, cols, rows),
                            rendered: rendered.map(Box::new),
                        }
                    },
                    |result| result,
                );
            }
            Task::LoadReview => {
                svc.spawn(move || load_review(&env), TaskResult::Review);
            }
            Task::WriteReview(write) => {
                svc.spawn(move || write_review(&env, &write), TaskResult::Review);
            }
            Task::Grep {
                root,
                query,
                regex,
                case_sensitive,
                seq,
                cancel,
            } => {
                svc.spawn(
                    move || (seq, grep(&root, &query, regex, case_sensitive, &cancel)),
                    |(seq, found)| TaskResult::Grep { seq, found },
                );
            }
            Task::PersistConfig(what) => {
                svc.spawn(move || persist(&what), TaskResult::Persisted);
            }
            Task::ListSessions { all } => {
                svc.spawn(move || list_sessions(&env, all), TaskResult::Sessions);
            }
            Task::FindResumable { paths } => {
                svc.spawn(move || find_resumable(&env, &paths), |result| result);
            }
            Task::ListHistory { query } => {
                svc.spawn(
                    move || TaskResult::History {
                        saved: false,
                        records: list_history(&env, &query),
                    },
                    |result| result,
                );
            }
            Task::SaveHistory {
                session_id,
                worktree,
                label,
                kind,
                output,
            } => {
                svc.spawn(
                    move || TaskResult::History {
                        saved: true,
                        records: save_history(&env, &session_id, &worktree, &label, kind, &output),
                    },
                    |result| result,
                );
            }

            Task::ListRemoteBranches { fetch } => {
                svc.spawn(
                    move || remote_branches(&env, fetch),
                    TaskResult::RemoteBranches,
                );
            }
            Task::CreateWorktreeFromRemote { remote_branch } => {
                svc.spawn(
                    move || create_from_remote(&env, &remote_branch),
                    TaskResult::WorktreeCreated,
                );
            }
            Task::LoadGrabState => {
                svc.spawn(move || load_grab_state(&env), TaskResult::GrabState);
            }
            Task::Grab { branch, source } => {
                svc.spawn(move || grab(&env, &branch, &source), TaskResult::Grab);
            }
            Task::Ungrab => {
                svc.spawn(move || ungrab(&env), TaskResult::Grab);
            }
            Task::ListStaleWorktrees => {
                svc.spawn(
                    move || git(&env, |git| git.find_stale_worktrees()),
                    TaskResult::StaleWorktrees,
                );
            }
            Task::PruneWorktrees { names } => {
                svc.spawn(move || prune(&env, &names), TaskResult::GitDone);
            }
            Task::ListBranchCommits { branch } => {
                svc.spawn(
                    move || {
                        git(&env, |git| {
                            git.list_branch_commits(&branch, CHERRY_PICK_COMMITS)
                        })
                    },
                    TaskResult::Commits,
                );
            }
            Task::CherryPick { worktree, commit } => {
                svc.spawn(
                    move || git(&env, |git| git.cherry_pick_to_worktree(&worktree, &commit)),
                    TaskResult::GitDone,
                );
            }
            Task::PullWorktree { worktree } => {
                svc.spawn(
                    move || git(&env, |git| git.pull_worktree(&worktree)),
                    TaskResult::GitDone,
                );
            }
            Task::MergeToMain { branch } => {
                svc.spawn(
                    move || git(&env, |git| git.merge_into_main(&branch, &env.main_branch)),
                    TaskResult::GitDone,
                );
            }
            Task::ResetMainToOrigin => {
                svc.spawn(
                    move || git(&env, |git| git.reset_main_to_origin(&env.main_branch)),
                    TaskResult::GitDone,
                );
            }
            Task::OpenPullRequest { branch } => {
                svc.spawn(
                    move || open_pull_request(&env, &branch),
                    TaskResult::GitDone,
                );
            }
            Task::OpenBranchPage { dir } => {
                svc.spawn(move || open_branch_page(&dir), TaskResult::BranchPageOpened);
            }
            Task::FleetChanges { dir } => {
                svc.spawn(
                    move || {
                        let (diff, status) = branch_changes(&dir, env.tab_width);
                        (dir, diff, status)
                    },
                    |(dir, diff, status)| TaskResult::FleetChanges { dir, diff, status },
                );
            }
            Task::IntakePr { input } => {
                svc.spawn(move || intake_pr(&env, &input), TaskResult::PrIntake);
            }
            // 結果が届かないと次の問い合わせが始まらないので、repo を切り替えても捨てない。
            Task::ReviewRequests {
                repos,
                tried,
                window_root,
                worktree_dir,
            } => {
                svc.spawn_unscoped(
                    move || review_round(&repos, &tried, &window_root, worktree_dir.as_deref()),
                    |(round, in_window_repo)| TaskResult::ReviewRound {
                        round: Box::new(round),
                        in_window_repo,
                    },
                );
            }
            Task::LoadPublishable { worktree } => {
                svc.spawn(
                    move || publishable(&env, &worktree),
                    TaskResult::Publishable,
                );
            }
            Task::Publish(request) => {
                svc.spawn(move || publish(&env, *request), TaskResult::Published);
            }

            Task::CheckForUpdate { max_age, announce } => {
                svc.spawn(
                    move || (check_update(max_age, env.version), announce),
                    |(outcome, announce)| TaskResult::UpdateCheck { outcome, announce },
                );
            }
            Task::SurveyIndex {
                repo_root,
                tree_root,
                reading,
                wanted,
            } => {
                svc.spawn(
                    move || {
                        let (survey, store) = semantic_index::survey_and_load(
                            &repo_root,
                            &tree_root,
                            reading.as_deref(),
                            &wanted,
                        );
                        Box::new(crate::index::Load {
                            requested: tree_root,
                            survey,
                            store,
                        })
                    },
                    TaskResult::IndexLoaded,
                );
            }
            // 段階ごとに報告するので spawn ではなく送信口を持って自前のスレッドで走る。
            Task::DownloadUpdate(info) => {
                let sender = svc.sender();
                std::thread::spawn(move || {
                    let stage = install_update(&info, &sender);
                    sender.send_task(TaskResult::UpdateProgress(stage));
                });
            }
        }
    }
}

fn check_update(max_age: Duration, version: &str) -> UpdateCheck {
    let Some(info) = update_checker::check(max_age) else {
        return UpdateCheck::Unreachable;
    };
    if update_checker::is_newer(&info.latest_version, version) {
        UpdateCheck::Newer(Box::new(info))
    } else {
        UpdateCheck::UpToDate
    }
}

fn install_update(
    info: &UpdateInfo,
    sender: &conductor_svc::EventSender<TaskResult>,
) -> UpdateStage {
    let Some(asset) = update_checker::find_binary_asset(&info.assets) else {
        return UpdateStage::Failed(format!(
            "No pre-built binary for this platform in v{}.",
            info.latest_version
        ));
    };
    let report = |step| sender.send_task(TaskResult::UpdateProgress(UpdateStage::Step(step)));
    match update_checker::install(asset, &info.latest_version, report) {
        Ok(()) => UpdateStage::Installed,
        Err(e) => UpdateStage::Failed(format!("{e:#}")),
    }
}

const CHERRY_PICK_COMMITS: usize = 20;

/// 失敗の綴りをここに集める。
fn git<T>(env: &TaskEnv, op: impl FnOnce(&GitEngine) -> anyhow::Result<T>) -> Result<T, String> {
    GitEngine::open(&env.root)
        .and_then(|git| op(&git))
        .map_err(|e| format!("{e:#}"))
}

fn remote_branches(env: &TaskEnv, fetch: bool) -> Result<Vec<String>, String> {
    git(env, |git| {
        if fetch {
            git.fetch_origin()?;
        }
        git.list_remote_branches()
    })
}

/// 作った worktree。ベースを最新にできなかったときはその理由を添え、Git Changes の
/// 差分が広がりうることを画面に出す。
#[derive(Debug)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    pub warning: Option<String>,
}

/// git の stderr は複数行で、ステータスバーは改行を捨てて 1 行に繋ぐ。lock の競合では
/// 1 行目が `From <url>` で理由は後ろの行にあるので、fatal: / error: の行を優先する。
fn reason_line(e: &anyhow::Error) -> String {
    let text = format!("{e:#}");
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    lines
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .unwrap_or(first)
        .to_string()
}

/// 一覧を開いたときの fetch とは競走になるので、作る直前にもう一度取る。ベースも取るのは
/// Git Changes の差分がそこから測られるため。ただしベースが取れなくても作成は止めない
/// — 上流で既定ブランチが改名されていても、目的のブランチは取り込める。
fn create_from_remote(env: &TaskEnv, remote_branch: &str) -> Result<Created, String> {
    let (remote, branch) = remote_branch
        .split_once('/')
        .unwrap_or(("origin", remote_branch));
    git(env, |git| {
        let with_base = remote == "origin";
        let wanted = [branch, env.main_branch.as_str()];
        let wanted = if with_base { &wanted[..] } else { &wanted[..1] };
        let warning = match git.fetch_branches(remote, wanted) {
            Ok(()) => None,
            Err(e) if with_base => {
                git.fetch_branches(remote, &[branch])?;
                Some(format!(
                    "base '{}' not fetched: {}",
                    env.main_branch,
                    reason_line(&e)
                ))
            }
            Err(e) => return Err(e),
        };
        let path = git.create_worktree_from_remote(remote_branch, env.worktree_dir.as_deref())?;
        Ok(Created {
            path,
            branch: branch.to_string(),
            warning,
        })
    })
}

/// wt-grab が指す worktree が消えていれば、その状態ごと片付ける。
fn load_grab_state(env: &TaskEnv) -> Result<Option<GrabState>, String> {
    git(env, |git| match git.load_grab_state()? {
        Some(state) if !state.source_worktree.exists() => {
            log::warn!(
                "stale wt-grab: source worktree '{}' no longer exists",
                state.source_worktree.display()
            );
            git.remove_grab_state()?;
            Ok(None)
        }
        state => Ok(state),
    })
}

fn grab(env: &TaskEnv, branch: &str, source: &Path) -> Result<Box<GrabDone>, String> {
    let main = git(env, |git| git.main_worktree_path())?;
    let session = env
        .claude_home()
        .find_latest_sessions_for_paths(std::slice::from_ref(&source.to_path_buf()))
        .ok()
        .and_then(|mut found| {
            found.remove(
                &source
                    .canonicalize()
                    .unwrap_or_else(|_| source.to_path_buf()),
            )
        });

    git(env, |git| {
        git.grab_branch(
            &main,
            source,
            branch,
            session.as_ref().map(|s| s.session_id.as_str()),
        )
    })?;

    // 移行に失敗しても grab そのものは済んでいる。セッションが main から
    // 見えないだけなので、報告はログに留めて先へ進む。
    let resume = session.filter(|session| {
        ClaudeHome::migrate_session(
            &session.config_dir,
            &session.session_id,
            source,
            &main,
            &session.display,
        )
        .unwrap_or_else(|e| {
            log::warn!("grab: session migration failed: {e:#}");
            false
        })
    });
    let message = match &resume {
        Some(session) => format!(
            "Grabbed '{branch}' + resumed session {}.",
            short(&session.session_id)
        ),
        None => format!("Grabbed '{branch}' \u{2014} main is now on this branch."),
    };
    let state = git(env, |git| git.load_grab_state())?;
    Ok(Box::new(GrabDone {
        message,
        state,
        resume: resume.map(|s| (s.session_id, main, s.config_dir)),
    }))
}

fn ungrab(env: &TaskEnv) -> Result<Box<GrabDone>, String> {
    let Some(grabbed) = git(env, |git| git.load_grab_state())? else {
        return Err("not grabbing any branch".into());
    };
    let main = git(env, |git| git.main_worktree_path())?;
    git(env, |git| {
        git.ungrab_branch(
            &main,
            &grabbed.source_worktree,
            &grabbed.branch,
            &env.main_branch,
        )
    })?;
    if let Some(id) = &grabbed.claude_session_id
        && let Err(e) = env
            .claude_home()
            .unmigrate_session(id, &grabbed.source_worktree, &main)
    {
        log::warn!("ungrab: session unmigration failed: {e:#}");
    }
    Ok(Box::new(GrabDone {
        message: format!("Ungrabbed '{}' \u{2014} main restored.", grabbed.branch),
        state: None,
        resume: None,
    }))
}

/// 1 つ消せなくても残りは消す。
fn prune(env: &TaskEnv, names: &[String]) -> Result<String, String> {
    let git = GitEngine::open(&env.root).map_err(|e| format!("{e:#}"))?;
    let mut pruned = 0;
    for name in names {
        match git.prune_stale_worktree(name) {
            Ok(()) => pruned += 1,
            Err(e) => log::warn!("failed to prune worktree '{name}': {e:#}"),
        }
    }
    Ok(format!("Pruned {pruned} stale worktree(s)."))
}

fn open_pull_request(env: &TaskEnv, branch: &str) -> Result<String, String> {
    let url = git(env, |git| Ok(git.pr_url_for_branch(branch)))?
        .ok_or("could not determine the remote URL")?;
    open::that(&url).map_err(|e| format!("failed to open the browser: {e}"))?;
    Ok(format!("Opened the pull request for '{branch}'"))
}

fn open_branch_page(dir: &Path) -> Result<String, String> {
    use crate::panels::fleet::branch_page;
    use conductor_core::git_engine::Head;
    if !dir.is_dir() {
        return Err(format!("{} no longer exists.", dir.display()));
    }
    let git = GitEngine::open(dir)
        .map_err(|_| format!("{} is not in a git repository.", dir.display()))?;
    let head = git.head();
    let pr = match &head {
        Head::Branch(branch) => Some(lookup_pr(dir, branch)),
        Head::Detached(_) | Head::Unborn(_) => None,
    };
    let (url, message) = branch_page(&head, pr, git.repo_web_url())?;
    open::that(&url).map_err(|e| format!("failed to open the browser: {e}"))?;
    Ok(message)
}

/// `gh pr view <branch>` は数字だけや `#` で始まる名前を PR 番号と読み、`--head` だけだと
/// 同じ名前の他人の fork の PR も拾う。
const OWN_PR_BY_HEAD: &str =
    r#"[.[] | select(.isCrossRepository | not)] | sort_by(.state != "OPEN") | .[0].url // empty"#;

fn lookup_pr(dir: &Path, branch: &str) -> crate::panels::fleet::PrLookup {
    let output = std::process::Command::new("gh")
        .current_dir(dir)
        .args([
            "pr", "list", "--head", branch, "--state", "all", "--limit", "30",
        ])
        .args([
            "--json",
            "url,isCrossRepository,state",
            "--jq",
            OWN_PR_BY_HEAD,
        ])
        .output();
    match output {
        Err(e) => crate::panels::fleet::PrLookup::Failed(format!("gh: {e}")),
        Ok(out) => pr_lookup_of(&out),
    }
}

fn pr_lookup_of(out: &std::process::Output) -> crate::panels::fleet::PrLookup {
    use crate::panels::fleet::PrLookup;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return PrLookup::Failed(stderr.lines().next().unwrap_or_default().trim().to_string());
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "" => PrLookup::Missing,
        url => PrLookup::Found(url.to_string()),
    }
}

fn intake_pr(env: &TaskEnv, input: &str) -> Result<Box<IntakenPr>, String> {
    intake_pr_into(&env.root, env.worktree_dir.as_deref(), input).map(Box::new)
}

/// PR のメタデータは取り込み先のリポジトリの DB に書く。窓のリポジトリとは限らない。
fn intake_pr_into(
    root: &Path,
    worktree_dir: Option<&Path>,
    input: &str,
) -> Result<IntakenPr, String> {
    let intaken = pr_intake::intake_pr(root, worktree_dir, input).map_err(|e| e.to_string())?;
    // 取り込みは済んでいるので、メタデータを書けなくても worktree は返す。
    if let Err(e) = save_pr_meta(root, intaken.pr_number, &intaken.meta) {
        log::warn!("could not record the PR metadata: {e}");
    }
    Ok(intaken)
}

fn review_round(
    repos: &[PathBuf],
    tried: &std::collections::HashSet<(PathBuf, u64)>,
    window_root: &Path,
    worktree_dir: Option<&Path>,
) -> (pr_intake::ReviewRound, bool) {
    let round = pr_intake::review_round(
        repos,
        tried,
        pr_intake::repo_worktrees,
        pr_intake::review_requests,
        |main, number| intake_pr_into(main, worktree_dir, &number.to_string()),
    );
    let window_main = GitEngine::open(window_root)
        .and_then(|git| git.main_worktree_path())
        .ok();
    let in_window_repo = round
        .intaken
        .as_ref()
        .is_some_and(|intake| window_main.as_ref() == Some(&intake.repo));
    (round, in_window_repo)
}

/// dir がサブディレクトリでも、それを含む worktree の変更を読む。失敗は diff の error に入れて必ず返す
/// — 返さないと投げ中が下りない。
fn branch_changes(dir: &Path, tab_width: usize) -> (Box<DiffState>, GitStatusMap) {
    let workdir = git2::Repository::discover(dir)
        .ok()
        .and_then(|repo| repo.workdir().map(Path::to_path_buf));
    let Some(workdir) = workdir else {
        let mut diff = DiffState::new(DiffSource::working_tree(""));
        diff.error = Some(format!("{} is not in a git working tree", dir.display()));
        return (Box::new(diff), GitStatusMap::default());
    };
    let base =
        GitEngine::open(&workdir).map_or_else(|_| "main".to_string(), |git| git.default_branch());
    let mut diff = DiffState::new(DiffSource::working_tree(&base));
    diff.load(&workdir, false, tab_width);
    let status = GitStatusMap::load(&workdir).unwrap_or_else(|e| {
        log::warn!("git status failed in {}: {e:#}", workdir.display());
        GitStatusMap::default()
    });
    (Box::new(diff), status)
}

fn fleet_status(
    known: &std::collections::HashMap<String, std::time::SystemTime>,
    own: Vec<(PathBuf, String)>,
    mut dirs: Vec<PathBuf>,
    roots: &[PathBuf],
    claude_dirs: &[PathBuf],
) -> crate::panels::fleet::FleetStatus {
    use conductor_core::claude_log::summary;
    use conductor_core::claude_sessions::registry;

    let live = registry::read_live(
        &registry::sessions_dirs(claude_dirs),
        &registry::SystemProcesses,
    );
    let home = ClaudeHome::of(claude_dirs.to_vec());
    let mut asked = std::collections::HashSet::new();
    let summaries = live
        .iter()
        .map(|l| (l.cwd.clone(), l.session_id.clone()))
        .chain(own)
        .filter(|(_, id)| asked.insert(id.clone()))
        .filter_map(|(cwd, id)| {
            let path = home.session_log(&cwd, &id)?;
            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            if known.get(&id) == Some(&mtime) {
                return None;
            }
            let summary = summary::summarize(&path)
                .inspect_err(|e| log::debug!("could not read {}: {e}", path.display()))
                .ok()?;
            Some((id, mtime, summary))
        })
        .collect();
    dirs.extend(
        live.iter()
            .filter(|l| l.owner != registry::Owner::Elsewhere)
            .map(|l| l.cwd.clone()),
    );
    let (trees, placement) = fleet_trees(&dirs, roots);
    crate::panels::fleet::FleetStatus {
        live,
        summaries,
        trees,
        placement,
        usage: statusline::read(&statusline::dir(), std::time::SystemTime::now()),
    }
}

/// dirs と roots が属する repo の worktree の木と、dirs それぞれを含む worktree。
/// パスは canonicalize してから比べる。git の記録する綴りと cwd の綴り (/var と /private/var) が違うため。
fn fleet_trees(
    dirs: &[PathBuf],
    roots: &[PathBuf],
) -> (
    Vec<crate::panels::fleet::RepoTree>,
    std::collections::HashMap<PathBuf, PathBuf>,
) {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let mut engines = std::collections::BTreeMap::new();
    for dir in dirs.iter().chain(roots) {
        let Ok(git) = GitEngine::open(dir) else {
            continue;
        };
        if let Ok(main) = git.main_worktree_path() {
            engines.entry(canonical(&main)).or_insert(git);
        }
    }
    let trees: Vec<crate::panels::fleet::RepoTree> = engines
        .into_iter()
        .filter_map(|(root, git)| {
            let mut worktrees = git
                .worktree_heads()
                .inspect_err(|e| log::debug!("could not list worktrees of {}: {e}", root.display()))
                .ok()?;
            for w in &mut worktrees {
                w.path = canonical(&w.path);
            }
            worktrees[1..].sort_by_key(|w| w.head.label());
            Some(crate::panels::fleet::RepoTree {
                name: root.file_name().map_or_else(
                    || root.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                ),
                root,
                worktrees,
            })
        })
        .collect();
    let placement = dirs
        .iter()
        .filter_map(|dir| {
            let dir_canonical = canonical(dir);
            let worktree = trees
                .iter()
                .flat_map(|t| &t.worktrees)
                .filter(|w| dir_canonical.starts_with(&w.path))
                .max_by_key(|w| w.path.components().count())?;
            Some((dir.clone(), worktree.path.clone()))
        })
        .collect();
    (trees, placement)
}

fn save_pr_meta(root: &Path, pr_number: u64, meta: &pr_intake::FetchedPr) -> Result<(), String> {
    let store = open_store_at(root)?;
    store
        .save_worktree_base_branch(&meta.branch, &meta.base_ref)
        .and_then(|()| store.save_pr_review_meta(&meta.branch, &meta.review_meta(pr_number)))
        .map_err(|e| e.to_string())
}

/// GitHub にはスレッド返信が無いので、返信は本文の末尾へ平坦化する。
fn body_with_replies(store: &ReviewStore, id: &str, body: &str) -> String {
    let replies = store.get_replies(id).unwrap_or_default();
    if replies.is_empty() {
        return body.to_string();
    }
    let mut out = format!("{body}\n\n---\n");
    for reply in replies {
        out.push_str(&format!("\n**{}:** {}\n", reply.author, reply.body));
    }
    out
}

fn publishable(env: &TaskEnv, worktree: &Path) -> Result<Box<Publishable>, String> {
    let store = open_store(env)?;
    let branch = env.branch.as_str();
    let meta = store
        .get_pr_review_meta(branch)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let (Some(pr_number), Some(pr_url)) = (meta.pr_number, meta.pr_url) else {
        return Err("comments can only be published on a branch opened via PR intake".into());
    };
    let (owner, repo) = review_publish::owner_repo_from_pr_url(&pr_url)
        .ok_or_else(|| format!("could not parse owner/repo from the PR URL: {pr_url}"))?;
    let comments = store
        .unpublished_reviews(branch)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| PublishComment {
            body: body_with_replies(&store, &c.id, &c.body),
            id: c.id,
            file_path: c.file_path,
            line_start: c.line_start,
            line_end: c.line_end,
        })
        .collect();
    let mut diff = DiffState::new(DiffSource::working_tree(&env.main_branch));
    diff.load(worktree, env.word_diff, env.tab_width);
    let (comments, skipped) = review_publish::filter_publishable(comments, &diff);
    Ok(Box::new(Publishable {
        owner,
        repo,
        pr_number: pr_number as u64,
        comments,
        skipped,
    }))
}

/// 投稿してから、実際に通ったものだけを published にする。ここで刻むので、
/// 途中で失敗しても次の確認には残りだけが出る。
fn publish(env: &TaskEnv, request: Publishable) -> PublishOutcome {
    let outcome = review_publish::publish(PublishRequest {
        owner: request.owner,
        repo: request.repo,
        pr_number: request.pr_number,
        comments: request.comments,
    });
    let published = match &outcome {
        PublishOutcome::Succeeded { published_ids }
        | PublishOutcome::PartialFailure { published_ids, .. } => published_ids.as_slice(),
        PublishOutcome::Failed { .. } => &[],
    };
    if !published.is_empty()
        && let Err(e) = open_store(env).and_then(|store| {
            store
                .mark_published(published, &chrono::Utc::now().to_rfc3339())
                .map_err(|e| e.to_string())
        })
    {
        log::warn!("failed to mark comments published: {e}");
    }
    outcome
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

fn grep(
    root: &Path,
    query: &str,
    regex: bool,
    case_sensitive: bool,
    cancel: &AtomicBool,
) -> Result<Vec<GrepMatch>, String> {
    let re =
        grep_search::compile_pattern(query, regex, case_sensitive).map_err(|e| e.to_string())?;
    Ok(grep_search::search_tree(root, &re, cancel))
}

fn persist(what: &Persist) -> Result<(), String> {
    match what {
        Persist::Theme(name) => config::persist_ui_theme(name),
        Persist::HighContrast(on) => config::persist_ui_high_contrast(*on),
        Persist::Layout(layout) => config::persist_layout_proportions(layout),
    }
    .map_err(|e| e.to_string())
}

/// pin した session id のログだけを開く。兄弟の .jsonl は同じワークツリーで走った
/// 別の会話なので、mtime などで選び直すと他人の履歴を見せることになる。
fn read_transcript(
    env: &TaskEnv,
    working_dir: &Path,
    session_id: &str,
) -> Result<Vec<LogEntry>, String> {
    let path = env
        .claude_home()
        .session_log(working_dir, session_id)
        .ok_or_else(|| format!("no session log on disk for {session_id}"))?;
    let entries = claude_log::load_session(&path);
    crate::unknown_tool_log::record(working_dir, &entries);
    Ok(entries)
}

fn list_sessions(env: &TaskEnv, all: bool) -> Result<Vec<ResumableSession>, String> {
    env.claude_home()
        .load_resumable_sessions((!all).then_some(env.root.as_path()))
        .map_err(|e| e.to_string())
}

/// 復帰の判断に要るものを 1 回で揃える。grab 状態をここでも読むのは、起動時に
/// Task::LoadGrabState が先に着いている保証が無いため。どちらが欠けても
/// 残りは復帰できるので、失敗は警告に落として空で返す。
fn find_resumable(env: &TaskEnv, paths: &[PathBuf]) -> TaskResult {
    let home = env.claude_home();
    let sessions = home
        .find_latest_sessions_for_paths(paths)
        .inspect_err(|e| log::warn!("could not look for resumable sessions: {e:#}"))
        .unwrap_or_default();
    let main = git(env, |git| git.main_worktree_path()).ok();
    let main_grabbed = git(env, |git| git.load_grab_state())
        .unwrap_or_else(|e| {
            log::warn!("could not read the grab state: {e}");
            None
        })
        .and_then(|state| state.claude_session_id)
        .zip(main)
        .and_then(|(id, main)| {
            let ran_in = home
                .config_dir_of(&main, &id)
                .inspect_err(|e| log::warn!("could not look for the grabbed session: {e:#}"))
                .ok()??;
            Some((id, ran_in))
        });
    TaskResult::Resumable {
        sessions,
        main_grabbed,
    }
}

/// 一覧に出す上限。ここを超えると保存の古い順に落ちる。
const HISTORY_LIMIT: usize = 50;

fn list_history(env: &TaskEnv, query: &str) -> Result<Vec<SessionHistory>, String> {
    let store = open_store(env)?;
    if query.is_empty() {
        store.list_session_history(HISTORY_LIMIT)
    } else {
        store.search_session_history(query)
    }
    .map_err(|e| e.to_string())
}

fn save_history(
    env: &TaskEnv,
    session_id: &str,
    worktree: &str,
    label: &str,
    kind: &str,
    output: &str,
) -> Result<Vec<SessionHistory>, String> {
    let store = open_store(env)?;
    store
        .save_session_history(session_id, worktree, label, kind, output)
        .map_err(|e| e.to_string())?;
    drop(store);
    list_history(env, "")
}

fn open_store(env: &TaskEnv) -> Result<ReviewStore, String> {
    open_store_at(&env.root)
}

fn open_store_at(root: &Path) -> Result<ReviewStore, String> {
    let dir = conductor_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    ReviewStore::open(&dir.join("conductor.db")).map_err(|e| e.to_string())
}

fn load_review(env: &TaskEnv) -> Result<Box<Snapshot>, String> {
    let store = open_store(env)?;
    let branch = env.branch.as_str();
    let read = || -> anyhow::Result<Snapshot> {
        let mut comments = store.reviews_for_worktree(branch)?;
        comments.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then(a.line_start.cmp(&b.line_start))
        });
        Ok(Snapshot {
            branch: branch.to_string(),
            comments,
            replies: store.replies_for_worktree(branch)?,
            summary: store.get_change_summary(branch)?,
            viewed: store.viewed_files(branch)?,
            unpublished: store.unpublished_reviews(branch)?.len(),
        })
    };
    read().map(Box::new).map_err(|e| e.to_string())
}

fn write_review(env: &TaskEnv, write: &ReviewWrite) -> Result<Box<Snapshot>, String> {
    let store = open_store(env)?;
    let branch = env.branch.as_str();
    let applied = match write {
        ReviewWrite::AddComment {
            file_path,
            line_start,
            line_end,
            kind,
            body,
        } => store
            .add_review(NewReview {
                branch,
                file_path,
                line_start: *line_start,
                line_end: *line_end,
                kind: *kind,
                body,
                author: Author::User,
            })
            .map(|_| ()),
        ReviewWrite::EditComment { id, body } => store.update_review_body(id, body),
        ReviewWrite::DeleteComment { id } => store.delete_review(id),
        ReviewWrite::SetStatus { id, status } => store.update_review_status(id, *status),
        ReviewWrite::AddReply { comment_id, body } => {
            store.add_reply(comment_id, body, Author::User)
        }
        ReviewWrite::EditReply { id, body } => store.update_reply_body(id, body),
        ReviewWrite::DeleteReply { id } => store.delete_reply(id),
        ReviewWrite::SetViewed { file_path, viewed } => {
            store.set_viewed(branch, file_path, *viewed)
        }
    };
    applied.map_err(|e| e.to_string())?;
    drop(store);
    load_review(env)
}

fn list_worktrees(env: &TaskEnv) -> Result<Vec<WorktreeInfo>, String> {
    GitEngine::open(&env.root)
        .and_then(|git| git.list_worktrees())
        .map_err(|e| e.to_string())
}

fn create_worktree(env: &TaskEnv, branch: &str) -> Result<Created, String> {
    let git = GitEngine::open(&env.root).map_err(|e| e.to_string())?;
    let base = git.resolve_base_ref(&env.main_branch);
    // 分岐していれば触らないので、失敗しても作成そのものは続けられる。origin が無い
    // リポジトリでは追いつく先が無いので、取れなかった警告も出さない。
    let warning = git
        .has_origin()
        .then(|| git.ensure_base_ref_available(&env.main_branch).err())
        .flatten()
        .map(|e| {
            format!(
                "base '{}' not fast-forwarded: {}",
                env.main_branch,
                reason_line(&e)
            )
        });
    let path = git
        .create_worktree_from_base(branch, &base, env.worktree_dir.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(Created {
        path,
        branch: branch.to_string(),
        warning,
    })
}

fn smart_worktree(
    env: &TaskEnv,
    repo: &Path,
    description: &str,
    api: &ApiConfig,
) -> Result<SmartWorktree, String> {
    let plan = smart_worktree::generate(description, api, repo)?;
    let git = GitEngine::open(repo).map_err(|e| e.to_string())?;
    // リモートを持たないリポジトリで origin/main を掴んで作成ごと失敗しないよう、
    // 実在する ref へ解決してから渡す。
    let base = git.resolve_base_ref(&git.default_branch());
    let path = git
        .create_worktree_from_base(&plan.branch, &base, env.worktree_dir.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(SmartWorktree {
        path,
        branch: plan.branch,
        prompt: description.to_string(),
        session_name: plan.session_name,
    })
}

fn delete_worktree(env: &TaskEnv, path: &Path, branch: &str) -> Result<String, String> {
    let git = GitEngine::open(&env.root).map_err(|e| e.to_string())?;
    git.remove_worktree(path).map_err(|e| e.to_string())?;
    // ブランチが消せなくても worktree は消えている。報告するのは worktree の方。
    if let Err(e) = git.delete_branch(branch, true) {
        log::warn!("removed the worktree but could not delete branch '{branch}': {e:#}");
    }
    Ok(branch.to_string())
}

/// 一覧から worktree を消す。止める前に消せるかを確かめ、止めた後に数え直してから消す。
pub(crate) mod fleet_delete {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use conductor_core::claude_sessions::registry::{self, LiveSession, Processes, Stopped};
    use conductor_core::git_engine::{GitEngine, Refusal};
    use conductor_svc::pty::SessionKind;

    use crate::panels::fleet::{Doomed, Inspected, Inspection, OwnPty};

    /// この窓のタブを閉じてから、その Claude が終わるのを待つ上限。
    pub(crate) const OWN_WAIT: Duration = Duration::from_secs(5);

    fn canonical(p: &Path) -> PathBuf {
        p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
    }

    /// path は canonicalize 済み。中のセッションは cwd がその下にあるもの (どの窓のものでも)。
    pub fn inspect(
        repo: &Path,
        path: &Path,
        label: String,
        own: &[OwnPty],
        window_root: &Path,
        live: &[LiveSession],
    ) -> Inspected {
        if canonical(window_root).starts_with(path) {
            return Inspected::Refused(format!(
                "This window has {label} open; open another worktree before deleting it."
            ));
        }
        let inside = |dir: &Path| canonical(dir).starts_with(path);
        if own
            .iter()
            .any(|p| p.kind == SessionKind::Editor && inside(&p.working_dir))
        {
            return Inspected::Refused(format!("Close the editor in {label} first."));
        }
        let check = match GitEngine::open(repo).and_then(|git| git.deletion_check(path)) {
            Ok(Ok(check)) => check,
            Ok(Err(refusal)) => return Inspected::Refused(refusal_text(&refusal, &label)),
            Err(e) => return Inspected::Failed(format!("Could not check {label}: {e:#}")),
        };
        let in_window_repo = GitEngine::open(window_root)
            .and_then(|git| git.main_worktree_path())
            .is_ok_and(|main| canonical(&main) == repo);
        let claudes = live
            .iter()
            .filter(|l| inside(&l.cwd))
            .map(|l| Doomed {
                pid: l.pid,
                started: l.started,
                sessions_dir: l.sessions_dir.clone(),
                own: own.iter().any(|p| {
                    p.kind == SessionKind::ClaudeCode
                        && p.claude_session_id.as_deref() == Some(l.session_id.as_str())
                }),
            })
            .collect();
        let closing: Vec<&OwnPty> = own
            .iter()
            .filter(|p| p.kind != SessionKind::Editor && inside(&p.working_dir))
            .collect();
        Inspected::Ready(Box::new(Inspection {
            repo: repo.to_path_buf(),
            path: path.to_path_buf(),
            label,
            check,
            claudes,
            panels: closing.iter().map(|p| p.panel_id.clone()).collect(),
            own_claudes: closing
                .iter()
                .filter(|p| p.kind == SessionKind::ClaudeCode)
                .count(),
            shells: closing
                .iter()
                .filter(|p| p.kind == SessionKind::Shell)
                .count(),
            in_window_repo,
        }))
    }

    /// この窓のタブは呼ぶ前に閉じてある。
    pub fn run(
        inspection: &Inspection,
        sessions: &[PathBuf],
        procs: &dyn Processes,
        grace: Duration,
    ) -> Result<String, String> {
        let label = &inspection.label;
        for doomed in &inspection.claudes {
            let ended = if doomed.own {
                registry::ended_within(doomed.pid, doomed.started, procs, OWN_WAIT)
            } else {
                // 照合できないだけでも AlreadyGone が返るので、終わったかは別に見る。
                if let Stopped::Failed(why) = registry::stop(
                    &doomed.sessions_dir,
                    doomed.pid,
                    doomed.started,
                    procs,
                    grace,
                ) {
                    return Err(format!(
                        "Could not stop pid {}: {why}. {label} was not deleted.",
                        doomed.pid
                    ));
                }
                registry::ended_within(doomed.pid, doomed.started, procs, Duration::ZERO)
            };
            if !ended {
                return Err(format!(
                    "pid {} is still running. {label} was not deleted.",
                    doomed.pid
                ));
            }
        }
        let git = GitEngine::open(&inspection.repo).map_err(|e| format!("{e:#}"))?;
        let now = match git.deletion_check(&inspection.path) {
            Ok(Ok(check)) => check,
            Ok(Err(refusal)) => return Err(refusal_text(&refusal, label)),
            Err(e) => return Err(format!("Could not check {label} again: {e:#}")),
        };
        let before = &inspection.check;
        if now.head != before.head || now.branch != before.branch {
            return Err(format!(
                "{label} moved to another commit after the check. Nothing was deleted."
            ));
        }
        let grew = match (before.dirty, now.dirty) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(then), Some(now)) => now > then,
        };
        if grew {
            return Err(format!(
                "{label} has more uncommitted changes than the check showed. Nothing was deleted."
            ));
        }
        if registry::read_every(sessions, procs)
            .iter()
            .any(|l| canonical(&l.cwd).starts_with(&inspection.path))
        {
            return Err(format!(
                "A Claude started in {label} after the check. Nothing was deleted."
            ));
        }
        git.remove_worktree(&now.recorded)
            .map_err(|e| format!("{e:#}"))?;
        if let Some(branch) = now.branch.as_deref().filter(|_| !now.keeps_branch)
            && let Err(e) = git.delete_branch(branch, true)
        {
            return Err(format!(
                "Deleted {label}, but not its branch {branch}: {e:#}"
            ));
        }
        Ok(format!("Deleted {label}."))
    }

    fn refusal_text(refusal: &Refusal, label: &str) -> String {
        match refusal {
            Refusal::Main => "The main worktree cannot be deleted.".into(),
            Refusal::NotRegistered => format!("git does not list {label} as a worktree."),
            Refusal::Locked => format!("{label} is locked; run git worktree unlock first."),
            Refusal::Nested(inner) => format!(
                "{label} has another worktree inside it ({}).",
                inner.display()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn 通知に出す理由はfatalかerrorの行で_無ければ先頭行() {
        let locked =
            anyhow::anyhow!("From ../origin\nerror: cannot lock ref 'refs/remotes/origin/main'")
                .context("git fetch failed (exit 1)");
        assert_eq!(
            super::reason_line(&locked),
            "error: cannot lock ref 'refs/remotes/origin/main'"
        );

        let missing = anyhow::anyhow!("fatal: couldn't find remote ref refs/heads/gone")
            .context("git fetch failed (exit 128)");
        assert_eq!(
            super::reason_line(&missing),
            "git fetch failed (exit 128): fatal: couldn't find remote ref refs/heads/gone"
        );
    }

    use super::*;

    #[test]
    fn prのメタデータは取り込み先のリポジトリのdbに書く() {
        let window = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let meta = pr_intake::FetchedPr {
            branch: String::from("feature/a"),
            title: String::from("t"),
            base_ref: String::from("main"),
            head_ref: String::from("feature/a"),
            url: String::from("https://example.com/pull/7"),
            head_owner_login: None,
        };
        save_pr_meta(target.path(), 7, &meta).unwrap();
        let read = |root: &Path| {
            open_store_at(root)
                .unwrap()
                .get_pr_review_meta("feature/a")
                .unwrap()
        };
        assert_eq!(read(target.path()).and_then(|m| m.pr_number), Some(7));
        assert!(read(window.path()).is_none());
    }

    #[test]
    fn 消えた_dir_では_gh_を呼ばずにそう言う() {
        let gone = tempfile::tempdir().unwrap().path().join("gone");
        assert_eq!(
            open_branch_page(&gone),
            Err(format!("{} no longer exists.", gone.display()))
        );
    }

    #[test]
    fn gh_の答えは_pr_と_pr_無しと答えられないに分ける() {
        use crate::panels::fleet::PrLookup;
        use std::os::unix::process::ExitStatusExt;
        let out = |code: i32, stdout: &str, stderr: &str| std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        let pr = "https://github.com/S-Nakamur-a/conductor/pull/419";
        assert_eq!(
            pr_lookup_of(&out(0, &format!("{pr}\n"), "")),
            PrLookup::Found(pr.into())
        );
        assert_eq!(pr_lookup_of(&out(0, "", "")), PrLookup::Missing);
        assert_eq!(pr_lookup_of(&out(0, " \n", "")), PrLookup::Missing);
        let ghe = "none of the git remotes configured for this repository point to a known GitHub host. To tell gh about a new GitHub host, please use `gh auth login`\n";
        assert_eq!(
            pr_lookup_of(&out(1, "", ghe)),
            PrLookup::Failed(ghe.trim().into())
        );
    }

    #[test]
    fn gitでない_dir_ではそう言う() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            open_branch_page(dir.path()),
            Err(format!(
                "{} is not in a git repository.",
                dir.path().display()
            ))
        );
    }

    #[test]
    fn ブランチも_origin_も無い_repo_では何も開かない() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        let unborn = open_branch_page(dir.path()).unwrap_err();
        assert!(
            unborn.ends_with("origin has no web URL to open."),
            "{unborn}"
        );
    }

    #[test]
    fn 一覧の木はmainを先頭に_残りをブランチ名順に並べ_セッションをそのdirを含む一番深いworktreeに置く()
     {
        let repo = crate::testing::TestRepo::new();
        let zeta = repo.worktree("zeta");
        let alpha = repo.worktree("alpha");
        let detached = repo.worktree("detached");
        let checkout = std::process::Command::new("git")
            .args(["checkout", "-q", "--detach"])
            .current_dir(&detached)
            .status()
            .unwrap();
        assert!(checkout.success());
        let early = repo.worktree("+early");
        let sub = zeta.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let (trees, placement) = fleet_trees(&[sub.clone(), repo.root()], &[]);
        assert_eq!(trees.len(), 1);
        let paths: Vec<&Path> = trees[0]
            .worktrees
            .iter()
            .map(|w| w.path.as_path())
            .collect();
        assert_eq!(
            paths,
            [
                repo.root().as_path(),
                detached.as_path(),
                early.as_path(),
                alpha.as_path(),
                zeta.as_path()
            ],
            "切り離した HEAD は表示どおり (sha) の綴りで並ぶ"
        );
        assert_eq!(placement.get(&sub), Some(&zeta));
        assert_eq!(placement.get(&repo.root()), Some(&repo.root()));
    }

    #[test]
    fn 一覧の木は名前が前方一致するだけの隣のdirをそのworktreeに置かない() {
        let repo = crate::testing::TestRepo::new();
        let foo = repo.worktree("foo");
        let neighbour = foo.with_file_name("foo-bar");
        std::fs::create_dir_all(&neighbour).unwrap();
        let (_, placement) = fleet_trees(std::slice::from_ref(&neighbour), &[repo.root()]);
        assert_eq!(placement.get(&neighbour), None);
    }

    #[test]
    fn 一覧の木は綴りの違うパスでも同じworktreeに置く() {
        let repo = crate::testing::TestRepo::new();
        let link_dir = tempfile::tempdir().unwrap();
        let link = link_dir.path().join("link");
        std::os::unix::fs::symlink(repo.root(), &link).unwrap();
        let (trees, placement) = fleet_trees(std::slice::from_ref(&link), &[repo.root()]);
        assert_eq!(trees.len(), 1, "同じ repo が 2 つの見出しにならない");
        assert_eq!(placement.get(&link), Some(&repo.root()));
    }

    #[test]
    fn 一覧の木はセッションの無いrootも引き_gitでないdirは置かない() {
        let repo = crate::testing::TestRepo::new();
        let plain = tempfile::tempdir().unwrap();
        let (trees, placement) = fleet_trees(&[plain.path().to_path_buf()], &[repo.root()]);
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].root, repo.root());
        assert!(placement.is_empty(), "{placement:?}");
    }

    #[test]
    fn smart_worktree_の結果は投げた後に_repo_を切り替えても届く() {
        let ws = crate::workspace::Workspace::for_test();
        let mut svc = conductor_svc::Services::<TaskResult>::new();
        // 存在しないプログラムを指すので AI は呼ばれず、結果は Err で返る。
        let api = ApiConfig {
            provider: "command".into(),
            command: vec!["/nonexistent/conductor-test-ai".into()],
            ..ApiConfig::default()
        };
        Task::SmartWorktree {
            description: "x".into(),
            api,
            repo: PathBuf::from("/nonexistent/repo"),
        }
        .spawn(&mut svc, &ws.task_env());
        svc.bump_generation();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let event = loop {
            if let Some(event) = svc.try_recv() {
                break event;
            }
            assert!(std::time::Instant::now() < deadline, "結果が届かない");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(matches!(
            event.kind,
            conductor_svc::EventKind::Task(TaskResult::SmartWorktreeCreated(Err(_)))
        ));
    }
}

#[cfg(test)]
mod fleet_delete_tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime};

    use conductor_core::claude_sessions::registry::{
        LiveSession, Owner, ProcInfo, Processes, Signal, Status,
    };
    use conductor_svc::pty::SessionKind;

    use super::fleet_delete::{inspect, run};
    use crate::panels::fleet::{Doomed, Inspected, Inspection, OwnPty};

    /// procStart "Thu Oct 22 02:05:54 2026" (UTC) と同じ時刻。
    fn started() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_634_754)
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `<tmp>/main` に main、`<tmp>/wt` に wt ブランチの linked worktree。
    fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&main, &["worktree", "add", "-q", "-b", "wt", "../wt"]);
        let main = main.canonicalize().unwrap();
        let wt = dir.path().join("wt").canonicalize().unwrap();
        (dir, main, wt)
    }

    fn live(pid: u32, cwd: &Path, session: &str) -> LiveSession {
        LiveSession {
            pid,
            started: started(),
            cwd: cwd.to_path_buf(),
            session_id: session.into(),
            status: Status::Idle,
            status_since: None,
            owner: Owner::Conductor,
            sessions_dir: PathBuf::from("/c/sessions"),
        }
    }

    fn pty(id: &str, kind: SessionKind, dir: &Path, session: Option<&str>) -> OwnPty {
        OwnPty {
            panel_id: id.into(),
            kind,
            working_dir: dir.to_path_buf(),
            claude_session_id: session.map(String::from),
        }
    }

    fn ready(outcome: Inspected) -> Inspection {
        match outcome {
            Inspected::Ready(inspection) => *inspection,
            other => panic!("{other:?}"),
        }
    }

    /// pid ごとに生きているかを持つ。SIGTERM で終わるかも pid ごとに決める。
    struct Procs {
        alive: Mutex<Vec<(u32, bool)>>,
    }

    impl Processes for Procs {
        fn info(&self, pid: u32) -> Option<ProcInfo> {
            let alive = self.alive.lock().unwrap();
            alive.iter().any(|(p, _)| *p == pid).then_some(ProcInfo {
                zombie: false,
                parent: 1,
                started: started(),
            })
        }
        fn is_conductor(&self, _pid: u32) -> bool {
            false
        }
        fn signal(&self, pid: u32, _signal: Signal) -> std::io::Result<()> {
            self.alive
                .lock()
                .unwrap()
                .retain(|(p, survives)| *p != pid || *survives);
            Ok(())
        }
    }

    fn procs(alive: &[(u32, bool)]) -> Procs {
        Procs {
            alive: Mutex::new(alive.to_vec()),
        }
    }

    #[test]
    fn 点検は_cwd_がその下にある_claude_をどの窓のものでも数え_この窓のタブを選ぶ() {
        let (_dir, main, wt) = repo();
        let sub = wt.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let live = [
            live(7, &sub, "s7"),
            live(8, &main, "s8"),
            live(9, &wt, "s9"),
        ];
        let own = [
            pty("p1", SessionKind::ClaudeCode, &wt, Some("s7")),
            pty("p2", SessionKind::Shell, &sub, None),
            pty("p3", SessionKind::ClaudeCode, &main, Some("s8")),
        ];
        let inspection = ready(inspect(&main, &wt, "wt".into(), &own, &main, &live));
        let pids: Vec<(u32, bool)> = inspection.claudes.iter().map(|c| (c.pid, c.own)).collect();
        assert_eq!(pids, [(7, true), (9, false)]);
        assert_eq!(inspection.panels, ["p1", "p2"]);
        assert_eq!((inspection.own_claudes, inspection.shells), (1, 1));
        assert!(inspection.in_window_repo);
    }

    #[test]
    fn 窓が開いている_worktree_とエディタのある_worktree_は何も止めずに断る() {
        let (_dir, main, wt) = repo();
        let refused = |outcome: Inspected| matches!(outcome, Inspected::Refused(_));
        assert!(refused(inspect(&main, &wt, "wt".into(), &[], &wt, &[])));
        let editor = [pty("e", SessionKind::Editor, &wt, None)];
        assert!(refused(inspect(
            &main,
            &wt,
            "wt".into(),
            &editor,
            &main,
            &[]
        )));
    }

    #[test]
    fn 中の_claude_が終わったと確かめられなければ消さない() {
        let (dir, main, wt) = repo();
        let mut inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &[]));
        inspection.claudes = vec![Doomed {
            pid: 9,
            started: started(),
            own: false,
            sessions_dir: dir.path().join("sessions"),
        }];
        let sessions = dir.path().join("sessions");
        let outcome = run(
            &inspection,
            std::slice::from_ref(&sessions),
            &procs(&[(9, true)]),
            Duration::ZERO,
        );
        assert!(
            outcome.as_ref().is_err_and(|e| e.contains("still running")),
            "{outcome:?}"
        );
        assert!(wt.exists());
    }

    #[test]
    fn 終わったのを確かめたら_worktree_とブランチを消す() {
        let (dir, main, wt) = repo();
        let mut inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &[]));
        inspection.claudes = vec![Doomed {
            pid: 7,
            started: started(),
            own: true,
            sessions_dir: dir.path().join("sessions"),
        }];
        let sessions = dir.path().join("sessions");
        let outcome = run(
            &inspection,
            std::slice::from_ref(&sessions),
            &procs(&[]),
            Duration::ZERO,
        );
        assert_eq!(outcome, Ok("Deleted wt.".to_string()));
        assert!(!wt.exists());
        let branches = Command::new("git")
            .args(["branch", "--list", "wt"])
            .current_dir(&main)
            .output()
            .unwrap();
        assert!(branches.stdout.is_empty());
    }

    #[test]
    fn 点検の後にコミットか未コミットが増えていたら消さない() {
        for change in ["commit", "dirty"] {
            let (dir, main, wt) = repo();
            let inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &[]));
            match change {
                "commit" => git(&wt, &["commit", "-q", "--allow-empty", "-m", "later"]),
                _ => std::fs::write(wt.join("new.txt"), "x").unwrap(),
            }
            let outcome = run(
                &inspection,
                &[dir.path().join("sessions")],
                &procs(&[]),
                Duration::ZERO,
            );
            assert!(outcome.is_err(), "{change}: {outcome:?}");
            assert!(wt.exists(), "{change}");
        }
    }

    #[test]
    fn 既定ブランチを持つ_worktree_を消してもブランチは残す() {
        let (dir, main, _wt) = repo();
        git(&main, &["checkout", "-q", "-b", "dev"]);
        git(&main, &["worktree", "add", "-q", "../on-main", "main"]);
        let on_main = dir.path().join("on-main").canonicalize().unwrap();
        let inspection = ready(inspect(&main, &on_main, "main".into(), &[], &main, &[]));
        assert!(inspection.check.keeps_branch);
        run(
            &inspection,
            &[dir.path().join("sessions")],
            &procs(&[]),
            Duration::ZERO,
        )
        .unwrap();
        assert!(!on_main.exists());
        git(&main, &["rev-parse", "--verify", "-q", "refs/heads/main"]);
    }

    #[test]
    fn 点検の後にその中で_claude_が起動していたら消さない() {
        let (dir, main, wt) = repo();
        let inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &[]));
        let sessions = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let record = serde_json::json!({
            "pid": 11,
            "sessionId": "late",
            "cwd": wt,
            "startedAt": 1_792_634_755_219u64,
            "procStart": "Thu Oct 22 02:05:54 2026",
            "status": "busy",
        });
        std::fs::write(sessions.join("11.json"), record.to_string()).unwrap();
        let outcome = run(
            &inspection,
            std::slice::from_ref(&sessions),
            &procs(&[(11, true)]),
            Duration::ZERO,
        );
        assert!(
            outcome.as_ref().is_err_and(|e| e.contains("started")),
            "{outcome:?}"
        );
        assert!(wt.exists());
    }

    #[test]
    fn 外の_claude_は止めて終わったのを確かめてから消す() {
        let (dir, main, wt) = repo();
        let mut inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &[]));
        inspection.claudes = vec![Doomed {
            pid: 9,
            started: started(),
            own: false,
            sessions_dir: dir.path().join("sessions"),
        }];
        let sessions = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let record = serde_json::json!({
            "pid": 9,
            "sessionId": "outside",
            "cwd": wt,
            "startedAt": 1_792_634_755_219u64,
            "procStart": "Thu Oct 22 02:05:54 2026",
            "status": "busy",
        });
        std::fs::write(sessions.join("9.json"), record.to_string()).unwrap();
        let procs = procs(&[(9, false)]);
        let outcome = run(
            &inspection,
            std::slice::from_ref(&sessions),
            &procs,
            Duration::ZERO,
        );
        assert_eq!(outcome, Ok("Deleted wt.".to_string()));
        assert!(procs.info(9).is_none(), "SIGTERM が届いている");
        assert!(!wt.exists());
    }

    #[test]
    fn 同じ会話の古いプロセスも点検で止める相手に入る() {
        let (dir, main, wt) = repo();
        let sessions = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for (pid, started_at) in [(9, 1_792_634_755_219u64), (10, 1_792_634_999_999u64)] {
            let record = serde_json::json!({
                "pid": pid,
                "sessionId": "same",
                "cwd": wt,
                "startedAt": started_at,
                "procStart": "Thu Oct 22 02:05:54 2026",
                "status": "busy",
            });
            std::fs::write(sessions.join(format!("{pid}.json")), record.to_string()).unwrap();
        }
        let procs = procs(&[(9, false), (10, false)]);
        let live = conductor_core::claude_sessions::registry::read_every(
            std::slice::from_ref(&sessions),
            &procs,
        );

        let inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &live));

        let mut pids: Vec<u32> = inspection.claudes.iter().map(|c| c.pid).collect();
        pids.sort();
        assert_eq!(pids, [9, 10]);
    }

    #[test]
    fn 別のアカウントの外の_claude_はその写しの_dir_で照合して止めてから消す() {
        let (dir, main, wt) = repo();
        let work = dir.path().join("work/sessions");
        let personal = dir.path().join("personal/sessions");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&personal).unwrap();
        let record = serde_json::json!({
            "pid": 9,
            "sessionId": "outside",
            "cwd": wt,
            "startedAt": 1_792_634_755_219u64,
            "procStart": "Thu Oct 22 02:05:54 2026",
            "status": "busy",
        });
        std::fs::write(personal.join("9.json"), record.to_string()).unwrap();
        let procs = procs(&[(9, false)]);
        let dirs = [work, personal.clone()];
        let live = conductor_core::claude_sessions::registry::read_every(&dirs, &procs);
        let inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &live));
        assert_eq!(inspection.claudes[0].sessions_dir, personal);

        let outcome = run(&inspection, &dirs, &procs, Duration::ZERO);

        assert_eq!(outcome, Ok("Deleted wt.".to_string()));
        assert!(procs.info(9).is_none(), "SIGTERM が届いている");
    }

    #[test]
    fn 無視ファイルは未コミットに数えず_worktree_ごと消える() {
        let (dir, main, wt) = repo();
        std::fs::create_dir_all(main.join(".git/info")).unwrap();
        std::fs::write(main.join(".git/info/exclude"), ".env\n").unwrap();
        std::fs::write(wt.join(".env"), "SECRET=1").unwrap();
        let inspection = ready(inspect(&main, &wt, "wt".into(), &[], &main, &[]));
        assert_eq!(inspection.check.dirty, Some(0));
        let outcome = run(
            &inspection,
            &[dir.path().join("sessions")],
            &procs(&[]),
            Duration::ZERO,
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(!wt.join(".env").exists());
    }

    #[test]
    fn 状態ファイルの無いこの窓の_claude_はタブを閉じる側に入り_終わりは待たない() {
        let (dir, main, wt) = repo();
        let own = [pty("p1", SessionKind::ClaudeCode, &wt, Some("s1"))];
        let inspection = ready(inspect(&main, &wt, "wt".into(), &own, &main, &[]));
        assert!(inspection.claudes.is_empty());
        assert_eq!(inspection.own_claudes, 1);
        assert_eq!(inspection.panels, ["p1"]);
        let begun = std::time::Instant::now();
        let outcome = run(
            &inspection,
            &[dir.path().join("sessions")],
            &procs(&[]),
            Duration::ZERO,
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(begun.elapsed() < Duration::from_secs(5), "待っていない");
    }
}

#[cfg(test)]
mod branch_changes_tests {
    use std::path::Path;
    use std::process::Command;

    use conductor_core::diff_state::DiffSource;

    use super::branch_changes;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// 既定ブランチが staging の repo で、feature にコミット 1 つと未コミット・未追跡を置く。
    fn staging_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "staging"]);
        std::fs::write(root.join("kept.txt"), "a\n").unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "a\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "init"]);
        git(root, &["update-ref", "refs/remotes/origin/staging", "HEAD"]);
        git(
            root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/staging",
            ],
        );
        git(root, &["checkout", "-qb", "feature"]);
        std::fs::write(root.join("committed.txt"), "c\n").unwrap();
        git(root, &["add", "committed.txt"]);
        git(root, &["commit", "-qm", "feature"]);
        std::fs::write(root.join("src/lib.rs"), "a\nb\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "u\n").unwrap();
        dir
    }

    fn paths(dir: &Path) -> Vec<String> {
        let (diff, _) = branch_changes(dir, 4);
        assert_eq!(diff.error, None);
        diff.files.iter().map(|f| f.path.clone()).collect()
    }

    #[test]
    fn その_repo_の既定ブランチからのコミット済み_未コミット_未追跡を読む() {
        let repo = staging_repo();
        let (diff, _) = branch_changes(repo.path(), 4);
        assert_eq!(diff.source, DiffSource::working_tree("staging"));
        assert_eq!(
            paths(repo.path()),
            ["committed.txt", "src/lib.rs", "untracked.txt"]
        );
    }

    #[test]
    fn サブディレクトリで動く_claude_の行でもその_worktree_全体を読む() {
        let repo = staging_repo();
        assert_eq!(
            paths(&repo.path().join("src")),
            ["committed.txt", "src/lib.rs", "untracked.txt"]
        );
    }

    #[test]
    fn 未コミットのファイルは_git_status_の色の元を持つ() {
        let repo = staging_repo();
        let (_, status) = branch_changes(repo.path(), 4);
        assert!(status.status("src/lib.rs").is_some());
        assert!(status.status("committed.txt").is_none());
    }

    #[test]
    fn git_の外では理由を返し落ちない() {
        let dir = tempfile::tempdir().unwrap();
        let (diff, _) = branch_changes(dir.path(), 4);
        assert!(diff.files.is_empty());
        assert!(diff.error.unwrap().contains("not in a git working tree"));
    }
}

#[cfg(test)]
mod resumable_tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{TaskEnv, TaskResult, find_resumable, list_sessions};

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }

    /// config_dir の projects に会話のログを置き、history.jsonl に 1 行足す。
    fn conversation(config_dir: &Path, project: &Path, id: &str, timestamp: u64) {
        use std::io::Write;
        let encoded = project.to_string_lossy().replace(['/', '.'], "-");
        let logs = config_dir.join("projects").join(encoded);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join(format!("{id}.jsonl")), "").unwrap();
        let mut history = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config_dir.join("history.jsonl"))
            .unwrap();
        writeln!(
            history,
            r#"{{"display":"d","timestamp":{timestamp},"project":"{}","sessionId":"{id}"}}"#,
            project.display()
        )
        .unwrap();
    }

    fn env_at(root: PathBuf, claude_dirs: Vec<PathBuf>) -> TaskEnv {
        TaskEnv {
            root,
            version: "0",
            main_branch: "main".into(),
            worktree_dir: None,
            word_diff: false,
            tab_width: 4,
            branch: "main".into(),
            claude_dirs,
        }
    }

    #[test]
    fn 起動時の復帰は会話を最後に走らせたアカウントで_grab済みのmainも同じ() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&main, &["worktree", "add", "-q", "-b", "wt", "../wt"]);
        let wt = root.join("wt");
        let (work, personal) = (root.join("work"), root.join("personal"));
        // projects を共有した設定: ログは両方に、一時的に personal で続けた行は personal に。
        for config_dir in [&work, &personal] {
            conversation(config_dir, &wt, "switched", 1000);
            conversation(config_dir, &main, "grabbed", 1000);
        }
        conversation(&personal, &wt, "switched", 2000);
        conversation(&personal, &main, "grabbed", 2000);
        let env = env_at(main.clone(), vec![work, personal.clone()]);

        let TaskResult::Resumable { sessions, .. } =
            find_resumable(&env, &[main.clone(), wt.clone()])
        else {
            panic!("Resumable が返らない");
        };

        assert_eq!(sessions[&wt].config_dir, personal);
        assert_eq!(
            env.claude_home().config_dir_of(&main, "grabbed").unwrap(),
            Some(personal)
        );
    }

    #[test]
    fn resumeの一覧は全アカウントの会話を出す() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (work, personal) = (root.join("work"), root.join("personal"));
        conversation(&work, &root.join("a"), "in-work", 1000);
        conversation(&personal, &root.join("b"), "in-personal", 2000);
        let env = env_at(root.clone(), vec![work, personal]);

        let mut ids: Vec<String> = list_sessions(&env, true)
            .unwrap()
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["in-personal", "in-work"]);
    }
}
