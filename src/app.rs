use std::path;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;

use crate::clients::git::GitClient;
use crate::clients::github::GithubClient;
use crate::clients::jujutsu::JujutsuClient;
use crate::config::Config;

/// Length of the change ID to use in GitHub branch names
pub const CHANGE_ID_LENGTH: usize = 8;

pub struct App {
    pub config: Config,
    pub gh: GithubClient,
    pub jj: JujutsuClient,
    pub git: GitClient,
}

impl App {
    pub fn new(config: Config, gh: GithubClient, path: path::PathBuf) -> Self {
        Self {
            config,
            gh,
            jj: JujutsuClient::new(path.clone()),
            git: GitClient::new(path),
        }
    }
}

/// Shared helper methods for App
impl App {
    /// Validate that a commit is not already merged to trunk
    pub(crate) async fn validate_not_merged_to_main(
        &self,
        commit: &crate::clients::jujutsu::JujutsuCommit,
    ) -> Result<()> {
        let trunk_commit = self.jj.get_trunk().await?;

        if self
            .git
            .is_ancestor(&commit.commit_id, &trunk_commit.commit_id)
            .await?
        {
            bail!(
                "Cannot create PR: commit {} is an ancestor of trunk. This commit is already merged.",
                commit.commit_id
            );
        }

        Ok(())
    }

    /// Find the previous PR branch in the stack based on parent change IDs from jujutsu
    pub(crate) async fn find_previous_branch(&self, revision: &str) -> Result<String> {
        let commit = self.jj.get_commit(revision).await?;
        if commit.parent_change_ids.is_empty() {
            bail!("No parents found");
        }
        if commit.parent_change_ids.len() > 1 {
            bail!("Multiple parents found");
        }

        // Check if parent PR branch exists
        let parent_change_id = &commit.parent_change_ids[0];
        let short_parent_id = &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
        let parent_branch = format!("{}{}", self.config.github_branch_prefix, short_parent_id);
        if self.git.get_branch_tip(&parent_branch).await.is_ok() {
            return Ok(parent_branch);
        }

        // Check if parent is trunk
        let trunk_commit = self.jj.get_commit("trunk()").await?;
        if parent_change_id == &trunk_commit.change_id {
            let trunk_branches = self
                .git
                .get_git_remote_branches(&trunk_commit.commit_id)
                .await?;
            if trunk_branches.is_empty() {
                bail!("Trunk has no remote branch. Push trunk to remote first.");
            }
            return Ok(trunk_branches[0].clone());
        }

        bail!("Parent commit has no PR branch. Create parent PR first (bottom-up).")
    }

    /// Check if any parent PRs in the stack are outdated
    /// Returns an error if any parent PR has a local single commit diff that doesn't match cumulative remote diff
    pub(crate) async fn check_parent_prs_up_to_date(&self, revision: &str) -> Result<()> {
        // Get all changes in the stack from revision back to trunk
        let commit = self.jj.get_commit(revision).await?;
        let stack_changes = self.jj.get_stack_ancestors(&commit.commit_id.0).await?;

        // Fetch all branches once
        let all_branches = self
            .git
            .find_branches_with_prefix(&self.config.github_branch_prefix)
            .await?;

        // Collect all branches that exist in the stack (excluding current revision)
        let branches_to_check: Vec<_> = stack_changes
            .iter()
            .filter(|stack_commit| stack_commit.commit_id != commit.commit_id)
            .filter_map(|stack_commit| {
                let short_change_id =
                    &stack_commit.change_id[..CHANGE_ID_LENGTH.min(stack_commit.change_id.len())];
                let expected_branch =
                    format!("{}{}", self.config.github_branch_prefix, short_change_id);
                if all_branches.contains(&expected_branch) {
                    Some((stack_commit, expected_branch))
                } else {
                    None
                }
            })
            .collect();

        // Fetch all pr_diffs in parallel
        let pr_diff_futures: Vec<_> = branches_to_check
            .iter()
            .map(|(_, branch)| async move {
                let diff = self.gh.pr_diff(branch).await;
                (branch.clone(), diff)
            })
            .collect();
        let pr_diff_results = futures_util::future::join_all(pr_diff_futures).await;

        // Check each change in the stack
        for ((stack_commit, expected_branch), (_, pr_diff_result)) in
            branches_to_check.iter().zip(pr_diff_results.iter())
        {
            // Get the commit for this change
            let commit_in_stack = self.jj.get_commit(&stack_commit.change_id).await?;

            // Compare local single commit diff vs cumulative PR diff from GitHub
            let local_diff = self.git.get_commit_diff(&commit_in_stack.commit_id).await?;
            let pr_diff = pr_diff_result.as_ref().map_err(|e| anyhow!("{}", e))?;

            if &local_diff != pr_diff {
                bail!(
                    "Cannot update PR: parent PR {} is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    expected_branch
                );
            }
        }

        Ok(())
    }
}
