use anyhow::bail;

use crate::App;
use crate::commit::CommitInfo;

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
        self.check_parent_prs_up_to_date(revision).await?;
        let commit = self.jj.get_commit(revision).await?;
        let commit = CommitInfo::new(commit, &self.config, &self.jj, &self.gh, &self.git).await?;
        if commit.pr_tip.is_some() {
            bail!("PR branch already exists: {}", commit.pr_branch);
        }

        let commit_message = commit.message();
        let Some(pr_title) = &commit_message.title else {
            bail!("Cannot create PR with empty description");
        };
        let pr_body = commit_message.body.as_deref().unwrap_or("");

        let tree = self.git.get_tree(&commit.commit.commit_id).await?;

        let new_commit = self
            .git
            .commit_tree(
                &tree,
                vec![&commit.base_tip.clone().expect("must exist")],
                &commit.full_message(),
            )
            .await?;

        self.git
            .push_commit_to_branch(&new_commit, &commit.pr_branch)
            .await?;

        let pr_url = self
            .gh
            .pr_create(&commit.pr_branch, &commit.base_branch, pr_title, pr_body)
            .await?;
        writeln!(stdout, "Created PR: {}", pr_url)?;

        Ok(())
    }
}
