use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::App;
use crate::app::CHANGE_ID_LENGTH;

impl App {
    pub async fn cmd_create(&self, revision: &str, stdout: &mut impl std::io::Write) -> Result<()> {
        let commit = self.jj.get_commit(revision).await?;

        let Some(pr_title) = &commit.message.title else {
            bail!(
                "Cannot create PR: commit has empty description. Add a description with 'jj describe'."
            );
        };
        let pr_body = commit.message.body.as_deref().unwrap_or("");

        writeln!(stdout, "Change ID: {}", commit.change_id)?;
        writeln!(stdout, "Commit ID: {}", commit.commit_id)?;

        self.validate_not_merged_to_main(&commit).await?;

        let short_change_id = &commit.change_id[..CHANGE_ID_LENGTH.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", self.config.github_branch_prefix, short_change_id);
        let base_branch = self.find_previous_branch(revision).await?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        let tree = self.git.get_tree(&commit.commit_id).await?;
        writeln!(stdout, "Tree: {}", tree)?;

        if let Ok(existing_branch_tip) = self.git.get_branch_tip(&pr_branch).await {
            let existing_tree = self.git.get_tree(&existing_branch_tip).await?;
            if tree == existing_tree {
                bail!("PR branch {} already exists and is up to date.", pr_branch);
            } else {
                bail!(
                    "PR branch {} already exists with different content. Use 'jr update -m \"message\"' to update it.",
                    pr_branch
                );
            }
        }

        // Use base branch as parent for new PR
        let parent = self
            .git
            .get_branch_tip(&base_branch)
            .await
            .context(format!("Base branch {} does not exist", base_branch))?;

        let new_commit = self
            .git
            .commit_tree(&tree, vec![&parent], &commit.full_message())
            .await?;
        writeln!(stdout, "Created new commit: {}", new_commit)?;

        self.git.update_branch(&pr_branch, &new_commit).await?;
        writeln!(stdout, "Updated PR branch {}", pr_branch)?;

        self.git.push_branch(&pr_branch).await?;
        writeln!(stdout, "Pushed PR branch {}", pr_branch)?;

        self.git.delete_local_branch(&pr_branch).await?;
        writeln!(stdout, "Deleted local branch {}", pr_branch)?;

        let pr_url = self
            .gh
            .pr_create(&pr_branch, &base_branch, pr_title, pr_body)
            .await?;
        writeln!(
            stdout,
            "Created PR for {} with base {}",
            pr_branch, base_branch
        )?;
        writeln!(stdout, "PR URL: {}", pr_url)?;

        Ok(())
    }
}
