use anyhow::Result;
use anyhow::bail;

use crate::App;
use crate::commit::CommitInfo;

impl App {
    /// Update a pull request in the case where (i) there are local changes, and
    /// (ii) the base branch may or may not have been updated.
    ///
    /// Define the "base branch" as the parent commit's PR branch (or main).
    ///
    /// 1. Create a commit:
    ///    - Use this revision's filesystem snapshot as the commit contents.
    ///    - If the base branch tip has changed, create a merge commit using the
    ///      old PR tip and the base branch tip as the two parents.  Else create
    ///      a regular commit using the base branch tip as the parent.
    /// 2. Push to the remote PR branch named after this revision's change ID.
    /// 3. Update the pull request's base branch.
    ///
    /// Note: When creating a merge commit we use the Jujutsu revision's tree
    /// directly, which reflects any conflict resolutions already made in
    /// Jujutsu, rather than computing a new merge via Git's merge machinery.
    pub async fn cmd_update(
        &self,
        revision: &str,
        message: &str,
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

        if commit.commit_diff_norm == commit.pr_diff_norm.expect("should be set") {
            if commit.pr_contains_base {
                bail!("No changes detected");
            } else {
                bail!("Commit unchanged; use 'jr restack' instead");
            }
        }

        let parents = if !commit.pr_contains_base {
            vec![pr_tip, commit.base_tip.expect("should be set")]
        } else {
            vec![pr_tip]
        };
        let tree = self.git.get_tree(&commit.commit.commit_id).await?;
        let new_commit = self
            .git
            .commit_tree(&tree, parents.iter().collect::<Vec<_>>(), message)
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
