//! reflog と merge-base ヒューリスティックによる親ブランチ・派生ブランチの
//! 検出と、ブランチに対する GitHub/GitLab のプルリクエスト URL の構築。

use super::GitEngine;

impl GitEngine {
    // PR URL

    /// 指定したブランチの GitHub/GitLab プルリクエスト URL を構築する。
    ///
    /// origin リモートの URL を読み、HTTPS のベース URL に変換し、新規
    /// プルリクエスト作成用のプラットフォーム別パスを付加する。リモート URL
    /// をパースできない場合は None を返す。
    /// reflog を使って親ブランチを検出し、失敗したら merge-base ヒューリス
    /// ティックにフォールバックする。
    ///
    /// 1. branch の reflog を読み、作成時のコミット(最も古いエントリの
    ///    id_new)を見つける。
    /// 2. 各候補について、その作成コミットが祖先になっているか確認し、
    ///    最も近い候補を選ぶ。
    /// 3. reflog が使えない場合は main_branch からの merge-base 距離に
    ///    フォールバックする。
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

        // まず reflog ベースの検出を試す。
        if let Some(parent) =
            self.detect_parent_via_reflog(branch, branch_oid, main_branch, candidates)
        {
            return Some(parent);
        }

        // フォールバック: branch HEAD への merge-base が最も近い候補を探す。
        self.detect_parent_via_merge_base(branch, branch_oid, main_branch, candidates)
    }

    /// reflog ベースの親検出: どの候補がブランチ作成点を含むかを探す。
    fn detect_parent_via_reflog(
        &self,
        branch: &str,
        _branch_oid: git2::Oid,
        main_branch: &str,
        candidates: &[String],
    ) -> Option<String> {
        let reflog = self.repo.reflog(&format!("refs/heads/{branch}")).ok()?;

        if reflog.is_empty() {
            return None;
        }

        // 最も古いエントリ(最後のインデックス)がブランチ作成時を記録している。
        let oldest = reflog.get(reflog.len() - 1)?;
        let creation_oid = oldest.id_new();

        // 候補リストを構築する: 渡された候補 + main ブランチ。
        let all_candidates: Vec<&str> = candidates
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once(main_branch))
            .filter(|&c| c != branch)
            .collect();

        let mut best: Option<(String, usize)> = None;

        for &candidate_name in &all_candidates {
            let candidate_oid = self.resolve_branch_oid(candidate_name)?;

            // 作成コミットが候補の祖先になっているか確認する。
            let is_ancestor = self
                .repo
                .graph_descendant_of(candidate_oid, creation_oid)
                .unwrap_or(false);
            let is_same = candidate_oid == creation_oid;

            if !is_ancestor && !is_same {
                continue;
            }

            // creation_oid から候補の HEAD までの距離を計算する。
            let (ahead, _) = self
                .repo
                .graph_ahead_behind(candidate_oid, creation_oid)
                .unwrap_or((usize::MAX, 0));

            if best.as_ref().is_none_or(|(_, d)| ahead < *d) {
                best = Some((candidate_name.to_string(), ahead));
            }
        }

        // 補足: 作成コミットが実はブランチ自身にしか存在しないケースもある。
        // creation_oid == branch_oid で best が距離0の main であれば問題ない。
        best.map(|(name, _)| name)
    }

    /// merge-base によるフォールバック: merge-base 距離で最も近い候補を探す。
    fn detect_parent_via_merge_base(
        &self,
        branch: &str,
        branch_oid: git2::Oid,
        main_branch: &str,
        candidates: &[String],
    ) -> Option<String> {
        let all_candidates: Vec<&str> = candidates
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once(main_branch))
            .filter(|&c| c != branch)
            .collect();

        let mut best: Option<(String, usize)> = None;

        for &candidate_name in &all_candidates {
            let candidate_oid = match self.resolve_branch_oid(candidate_name) {
                Some(oid) => oid,
                None => continue,
            };

            let merge_base = match self.repo.merge_base(branch_oid, candidate_oid) {
                Ok(mb) => mb,
                Err(_) => continue,
            };

            // merge-base から branch HEAD までの距離(分岐後のコミット数)。
            let (ahead, _) = self
                .repo
                .graph_ahead_behind(branch_oid, merge_base)
                .unwrap_or((usize::MAX, 0));

            if best.as_ref().is_none_or(|(_, d)| ahead < *d) {
                best = Some((candidate_name.to_string(), ahead));
            }
        }

        best.map(|(name, _)| name)
    }

    /// ブランチ名を OID に解決する。まずローカルブランチを試し、次に
    /// origin リモートを試す。
    fn resolve_branch_oid(&self, name: &str) -> Option<git2::Oid> {
        // まずローカルブランチを試す。
        if let Ok(branch) = self.repo.find_branch(name, git2::BranchType::Local)
            && let Some(oid) = branch.get().target()
        {
            return Some(oid);
        }
        // origin/<name> を試す。
        let remote_ref = format!("refs/remotes/origin/{name}");
        self.repo.refname_to_id(&remote_ref).ok()
    }

    /// 指定したブランチから分岐した(candidates の中の)ブランチを探す。
    ///
    /// 各候補について detect_parent_branch を呼び、結果が現在のブランチ
    /// かどうかを確認する。
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
            // 内側の detect_parent_branch 呼び出し用の候補を構築する:
            // 現在のブランチ + 他の候補(候補自身は除く)を含める。
            let inner_candidates: Vec<String> = candidates
                .iter()
                .filter(|c| c.as_str() != candidate)
                .cloned()
                .collect();

            if let Some(parent) =
                self.detect_parent_branch(candidate, main_branch, &inner_candidates)
                && parent == branch
            {
                derived.push(candidate.clone());
            }
        }

        Ok(derived)
    }

    pub fn pr_url_for_branch(&self, branch: &str) -> Option<String> {
        let remote = self.repo.find_remote("origin").ok()?;
        let raw_url = remote.url()?;
        let base = Self::remote_url_to_https_base(raw_url)?;

        // GitHub: /compare/<branch>  (既存の PR があればそれを、なければ作成フォームを表示)
        // GitLab: /-/merge_requests/new?merge_request[source_branch]=<branch>
        if base.contains("gitlab") {
            Some(format!(
                "{base}/-/merge_requests/new?merge_request[source_branch]={branch}",
            ))
        } else {
            // デフォルトは GitHub 形式。
            Some(format!("{base}/pull/{branch}"))
        }
    }

    /// git のリモート URL を HTTPS のベース URL(末尾スラッシュなし)に変換する。
    ///
    /// SSH 形式(git@host:owner/repo.git)と HTTPS 形式
    /// (https://host/owner/repo.git)を扱う。
    ///
    /// 非公開ではなく pub(crate) にしているのは、tests.rs のユニット
    /// テストがこの関数を直接呼ぶため。そのサブモジュールはこのモジュールの
    /// プライバシー境界の外にある。
    pub(crate) fn remote_url_to_https_base(url: &str) -> Option<String> {
        let url = url.trim();
        if url.starts_with("git@") || url.starts_with("ssh://") {
            // git@github.com:owner/repo.git  →  https://github.com/owner/repo
            // ssh://git@github.com/owner/repo.git も同様。
            let without_prefix = url
                .strip_prefix("ssh://")
                .unwrap_or(url)
                .strip_prefix("git@")
                .unwrap_or(url);
            // "github.com:owner/repo.git" または "github.com/owner/repo.git"
            let normalised = without_prefix.replace(':', "/");
            let trimmed = normalised.trim_end_matches(".git");
            Some(format!("https://{trimmed}"))
        } else if url.starts_with("https://") || url.starts_with("http://") {
            let trimmed = url.trim_end_matches(".git");
            Some(trimmed.to_string())
        } else {
            None
        }
    }
}
