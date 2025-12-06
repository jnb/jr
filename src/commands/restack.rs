use anyhow::Context;
use anyhow::Result;

use crate::App;
use crate::app::CHANGE_ID_LENGTH;
use crate::diff_utils::normalize_diff;

impl App {
    pub async fn cmd_restack(
        &self,
        revision: &str,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get commit information from jj
        let commit = self.jj.get_commit(revision).await?;

        writeln!(stdout, "Change ID: {}", commit.change_id)?;
        writeln!(stdout, "Commit ID: {}", commit.commit_id)?;

        self.validate_not_merged_to_main(&commit).await?;
        self.check_parent_prs_up_to_date(revision).await?;

        // PR branch names: current and base
        let short_change_id = &commit.change_id[..CHANGE_ID_LENGTH.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", self.config.github_branch_prefix, short_change_id);
        let base_branch = self.find_previous_branch(revision).await?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id).await?;
        writeln!(stdout, "Tree: {}", tree)?;

        // PR branch must exist for restack
        let _existing_pr_branch = self.git.get_branch_tip(&pr_branch).await.context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;

        writeln!(stdout, "PR branch {} exists", pr_branch)?;

        // Check if this is a pure restack (no local changes)
        let local_change_diff = self.git.get_commit_diff(&commit.commit_id).await?;
        let pr_cumulative_diff = self.gh.pr_diff(&pr_branch).await?;

        // Normalize both diffs to ignore differences in index line hash lengths
        let normalized_local = normalize_diff(&local_change_diff);
        let normalized_pr = normalize_diff(&pr_cumulative_diff);

        if normalized_local != normalized_pr {
            return Err(anyhow::anyhow!(
                "Cannot restack: commit has local changes. Use 'jr update -m \"<message>\"' to update with your changes."
            ));
        }

        writeln!(stdout, "Detected pure restack (no changes to this commit)")?;

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

        let commit_message = "Restack";

        // Check if we need to create a new commit
        let old_pr_tree = self.git.get_tree(&old_pr_tip).await?;
        let base_has_changed = !self.git.is_ancestor(&base_tip, &old_pr_tip).await?;

        let new_commit = if tree == old_pr_tree && !base_has_changed {
            writeln!(
                stdout,
                "Tree unchanged and base hasn't moved, reusing old PR tip commit"
            )?;
            old_pr_tip.clone()
        } else {
            // Create merge commit with old PR tip and base as parents
            let commit = self
                .git
                .commit_tree_merge(
                    &tree,
                    vec![old_pr_tip.clone(), base_tip.clone()],
                    commit_message,
                )
                .await?;
            writeln!(stdout, "Created new merge commit: {}", commit)?;
            commit
        };

        // Only update if there are actual changes
        if new_commit == old_pr_tip {
            writeln!(stdout, "No changes to push - PR is already up to date")?;
            return Ok(());
        }

        // Update PR branch to point to new commit
        self.git.update_branch(&pr_branch, &new_commit).await?;
        writeln!(stdout, "Updated PR branch {}", pr_branch)?;

        // Push PR branch
        self.git.push_branch(&pr_branch).await?;
        writeln!(stdout, "Pushed PR branch {}", pr_branch)?;

        self.git.delete_local_branch(&pr_branch).await?;
        writeln!(stdout, "Deleted local branch {}", pr_branch)?;

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
            return Err(anyhow::anyhow!(
                "No open PR found for PR branch {}. The PR may have been closed or merged.",
                pr_branch
            ));
        };

        writeln!(stdout, "PR URL: {}", pr_url)?;

        Ok(())
    }
}
