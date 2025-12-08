use std::path;
use std::sync::Arc;

use anyhow::Result;
use anyhow::bail;

use crate::clients::git::GitClient;
use crate::clients::github::GithubClient;
use crate::clients::jujutsu::JujutsuClient;
use crate::config::Config;
use crate::stack::Stack;

pub struct App {
    pub config: Arc<Config>,
    pub gh: Arc<GithubClient>,
    pub jj: Arc<JujutsuClient>,
    pub git: Arc<GitClient>,
}

impl App {
    pub fn new(config: Config, gh: GithubClient, path: path::PathBuf) -> Self {
        Self {
            config: Arc::new(config),
            gh: Arc::new(gh),
            jj: Arc::new(JujutsuClient::new(path.clone())),
            git: Arc::new(GitClient::new(path)),
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
        let parent_branch = parent_change_id.branch_name(&self.config.github_branch_prefix);
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

    /// Check if any parent PRs in the stack are outdated or need restacking.
    pub(crate) async fn check_parent_prs_up_to_date(&self, revision: &str) -> Result<()> {
        let commit = self.jj.get_commit(revision).await?;
        let stack_changes = self
            .jj
            .get_stack_ancestors_exclusive(&commit.commit_id.0)
            .await?;

        let stack = Stack::new(
            self.config.clone(),
            self.jj.clone(),
            self.gh.clone(),
            self.git.clone(),
            stack_changes.clone(),
        )
        .await?;

        for (_commit, status) in stack
            .commits
            .iter()
            .rev()
            .zip(stack.sync_statuses().await?.iter().rev())
        {
            match status {
                crate::stack::SyncStatus::Unknown => {
                    bail!("Parent commit has no PR branch. Create parent PR first (bottom-up).",);
                }
                crate::stack::SyncStatus::Restack => {
                    // bail!(
                    //     "Cannot update PR: parent PR {} needs restacking. Its base branch '{}' has been updated. Run 'jr restack' on the parent first.",
                    //     expected_branch,
                    //     base_branch
                    // );
                    bail!(
                        "Cannot update PR: parent PR needs restacking. Its base branch has been updated. Run 'jr restack' on the parent first.",
                    );
                }
                crate::stack::SyncStatus::Changed => {
                    // bail!(
                    //     "Cannot update PR: parent PR {} is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    //     expected_branch
                    // );
                    bail!(
                        "Cannot update PR: parent PR is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    );
                }
                crate::stack::SyncStatus::Synced => {}
            }
        }

        Ok(())
    }
}
