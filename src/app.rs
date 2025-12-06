use anyhow::Result;
use anyhow::bail;

use crate::config::Config;
use crate::ops::git::RealGit;
use crate::ops::github::RealGithub;
use crate::ops::jujutsu::RealJujutsu;

/// Length of the change ID to use in GitHub branch names
pub const CHANGE_ID_LENGTH: usize = 8;

pub struct App {
    pub config: Config,
    pub gh: RealGithub,
    pub jj: RealJujutsu,
    pub git: RealGit,
}

impl App {
    pub fn new(config: Config, gh: RealGithub) -> Self {
        Self {
            config,
            gh,
            jj: RealJujutsu,
            git: RealGit,
        }
    }
}

/// Shared helper methods for App
impl App {
    /// Validate that a commit is not already merged to trunk
    pub(crate) async fn validate_not_merged_to_main(
        &self,
        commit: &crate::ops::jujutsu::Commit,
    ) -> Result<()> {
        let trunk_commit = self.jj.get_trunk_commit_id().await?;

        if self
            .jj
            .is_ancestor(&commit.commit_id.0, &trunk_commit)
            .await?
        {
            return Err(anyhow::anyhow!(
                "Cannot create PR: commit {} is an ancestor of trunk. This commit is already merged.",
                commit.commit_id
            ));
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
                .jj
                .get_git_remote_branches(&trunk_commit.change_id)
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
        let stack_changes = self.jj.get_stack_changes(&commit.commit_id.0).await?;

        // Fetch all branches once
        let all_branches = self
            .gh
            .find_branches_with_prefix(&self.config.github_branch_prefix)
            .await?;

        // Collect all branches that exist in the stack (excluding current revision)
        let branches_to_check: Vec<_> = stack_changes
            .iter()
            .filter(|(_, commit_id_in_stack)| commit_id_in_stack != &commit.commit_id)
            .filter_map(|(change_id, _commit_id_in_stack)| {
                let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
                let expected_branch =
                    format!("{}{}", self.config.github_branch_prefix, short_change_id);
                if all_branches.contains(&expected_branch) {
                    Some((
                        change_id.clone(),
                        _commit_id_in_stack.clone(),
                        expected_branch,
                    ))
                } else {
                    None
                }
            })
            .collect();

        // Fetch all pr_diffs in parallel
        let pr_diff_futures: Vec<_> = branches_to_check
            .iter()
            .map(|(_, _, branch)| async move {
                let diff = self.gh.pr_diff(branch).await;
                (branch.clone(), diff)
            })
            .collect();
        let pr_diff_results = futures_util::future::join_all(pr_diff_futures).await;

        // Check each change in the stack
        for ((change_id, _, expected_branch), (_, pr_diff_result)) in
            branches_to_check.iter().zip(pr_diff_results.iter())
        {
            // Get the commit for this change
            let commit_in_stack = self.jj.get_commit(change_id).await?;

            // Compare local single commit diff vs cumulative PR diff from GitHub
            let local_diff = self.git.get_commit_diff(&commit_in_stack.commit_id).await?;
            let pr_diff = pr_diff_result
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            if &local_diff != pr_diff {
                return Err(anyhow::anyhow!(
                    "Cannot update PR: parent PR {} is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    expected_branch
                ));
            }
        }

        Ok(())
    }
}
