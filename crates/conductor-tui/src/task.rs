//! svc に投げる仕事と、その結果の語彙。svc は中身を知らない。

use std::path::{Path, PathBuf};

use conductor_core::claude_sessions::{ClaudeHome, ResumableSession};
use conductor_core::config::{self, LayoutConfig};
use conductor_core::diff_state::DiffState;
use conductor_core::git_engine::{GitEngine, WorktreeInfo, conductor_dir};
use conductor_core::grep_search::{self, GrepMatch};
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
        }
    }
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
