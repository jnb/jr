use std::fmt::Display;

use anyhow::bail;
use log::debug;

use crate::Config;
use crate::clients::git::CommitId;
use crate::clients::git::GitClient;
use crate::clients::github::GithubClient;
use crate::clients::jujutsu::JujutsuChangeId;
use crate::clients::jujutsu::JujutsuClient;
use crate::clients::jujutsu::JujutsuCommit;
use crate::clients::jujutsu::JujutsuCommitMessage;
use crate::diff_utils::normalize_diff;

/// Length of the change ID to use in GitHub branch names
pub const GITHUB_CHANGE_ID_LENGTH: usize = 8;

/// An elaborated Jujutsu commit.
pub struct CommitInfo {
    pub commit: JujutsuCommit,
    /// The diff of this commit.
    pub commit_diff: String,
    /// The normalized diff of this commit.
    pub commit_diff_norm: String,
    /// The name of the PR branch e.g. prefix/klmnopqr.
    pub pr_branch: String,
    /// The tip of the remote PR branch, if it exists.
    pub pr_tip: Option<CommitId>,
    /// The current PR diff, if it exists.
    pub pr_diff: Option<String>,
    /// The normalized PR diff, if it exists.
    pub pr_diff_norm: Option<String>,
    /// The name of the parent commit's PR branch or main.
    pub base_branch: String,
    /// The tip of the remote base branch, if it exists.
    pub base_tip: Option<CommitId>,
    /// Whether the PR branch tip is a descendent of the base branch tip.
    pub pr_contains_base: bool,
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

impl CommitInfo {
    pub async fn new(
        commit: JujutsuCommit,
        config: &Config,
        jj: &JujutsuClient,
        gh: &GithubClient,
        git: &GitClient,
    ) -> anyhow::Result<Self> {
        let commit_diff = git.get_commit_diff(&commit.commit_id).await?;
        let commit_diff_norm = normalize_diff(&commit_diff);
        let trunk_commit = jj.get_trunk().await?;
        if git
            .is_ancestor(&commit.commit_id, &trunk_commit.commit_id)
            .await?
        {
            bail!(
                "Commit {} is an ancestor of trunk; this commit is already merged.",
                commit.commit_id
            );
        }

        let pr_branch = Self::branch_name(&commit.change_id, &config.github_branch_prefix);
        let pr_tip = git.get_branch_tip(&pr_branch).await.ok();
        let pr_diff = gh.pr_diff(&pr_branch).await.ok();
        let pr_diff_norm = pr_diff.as_ref().map(|diff| normalize_diff(diff));

        let parent_change_id = &commit.parent_change_ids[0];
        let parent_commit_id = jj.get_commit(&parent_change_id.0).await?.commit_id;
        let base_branch = if git
            .is_ancestor(&parent_commit_id, &trunk_commit.commit_id)
            .await?
        {
            // Parent is either trunk or an ancestor of trunk; in both cases
            // return a base branch of "main" or "master" etc.
            let trunk_branches = git.get_git_remote_branches(&trunk_commit.commit_id).await?;
            if trunk_branches.is_empty() {
                bail!("Trunk has no remote branch. Push trunk to remote first.");
            }
            trunk_branches[0].clone()
        } else {
            // Parent is in our stack
            Self::branch_name(&commit.parent_change_ids[0], &config.github_branch_prefix)
        };
        let base_tip = git.get_branch_tip(&base_branch).await.ok();

        let mut pr_contains_base = false;
        if let Some(base_tip) = &base_tip
            && let Some(pr_tip) = &pr_tip
        {
            pr_contains_base = git.is_ancestor(base_tip, pr_tip).await?;
        }

        Ok(Self {
            commit,
            commit_diff,
            commit_diff_norm,
            pr_branch,
            pr_tip,
            pr_diff,
            pr_diff_norm,
            base_branch,
            base_tip,
            pr_contains_base,
        })
    }

    pub fn status(&self) -> SyncStatus {
        if self.pr_tip.is_none() {
            debug!("pr_tip is None");
            return SyncStatus::Unknown;
        };
        if self.base_tip.is_none() {
            debug!("base_tip is None");
            return SyncStatus::Unknown;
        };
        let Some(pr_diff) = &self.pr_diff else {
            debug!("pr_diff is None");
            return SyncStatus::Unknown;
        };
        let commit_diff_norm = normalize_diff(&self.commit_diff);
        let pr_diff_norm = normalize_diff(pr_diff);
        if commit_diff_norm != pr_diff_norm {
            debug!("diffs are different");
            debug!("{}", commit_diff_norm);
            debug!("{}", pr_diff_norm);
            return SyncStatus::Changed;
        }
        if !self.pr_contains_base {
            debug!("pr doesn't contain base");
            return SyncStatus::Restack;
        }
        SyncStatus::Synced
    }

    fn branch_name(change_id: &JujutsuChangeId, github_branch_prefix: &str) -> String {
        format!(
            "{github_branch_prefix}{}",
            &change_id.0[..GITHUB_CHANGE_ID_LENGTH.min(change_id.0.len())]
        )
    }

    pub fn message(&self) -> JujutsuCommitMessage {
        self.commit.message.clone()
    }

    pub fn full_message(&self) -> String {
        self.commit.full_message()
    }

    pub fn short_id(&self) -> String {
        let change_id = &self.commit.change_id;
        change_id.0[..4.min(change_id.0.len())].into()
    }
}
