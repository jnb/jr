use anyhow::Context;
use anyhow::bail;

use crate::App;

impl App {
    /// Create a new pull request.
    ///
    /// Define the "base branch" as the parent commit's PR branch (or main).
    ///
    /// 1. Create a new commit:
    ///    - Use this revision's filesystem snapshot as the commit contents.
    ///    - Use the base branch as the parent.
    /// 2. Push to a remote PR branch named after this revision's change ID.
    /// 3. Create a pull request to merge the PR branch into the base branch.
    pub async fn cmd_create(
        &self,
        revision: &str,
        stdout: &mut impl std::io::Write,
    ) -> anyhow::Result<()> {
        let commit = self.jj.get_commit(revision).await?;

        self.validate_not_merged_to_main(&commit).await?;
        self.check_parent_prs_up_to_date(revision).await?;

        let Some(pr_title) = &commit.message.title else {
            bail!("Cannot create PR with empty description");
        };
        let pr_body = commit.message.body.as_deref().unwrap_or("");

        let pr_branch = commit
            .change_id
            .branch_name(&self.config.github_branch_prefix);
        if self.git.get_branch_tip(&pr_branch).await.is_ok() {
            bail!("PR branch already exists: {pr_branch}");
        }
        let base_branch = self.find_previous_branch(revision).await?;

        let tree = self.git.get_tree(&commit.commit_id).await?;

        let parent_commit = self
            .git
            .get_branch_tip(&base_branch)
            .await
            .context(format!("Base branch {} does not exist", base_branch))?;

        let new_commit = self
            .git
            .commit_tree(&tree, vec![&parent_commit], &commit.full_message())
            .await?;

        self.git
            .push_commit_to_branch(&new_commit, &pr_branch)
            .await?;

        let pr_url = self
            .gh
            .pr_create(&pr_branch, &base_branch, pr_title, pr_body)
            .await?;
        writeln!(stdout, "Created PR: {}", pr_url)?;

        Ok(())
    }
}
