use anyhow::Result;
use anyhow::bail;

use crate::App;
use crate::commit::CommitInfo;

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
        self.check_parent_prs_up_to_date(revision).await?;

        let commit = self.jj.get_commit(revision).await?;
        let commit = CommitInfo::new(commit, &self.config, &self.jj, &self.gh, &self.git).await?;

        let Some(pr_tip) = commit.pr_tip else {
            bail!(
                "PR branch {} does not exist. Use 'jr create' to create a new PR.",
                commit.pr_branch
            );
        };

        if !self.gh.pr_is_open(&commit.pr_branch).await? {
            bail!(
                "No open PR found for branch {}. The PR may have been closed or merged.",
                commit.pr_branch
            );
        }

        if commit.commit_diff_norm != commit.pr_diff_norm.expect("pr branch exists") {
            bail!(concat!(
                "Cannot restack: commit has local changes.\n",
                "Use 'jr update -m \"<message>\"' to update with your changes."
            ));
        }

        if commit.pr_contains_base {
            bail!("Base hasn't changed; no need to restack");
        }

        let tree = self.git.get_tree(&commit.commit.commit_id).await?;
        let commit_message = "Merge";
        let new_commit = self
            .git
            .commit_tree(
                &tree,
                vec![&pr_tip, &commit.base_tip.expect("should be set")],
                commit_message,
            )
            .await?;

        self.git
            .push_commit_to_branch(&new_commit, &commit.pr_branch)
            .await?;

        let pr_url = self
            .gh
            .pr_edit(&commit.pr_branch, &commit.base_branch)
            .await?;
        writeln!(stdout, "Updated PR: {}", pr_url)?;

        Ok(())
    }
}
