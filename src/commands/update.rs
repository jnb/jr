use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::App;

impl App {
    pub async fn cmd_update(
        &self,
        revision: &str,
        message: &str,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        let commit = self.jj.get_commit(revision).await?;

        self.validate_not_merged_to_main(&commit).await?;
        self.check_parent_prs_up_to_date(revision).await?;

        let pr_branch = commit
            .change_id
            .branch_name(&self.config.github_branch_prefix);
        let base_branch = self.find_previous_branch(revision).await?;

        let tree = self.git.get_tree(&commit.commit_id).await?;

        // PR branch must exist for update
        let _existing_pr_branch = self.git.get_branch_tip(&pr_branch).await.context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;

        // Get both parents for merge commit
        let old_pr_tip = self
            .git
            .get_branch_tip(&pr_branch)
            .await
            .context(format!("PR branch {} does not exist", pr_branch))?;
        let base_tip = self
            .git
            .get_branch_tip(&base_branch)
            .await
            .context(format!("Base branch {} does not exist", base_branch))?;

        // Use the provided commit message
        let commit_message = message;

        // Check if we need to create a new commit
        let old_pr_tree = self.git.get_tree(&old_pr_tip).await?;
        let base_has_changed = !self.git.is_ancestor(&base_tip, &old_pr_tip).await?;

        let new_commit = if tree == old_pr_tree && !base_has_changed {
            writeln!(
                stdout,
                "Tree unchanged and base hasn't moved, reusing old PR tip commit"
            )?;
            old_pr_tip.clone()
        } else if base_has_changed {
            // Create merge commit with old PR tip and base as parents
            let commit = self
                .git
                .commit_tree(&tree, vec![&old_pr_tip, &base_tip], commit_message)
                .await?;
            writeln!(stdout, "Created new merge commit: {}", commit)?;
            commit
        } else {
            // Tree changed but base hasn't - create regular commit with single parent
            let commit = self
                .git
                .commit_tree(&tree, vec![&old_pr_tip], commit_message)
                .await?;
            writeln!(stdout, "Created new commit: {}", commit)?;
            commit
        };

        if new_commit == old_pr_tip {
            bail!("No changes to push; PR is already up to date");
        }

        // Push commit directly to PR branch
        self.git
            .push_commit_to_branch(&new_commit, &pr_branch)
            .await?;
        writeln!(stdout, "Pushed PR branch {}", pr_branch)?;

        // Update PR base if needed
        let pr_url = if self.gh.pr_is_open(&pr_branch).await? {
            let url = self.gh.pr_edit(&pr_branch, &base_branch).await?;
            writeln!(
                stdout,
                "Updated PR for {} with base {}",
                pr_branch, base_branch
            )?;
            url
        } else {
            bail!(
                "No open PR found for PR branch {}. The PR may have been closed or merged.",
                pr_branch
            );
        };

        writeln!(stdout, "PR URL: {}", pr_url)?;

        Ok(())
    }
}
