use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::App;
use crate::diff_utils::normalize_diff;

impl App {
    /// Update a pull request in the case where (i) there are no local changes,
    /// and (ii) the base branch has been updated.
    ///
    /// Define the "base branch" as the parent commit's PR branch (or main).
    ///
    /// 1. Create a merge commit:
    ///    - Use this revision's filesystem snapshot as the commit contents.
    ///    - Use the old PR tip and the base branch tip as the two parents.
    /// 2. Push to the remote PR branch named after this revision's change ID.
    /// 3. Update the pull request's base branch.
    ///
    /// Note: The merge commit uses the Jujutsu revision's tree directly, which
    /// reflects any conflict resolutions already made in Jujutsu, rather than
    /// computing a new merge via Git's merge machinery.
    pub async fn cmd_restack(
        &self,
        revision: &str,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        let commit = self.jj.get_commit(revision).await?;

        self.validate_not_merged_to_main(&commit).await?;
        self.check_parent_prs_up_to_date(revision).await?;

        let pr_branch = commit
            .change_id
            .branch_name(&self.config.github_branch_prefix);
        let old_pr_tip = self.git.get_branch_tip(&pr_branch).await.context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;
        if !self.gh.pr_is_open(&pr_branch).await? {
            bail!(
                "No open PR found for branch {}. The PR may have been closed or merged.",
                pr_branch
            );
        }
        let base_branch = self.find_previous_branch(revision).await?;

        let tree = self.git.get_tree(&commit.commit_id).await?;

        let commit_diff = self.git.get_commit_diff(&commit.commit_id).await?;
        let pr_diff = self.gh.pr_diff(&pr_branch).await?;
        if normalize_diff(&commit_diff) != normalize_diff(&pr_diff) {
            bail!(
                "Cannot restack: commit has local changes.\n
                Use 'jr update -m \"<message>\"' to update with your changes."
            );
        }

        let base_tip = self
            .git
            .get_branch_tip(&base_branch)
            .await
            .context(format!("Base branch {} does not exist", base_branch))?;

        let base_has_changed = !self.git.is_ancestor(&base_tip, &old_pr_tip).await?;
        if !base_has_changed {
            bail!("Base hasn't changed; no need to restack");
        }

        let commit_message = "Merge";
        let new_commit = self
            .git
            .commit_tree(&tree, vec![&old_pr_tip, &base_tip], commit_message)
            .await?;

        self.git
            .push_commit_to_branch(&new_commit, &pr_branch)
            .await?;

        let pr_url = self.gh.pr_edit(&pr_branch, &base_branch).await?;
        writeln!(stdout, "Updated PR: {}", pr_url)?;

        Ok(())
    }
}
