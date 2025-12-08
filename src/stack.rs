use std::fmt::Display;
use std::sync::Arc;

use anyhow::ensure;
use futures_util::future::join_all;

use crate::Config;
use crate::clients::git::GitClient;
use crate::clients::github::GithubClient;
use crate::clients::jujutsu::JujutsuClient;
use crate::clients::jujutsu::JujutsuCommit;

/// A stack of Jujutsu commits, ordered from child to parent (youngest to oldest).
pub struct Stack {
    config: Arc<Config>,
    jj: Arc<JujutsuClient>,
    gh: Arc<GithubClient>,
    git: Arc<GitClient>,
    pub commits: Vec<JujutsuCommit>,
}

pub enum SyncStatus {
    /// Commit has no associated PR
    Unknown,
    /// Commit unchanged from associated PR, but base is stale.
    Restack,
    /// Commit has been changed from associated PR, base may or may not be
    /// stale.
    Changed,
    /// Commit is in-sync with associated PR.
    Synced,
}

impl Display for SyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => f.write_str("?"),
            Self::Restack => f.write_str("↻"),
            Self::Changed => f.write_str("✗"),
            Self::Synced => f.write_str("✓"),
        }
    }
}

impl Stack {
    pub async fn new(
        config: Arc<Config>,
        jj: Arc<JujutsuClient>,
        gh: Arc<GithubClient>,
        git: Arc<GitClient>,
        commits: Vec<JujutsuCommit>,
    ) -> anyhow::Result<Self> {
        // Validate
        for (i, commit) in commits.iter().enumerate() {
            if i == commits.len() - 1 {
                let trunk = jj.get_commit("trunk()").await?;
                ensure!(commit.parent_change_ids.contains(&trunk.change_id));
            } else {
                ensure!(commit.parent_change_ids.contains(&commits[i + 1].change_id));
            }
        }

        Ok(Self {
            config,
            jj,
            gh,
            git,
            commits,
        })
    }

    pub(crate) async fn sync_statuses(&self) -> anyhow::Result<Vec<SyncStatus>> {
        // Iterate from parent to child (oldest to youngest)
        let commits_rev = self.commits.iter().rev().collect::<Vec<_>>();

        let pr_diffs = join_all(commits_rev.iter().map(|commit| async move {
            let pr_branch = commit
                .change_id
                .branch_name(&self.config.github_branch_prefix);
            self.gh.pr_diff(&pr_branch).await
        }))
        .await;

        // Iterate from parent to child (oldest to youngest)
        let mut statuses: Vec<SyncStatus> = vec![];
        let mut restack = false;
        for (i, commit) in commits_rev.iter().enumerate() {
            // PR branch
            let pr_branch = commit
                .change_id
                .branch_name(&self.config.github_branch_prefix);
            let Some(pr_tip) = self.git.get_branch_tip(&pr_branch).await.ok() else {
                restack = true;
                statuses.push(SyncStatus::Unknown);
                continue;
            };

            // Base branch
            let base_branch = if i > 0 {
                commits_rev[i - 1]
                    .change_id
                    .branch_name(&self.config.github_branch_prefix)
            } else {
                let trunk_commit = self.jj.get_commit("trunk()").await?;
                self.git
                    .get_git_remote_branches(&trunk_commit.commit_id)
                    .await?[0]
                    .clone()
            };
            let Some(base_tip) = self.git.get_branch_tip(&base_branch).await.ok() else {
                restack = true;
                statuses.push(SyncStatus::Unknown);
                continue;
            };

            // Check diff equality
            let commit_diff = self.git.get_commit_diff(&commit.commit_id).await?;
            let Ok(pr_diff) = &pr_diffs[i] else {
                restack = true;
                statuses.push(SyncStatus::Unknown);
                continue;
            };
            if commit_diff != *pr_diff {
                restack = true;
                statuses.push(SyncStatus::Changed);
                continue;
            }

            // Diffs are equal; now check whether PR contains base tip
            if !self.git.is_ancestor(&base_tip, &pr_tip).await? {
                restack = true;
                statuses.push(SyncStatus::Restack);
                continue;
            }

            if restack {
                statuses.push(SyncStatus::Restack);
                continue;
            }

            statuses.push(SyncStatus::Synced);
        }

        Ok(statuses.into_iter().rev().collect::<Vec<_>>())
    }
}
