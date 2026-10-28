//! revidere skill が一時 dir に残した作業 dir から、worktree ごとにレビューの有無を引く。
//!
//! 作業 dir は `revidere.py prepare` が `revidere-*` の名前で作り、meta.json を書く。
//! review.html は render まで進んだものにしか無いので、両方ある dir だけを数える。
//! publish したページの URL は `revidere.py published` が meta.json の artifact_url に書き足す。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub root: PathBuf,
    pub branch: String,
    pub head_oid: String,
    pub url: Option<String>,
    /// review.html の mtime。同じ印の記録のうち url のあるものが複数あれば、新しいものを開く。
    pub rendered: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub mark: Mark,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// 今の commit を読んだレビューがある。
    Current,
    /// 同じブランチの、HEAD と違う commit を読んだレビューだけがある。
    Stale,
}

#[derive(Deserialize)]
struct Meta {
    root: PathBuf,
    branch: String,
    head_oid: String,
    artifact_url: Option<String>,
}

const ARTIFACT_URL: &str = "https://claude.ai/";

pub fn read(tmp: &Path) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("revidere-"))
        .filter_map(|e| record_in(&e.path()))
        .collect()
}

fn record_in(dir: &Path) -> Option<Record> {
    let page = std::fs::metadata(dir.join("review.html")).ok()?;
    if !page.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(dir.join("meta.json")).ok()?;
    let meta: Meta = serde_json::from_str(&text).ok()?;
    Some(Record {
        root: meta.root.canonicalize().unwrap_or(meta.root),
        branch: meta.branch,
        head_oid: meta.head_oid,
        url: meta
            .artifact_url
            .filter(|url| url.starts_with(ARTIFACT_URL)),
        rendered: page.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    })
}

/// worktree は canonical なパスで渡す。detached なら branch は git の `--abbrev-ref` と同じ "HEAD"。
/// `--abbrev-ref` は同名の ref があると `heads/<branch>` や `refs/heads/<branch>` と書く。
pub fn review_of(records: &[Record], worktree: &Path) -> Option<Review> {
    let mine: Vec<&Record> = records.iter().filter(|r| r.root == worktree).collect();
    if mine.is_empty() {
        return None;
    }
    let repo = git2::Repository::open(worktree).ok()?;
    let head = repo.head().ok()?;
    let oid = head.target()?.to_string();
    let branch = if head.is_branch() {
        head.shorthand()?.to_string()
    } else {
        "HEAD".to_string()
    };
    let spellings = [
        branch.clone(),
        format!("heads/{branch}"),
        format!("refs/heads/{branch}"),
    ];
    let (current, stale): (Vec<&Record>, Vec<&Record>) = mine
        .into_iter()
        .filter(|r| spellings.contains(&r.branch))
        .partition(|r| r.head_oid == oid);
    let (mark, group) = match (current.is_empty(), stale.is_empty()) {
        (false, _) => (Mark::Current, current),
        (true, false) => (Mark::Stale, stale),
        (true, true) => return None,
    };
    let url = group
        .into_iter()
        .filter(|r| r.url.is_some())
        .max_by_key(|r| r.rendered)
        .and_then(|r| r.url.clone());
    Some(Review { mark, url })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestRepo;

    /// revidere.py prepare が書いた実物 (このリポジトリを読んだもの) をそのまま。
    const REAL_META: &str = r#"{
 "root": "/Users/shunnaka/ghq/github.com/S-Nakamur-a/conductor-worktrees/revidere-as-claude-skill",
 "branch": "feature/revidere-as-claude-skill",
 "base_ref": "origin/main",
 "base_oid": "a862d24a13479c8dbc9df65cfceae05e72abe40f",
 "head_oid": "2917a623ceba5261db1216c46ad85c65cc4b9f19",
 "github": "S-Nakamur-a/conductor",
 "github_note": null,
 "head_pushed": false,
 "base_pushed": true,
 "dirty": [
  "CLAUDE.md",
  "README.md",
  "plugins/conductor/.claude-plugin/plugin.json",
  "plugins/conductor/skills/revidere/SKILL.md",
  "plugins/conductor/skills/revidere/scripts/revidere.py",
  "plugins/conductor/skills/revidere/scripts/test_revidere.py"
 ]
}"#;

    fn workdir(tmp: &Path, name: &str, meta: &str, rendered: bool) {
        let dir = tmp.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meta.json"), meta).unwrap();
        std::fs::write(dir.join("diff.patch"), "").unwrap();
        if rendered {
            std::fs::write(dir.join("review.html"), "<html></html>").unwrap();
        }
    }

    fn mark(tmp: &Path, worktree: &Path) -> Option<Mark> {
        review_of(&read(tmp), worktree).map(|r| r.mark)
    }

    fn meta(root: &Path, branch: &str, head: &str) -> String {
        serde_json::json!({
            "root": root, "branch": branch, "base_ref": "origin/main", "base_oid": head,
            "head_oid": head, "github": null, "github_note": "origin が無い",
            "head_pushed": false, "base_pushed": false, "dirty": [],
        })
        .to_string()
    }

    #[test]
    fn 実物のmeta_jsonから根とブランチとheadを読む() {
        let tmp = tempfile::tempdir().unwrap();
        workdir(tmp.path(), "revidere-p553jsm0", REAL_META, true);
        let [record] = <[Record; 1]>::try_from(read(tmp.path())).unwrap();
        assert_eq!(
            record.root,
            PathBuf::from(
                "/Users/shunnaka/ghq/github.com/S-Nakamur-a/conductor-worktrees/revidere-as-claude-skill"
            )
        );
        assert_eq!(record.branch, "feature/revidere-as-claude-skill");
        assert_eq!(record.head_oid, "2917a623ceba5261db1216c46ad85c65cc4b9f19");
        assert_eq!(record.url, None);
    }

    #[test]
    fn ページまで作っていない作業dirと壊れたmetaと別名のdirは数えない() {
        let tmp = tempfile::tempdir().unwrap();
        workdir(tmp.path(), "revidere-prepared", REAL_META, false);
        workdir(tmp.path(), "revidere-broken", "{\"root\": ", true);
        workdir(
            tmp.path(),
            "revidere-nohead",
            r#"{"root": "/r", "branch": "b"}"#,
            true,
        );
        workdir(tmp.path(), "other-dir", REAL_META, true);
        workdir(tmp.path(), "revidere-ok", REAL_META, true);
        assert_eq!(read(tmp.path()).len(), 1);
        assert!(read(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn 今のheadを読んだものがあれば済_同じブランチの前のだけなら古() {
        let repo = TestRepo::with_base_commit();
        let worktree = repo.path.canonicalize().unwrap();
        let first = repo.repo.head().unwrap().target().unwrap().to_string();
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "main", &first),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));

        repo.file("b.txt", "b").add("b.txt").commit("second");
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Stale));

        let second = repo.repo.head().unwrap().target().unwrap().to_string();
        workdir(
            tmp.path(),
            "revidere-b",
            &meta(&worktree, "main", &second),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));
    }

    #[test]
    fn 別のブランチを読んだものと別のworktreeのものは数えない() {
        let repo = TestRepo::with_base_commit();
        let worktree = repo.path.canonicalize().unwrap();
        let head = repo.repo.head().unwrap().target().unwrap().to_string();
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "other", &head),
            true,
        );
        workdir(
            tmp.path(),
            "revidere-b",
            &meta(Path::new("/r/elsewhere"), "main", &head),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), None);
    }

    #[test]
    fn 同名のrefがあるときのheads付きとrefs_heads付きの綴りもブランチとして比べ_別の接頭辞は比べない()
     {
        let repo = TestRepo::with_base_commit();
        let worktree = repo.path.canonicalize().unwrap();
        let head = repo.repo.head().unwrap().target().unwrap().to_string();
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "heads/main", &head),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "refs/heads/main", &head),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "a/main", &head),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), None);
    }

    #[test]
    fn heads_で始まる名前のブランチもその名前のまま比べる() {
        let repo = TestRepo::with_base_commit();
        repo.branch("heads/foo").checkout("heads/foo");
        let worktree = repo.path.canonicalize().unwrap();
        let head = repo.repo.head().unwrap().target().unwrap().to_string();
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "heads/foo", &head),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));
    }

    #[test]
    fn detachedのworktreeはheadとして書かれたものと比べる() {
        let repo = TestRepo::with_base_commit();
        let worktree = repo.path.canonicalize().unwrap();
        let oid = repo.repo.head().unwrap().target().unwrap();
        repo.repo.set_head_detached(oid).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "main", &oid.to_string()),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), None);
        workdir(
            tmp.path(),
            "revidere-b",
            &meta(&worktree, "HEAD", &oid.to_string()),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));
    }

    #[test]
    fn 記録の根の綴りが違っても実体が同じworktreeなら数える() {
        let repo = TestRepo::with_base_commit();
        let worktree = repo.path.canonicalize().unwrap();
        let link_dir = tempfile::tempdir().unwrap();
        let link = link_dir.path().join("link");
        std::os::unix::fs::symlink(&worktree, &link).unwrap();
        let head = repo.repo.head().unwrap().target().unwrap().to_string();
        let tmp = tempfile::tempdir().unwrap();
        workdir(tmp.path(), "revidere-a", &meta(&link, "main", &head), true);
        assert_eq!(mark(tmp.path(), &worktree), Some(Mark::Current));
    }

    fn published(tmp: &Path, name: &str, meta: &str, url: Option<&str>, age_secs: u64) {
        let mut meta: serde_json::Value = serde_json::from_str(meta).unwrap();
        if let Some(url) = url {
            meta["artifact_url"] = url.into();
        }
        workdir(tmp, name, &meta.to_string(), true);
        let at = SystemTime::now() - std::time::Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(tmp.join(name).join("review.html"))
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    #[test]
    fn 開くのは印を決めた記録のうちurlのある新しいページ_claude_ai以外は採らない() {
        let repo = TestRepo::with_base_commit();
        let worktree = repo.path.canonicalize().unwrap();
        let old = repo.repo.head().unwrap().target().unwrap().to_string();
        repo.file("b.txt", "b").add("b.txt").commit("second");
        let head = repo.repo.head().unwrap().target().unwrap().to_string();
        let url = |tmp: &Path| review_of(&read(tmp), &worktree).and_then(|r| r.url);

        let tmp = tempfile::tempdir().unwrap();
        let stale = meta(&worktree, "main", &old);
        published(
            tmp.path(),
            "revidere-a",
            &stale,
            Some("https://claude.ai/artifact/old"),
            60,
        );
        published(
            tmp.path(),
            "revidere-b",
            &stale,
            Some("https://claude.ai/artifact/new"),
            10,
        );
        assert_eq!(
            url(tmp.path()).as_deref(),
            Some("https://claude.ai/artifact/new")
        );

        let current = meta(&worktree, "main", &head);
        published(
            tmp.path(),
            "revidere-c",
            &current,
            Some("https://claude.ai/artifact/cur"),
            300,
        );
        assert_eq!(
            url(tmp.path()).as_deref(),
            Some("https://claude.ai/artifact/cur")
        );

        published(tmp.path(), "revidere-d", &current, None, 5);
        assert_eq!(
            url(tmp.path()).as_deref(),
            Some("https://claude.ai/artifact/cur"),
            "publish 前の新しいページがあっても、公開済みのページを開く"
        );
        let other_branch = meta(&worktree, "other", &head);
        published(
            tmp.path(),
            "revidere-e",
            &other_branch,
            Some("https://claude.ai/artifact/other"),
            0,
        );
        assert_eq!(
            url(tmp.path()).as_deref(),
            Some("https://claude.ai/artifact/cur")
        );

        let tmp = tempfile::tempdir().unwrap();
        published(tmp.path(), "revidere-a", &current, None, 0);
        published(
            tmp.path(),
            "revidere-b",
            &stale,
            Some("https://claude.ai/artifact/old"),
            0,
        );
        published(
            tmp.path(),
            "revidere-c",
            &current,
            Some("https://example.com/x"),
            0,
        );
        assert_eq!(
            review_of(&read(tmp.path()), &worktree),
            Some(Review {
                mark: Mark::Current,
                url: None
            }),
            "前の commit のページでは代えない"
        );
    }

    #[test]
    fn commitの無いworktreeには印を付けない() {
        let repo = TestRepo::new();
        let worktree = repo.path.canonicalize().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        workdir(
            tmp.path(),
            "revidere-a",
            &meta(&worktree, "main", "0000"),
            true,
        );
        assert_eq!(mark(tmp.path(), &worktree), None);
    }
}
