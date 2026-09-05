//! reflog と merge-base による親ブランチ・派生ブランチの検出と、PR 作成 URL の構築。

use super::GitEngine;

impl GitEngine {
    /// branch の作成元と思われるブランチを candidates と main_branch の中から選ぶ。
    ///
    /// reflog の最古のエントリ (作成時のコミット) を祖先に持つ候補のうち最も近いものを
    /// 選び、reflog が無ければ merge-base が branch の tip に最も近い候補にする。
    pub fn detect_parent_branch(
        &self,
        branch: &str,
        main_branch: &str,
        candidates: &[String],
    ) -> Option<String> {
        if branch == main_branch {
            return None;
        }
        let branch_oid = self
            .repo
            .find_branch(branch, git2::BranchType::Local)
            .ok()?
            .get()
            .target()?;
        let others: Vec<&str> = candidates
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(main_branch))
            .filter(|&c| c != branch)
            .collect();

        self.parent_via_reflog(branch, &others)
            .or_else(|| self.parent_via_merge_base(branch_oid, &others))
    }

    fn parent_via_reflog(&self, branch: &str, candidates: &[&str]) -> Option<String> {
        let reflog = self.repo.reflog(&format!("refs/heads/{branch}")).ok()?;
        let creation_oid = reflog.get(reflog.len().checked_sub(1)?)?.id_new();

        let mut best: Option<(&str, usize)> = None;
        for &name in candidates {
            let candidate_oid = self.resolve_branch_oid(name)?;
            let contains_creation = candidate_oid == creation_oid
                || self
                    .repo
                    .graph_descendant_of(candidate_oid, creation_oid)
                    .unwrap_or(false);
            if !contains_creation {
                continue;
            }
            let (distance, _) = self
                .repo
                .graph_ahead_behind(candidate_oid, creation_oid)
                .unwrap_or((usize::MAX, 0));
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((name, distance));
            }
        }
        best.map(|(name, _)| name.to_string())
    }

    fn parent_via_merge_base(&self, branch_oid: git2::Oid, candidates: &[&str]) -> Option<String> {
        let mut best: Option<(&str, usize)> = None;
        for &name in candidates {
            let Some(candidate_oid) = self.resolve_branch_oid(name) else {
                continue;
            };
            let Ok(merge_base) = self.repo.merge_base(branch_oid, candidate_oid) else {
                continue;
            };
            let (distance, _) = self
                .repo
                .graph_ahead_behind(branch_oid, merge_base)
                .unwrap_or((usize::MAX, 0));
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((name, distance));
            }
        }
        best.map(|(name, _)| name.to_string())
    }

    /// ローカルブランチ、無ければ origin/<name>。
    fn resolve_branch_oid(&self, name: &str) -> Option<git2::Oid> {
        if let Ok(branch) = self.repo.find_branch(name, git2::BranchType::Local)
            && let Some(oid) = branch.get().target()
        {
            return Some(oid);
        }
        self.repo
            .refname_to_id(&format!("refs/remotes/origin/{name}"))
            .ok()
    }

    /// candidates のうち、親が branch だと判定されるものを返す。
    pub fn find_derived_branches(
        &self,
        branch: &str,
        main_branch: &str,
        candidates: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let mut derived = Vec::new();
        for candidate in candidates {
            if candidate == branch {
                continue;
            }
            let others: Vec<String> = candidates
                .iter()
                .filter(|c| *c != candidate)
                .cloned()
                .collect();
            if self
                .detect_parent_branch(candidate, main_branch, &others)
                .is_some_and(|parent| parent == branch)
            {
                derived.push(candidate.clone());
            }
        }
        Ok(derived)
    }

    /// origin の URL から、GitHub なら /pull/<branch>、GitLab なら MR 作成フォームの URL を作る。
    pub fn pr_url_for_branch(&self, branch: &str) -> Option<String> {
        let remote = self.repo.find_remote("origin").ok()?;
        let base = remote_url_to_https_base(remote.url()?)?;
        if base.contains("gitlab") {
            Some(format!(
                "{base}/-/merge_requests/new?merge_request[source_branch]={branch}"
            ))
        } else {
            Some(format!("{base}/pull/{branch}"))
        }
    }
}

/// `git@host:owner/repo.git`、`ssh://git@host/owner/repo.git`、`https://host/owner/repo.git`
/// を末尾スラッシュ無しの `https://host/owner/repo` にする。
fn remote_url_to_https_base(url: &str) -> Option<String> {
    let url = url.trim();
    if url.starts_with("git@") || url.starts_with("ssh://") {
        let without_scheme = url.strip_prefix("ssh://").unwrap_or(url);
        let without_user = without_scheme
            .strip_prefix("git@")
            .unwrap_or(without_scheme);
        let slashed = without_user.replace(':', "/");
        Some(format!("https://{}", slashed.trim_end_matches(".git")))
    } else if url.starts_with("https://") || url.starts_with("http://") {
        Some(url.trim_end_matches(".git").to_string())
    } else {
        None
    }
}
