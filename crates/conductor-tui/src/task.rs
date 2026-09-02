//! svc に投げる仕事と、その結果の語彙。svc は中身を知らない。

use std::path::{Path, PathBuf};

use conductor_core::claude_sessions::{ClaudeHome, ResumableSession};
use conductor_core::config::{self, LayoutConfig};
use conductor_core::diff_state::DiffState;
use conductor_core::git_engine::{CommitInfo, GitEngine, GrabState, WorktreeInfo, conductor_dir};
use conductor_core::grep_search::{self, GrepMatch};
use conductor_core::pr_intake::{self, PrIntakeOutcome};
use conductor_core::review_publish::{self, PublishComment, PublishOutcome, PublishRequest};
use conductor_core::review_store::{
    Author, CommentKind, CommentStatus, NewReview, ReviewStore, SessionHistory,
};
use conductor_svc::Services;

use crate::panels::explorer::tree;
use crate::panels::viewer::content;
use crate::review::Snapshot;

/// Task が git を触るのに要るもの。Workspace が持っているので、Task 自身は運ばない。
#[derive(Debug, Clone)]
pub struct TaskEnv {
    pub root: PathBuf,
    pub main_branch: String,
    pub worktree_dir: Option<PathBuf>,
    pub word_diff: bool,
    pub tab_width: usize,
    /// レビュー DB の行を引くキー。
    pub branch: String,
}

#[derive(Debug)]
pub enum Task {
    ListWorktrees,
    CreateWorktree {
        branch: String,
    },
    DeleteWorktree {
        path: PathBuf,
        branch: String,
    },
    LoadTree {
        root: PathBuf,
        expanded: Vec<String>,
    },
    ComputeDiff {
        worktree: PathBuf,
    },
    /// `seq` は結果の照合に使う。同じファイルを続けて開いても古い結果が新しい本文を
    /// 上書きしない。
    LoadFile {
        root: PathBuf,
        path: String,
        seq: u64,
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
    },
    /// resume できる Claude セッション。`all` ならこのリポジトリの外も含める。
    ListSessions {
        all: bool,
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
    /// PR 番号か URL を worktree にする。取り込めたら PR のメタデータも書く。
    IntakePr {
        input: String,
    },
    /// 未公開コメントと投稿先。
    LoadPublishable,
    Publish(Box<Publishable>),
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

/// grab / ungrab が返すもの。
#[derive(Debug)]
pub struct GrabDone {
    pub message: String,
    pub state: Option<GrabState>,
    /// 移行した Claude セッションと、それを開き直す worktree。
    pub resume: Option<(String, PathBuf)>,
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
    /// 作成できた worktree のパスと、そのブランチ。
    WorktreeCreated(Result<(PathBuf, String), String>),
    WorktreeDeleted(Result<String, String>),
    Tree(Box<tree::Snapshot>),
    /// DiffState は失敗の理由を自分の中に持つので Result にしない。
    Diff(Box<DiffState>),
    FileLoaded {
        seq: u64,
        loaded: Result<content::Loaded, String>,
    },
    Review(Result<Box<Snapshot>, String>),
    Grep {
        seq: u64,
        found: Result<Vec<GrepMatch>, String>,
    },
    Sessions(Result<Vec<ResumableSession>, String>),
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
    /// 取り込めた PR の番号と worktree。
    PrIntake(Result<(u64, PathBuf), String>),
    Publishable(Result<Box<Publishable>, String>),
    Published(PublishOutcome),
}

impl Task {
    pub fn spawn(self, svc: &mut Services<TaskResult>, env: &TaskEnv) {
        let env = env.clone();
        match self {
            Task::ListWorktrees => {
                svc.spawn(move || list_worktrees(&env), TaskResult::Worktrees);
            }
            Task::CreateWorktree { branch } => {
                svc.spawn(
                    move || create_worktree(&env, &branch),
                    TaskResult::WorktreeCreated,
                );
            }
            Task::DeleteWorktree { path, branch } => {
                svc.spawn(
                    move || delete_worktree(&env, &path, &branch),
                    TaskResult::WorktreeDeleted,
                );
            }
            Task::LoadTree { root, expanded } => {
                svc.spawn(
                    move || Box::new(tree::survey(&root, &expanded)),
                    TaskResult::Tree,
                );
            }
            Task::ComputeDiff { worktree } => {
                svc.spawn(
                    move || Box::new(compute_diff(&env, &worktree)),
                    TaskResult::Diff,
                );
            }
            Task::LoadFile { root, path, seq } => {
                svc.spawn(
                    move || (seq, content::read(&root, &path, env.tab_width)),
                    |(seq, loaded)| TaskResult::FileLoaded { seq, loaded },
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
            } => {
                svc.spawn(
                    move || (seq, grep(&root, &query, regex, case_sensitive)),
                    |(seq, found)| TaskResult::Grep { seq, found },
                );
            }
            Task::PersistConfig(what) => {
                svc.spawn(move || persist(&what), TaskResult::Persisted);
            }
            Task::ListSessions { all } => {
                svc.spawn(move || list_sessions(&env, all), TaskResult::Sessions);
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
            Task::IntakePr { input } => {
                svc.spawn(move || intake_pr(&env, &input), TaskResult::PrIntake);
            }
            Task::LoadPublishable => {
                svc.spawn(move || publishable(&env), TaskResult::Publishable);
            }
            Task::Publish(request) => {
                svc.spawn(move || publish(&env, *request), TaskResult::Published);
            }
        }
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

fn create_from_remote(env: &TaskEnv, remote_branch: &str) -> Result<(PathBuf, String), String> {
    let branch = remote_branch
        .strip_prefix("origin/")
        .unwrap_or(remote_branch)
        .to_string();
    git(env, |git| {
        git.create_worktree_from_remote(remote_branch, env.worktree_dir.as_deref())
    })
    .map(|path| (path, branch))
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
    let home = ClaudeHome::detect();
    let session = home.as_ref().and_then(|home| {
        home.find_latest_sessions_for_paths(std::slice::from_ref(&source.to_path_buf()))
            .ok()?
            .remove(
                &source
                    .canonicalize()
                    .unwrap_or_else(|_| source.to_path_buf()),
            )
    });

    let main = git(env, |git| git.main_worktree_path())?;
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
    let resume = session.filter(|session| match &home {
        Some(home) => {
            match home.migrate_session(&session.session_id, source, &main, &session.display) {
                Ok(migrated) => migrated,
                Err(e) => {
                    log::warn!("grab: session migration failed: {e:#}");
                    false
                }
            }
        }
        None => false,
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
        resume: resume.map(|s| (s.session_id, main)),
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
    if let (Some(id), Some(home)) = (&grabbed.claude_session_id, ClaudeHome::detect())
        && let Err(e) = home.unmigrate_session(id, &grabbed.source_worktree, &main)
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

fn intake_pr(env: &TaskEnv, input: &str) -> Result<(u64, PathBuf), String> {
    match pr_intake::intake_pr(&env.root, env.worktree_dir.as_deref(), input) {
        PrIntakeOutcome::Failed { error } => Err(error.to_string()),
        PrIntakeOutcome::Ready {
            pr_number,
            worktree_path,
            meta,
        } => {
            // 取り込みは済んでいるので、メタデータを書けなくても worktree は返す。
            if let Some(meta) = meta
                && let Err(e) = save_pr_meta(env, pr_number, &meta)
            {
                log::warn!("could not record the PR metadata: {e}");
            }
            Ok((pr_number, worktree_path))
        }
    }
}

fn save_pr_meta(env: &TaskEnv, pr_number: u64, meta: &pr_intake::FetchedPr) -> Result<(), String> {
    let store = open_store(env)?;
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

fn publishable(env: &TaskEnv) -> Result<Box<Publishable>, String> {
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
    Ok(Box::new(Publishable {
        owner,
        repo,
        pr_number: pr_number as u64,
        comments,
        skipped: 0,
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
) -> Result<Vec<GrepMatch>, String> {
    let re =
        grep_search::compile_pattern(query, regex, case_sensitive).map_err(|e| e.to_string())?;
    Ok(grep_search::search_tree(root, &re))
}

fn persist(what: &Persist) -> Result<(), String> {
    match what {
        Persist::Theme(name) => config::persist_ui_theme(name),
        Persist::HighContrast(on) => config::persist_ui_high_contrast(*on),
        Persist::Layout(layout) => config::persist_layout_proportions(layout),
    }
    .map_err(|e| e.to_string())
}

fn list_sessions(env: &TaskEnv, all: bool) -> Result<Vec<ResumableSession>, String> {
    let home = ClaudeHome::detect().ok_or("could not find ~/.claude")?;
    home.load_resumable_sessions((!all).then_some(env.root.as_path()))
        .map_err(|e| e.to_string())
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
    let dir = conductor_dir(&env.root);
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

fn create_worktree(env: &TaskEnv, branch: &str) -> Result<(PathBuf, String), String> {
    let git = GitEngine::open(&env.root).map_err(|e| e.to_string())?;
    let base = git.resolve_base_ref(&env.main_branch);
    // 分岐していれば触らないので、失敗しても作成そのものは続けられる。
    if let Err(e) = git.ensure_base_ref_available(&env.main_branch) {
        log::warn!("could not fast-forward the base ref: {e:#}");
    }
    git.create_worktree_from_base(branch, &base, env.worktree_dir.as_deref())
        .map(|path| (path, branch.to_string()))
        .map_err(|e| e.to_string())
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

fn compute_diff(env: &TaskEnv, worktree: &Path) -> DiffState {
    let mut diff = DiffState::new(&env.main_branch);
    diff.load_diff(worktree, &env.main_branch, env.word_diff, env.tab_width);
    diff
}
