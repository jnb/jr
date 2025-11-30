use anyhow::Context;
use anyhow::Result;

use crate::app::CHANGE_ID_LENGTH;
use crate::app::GLOBAL_BRANCH_PREFIX;
use crate::ops::git::GitOps;
use crate::ops::github::GithubOps;
use crate::ops::jujutsu::JujutsuOps;
use crate::App;

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
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
        let pr_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;
        let base_branch = self.find_previous_branch(revision, &all_branches).await?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id).await?;
        writeln!(stdout, "Tree: {}", tree)?;

        // PR branch must exist for restack
        let _existing_pr_branch = self.git.get_branch(&pr_branch).await.context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;

        writeln!(stdout, "PR branch {} exists", pr_branch)?;

        // Check if this is a pure restack (no local changes)
        let local_change_diff = self.git.get_commit_diff(&commit.commit_id).await?;
        let pr_cumulative_diff = self.gh.pr_diff(&pr_branch).await?;

        if local_change_diff != pr_cumulative_diff {
            return Err(anyhow::anyhow!(
                "Cannot restack: commit has local changes. Use 'jr update -m \"<message>\"' to update with your changes."
            ));
        }

        writeln!(stdout, "Detected pure restack (no changes to this commit)")?;

        // Get both parents for merge commit
        let old_pr_tip = self
            .git
            .get_branch(&pr_branch)
            .await
            .context(format!("PR branch {} does not exist", pr_branch))?;
        let base_tip = self
            .git
            .get_branch(&base_branch)
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
            let commit = self.git.commit_tree_merge(
                &tree,
                vec![old_pr_tip.clone(), base_tip.clone()],
                commit_message,
            ).await?;
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

#[cfg(test)]
mod tests {
    use crate::ops::git::MockGitOps;
    use crate::ops::github::MockGithubOps;
    use crate::ops::jujutsu::Commit;
    use crate::ops::jujutsu::CommitMessage;
    use crate::ops::jujutsu::MockJujutsuOps;
    use crate::App;

    #[tokio::test]
    async fn test_cmd_restack_works_when_diffs_match() {
        // Set up a commit where the diff introduced by each commit matches (pure restack)
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj.expect_get_commit().returning(|_| {
            Ok(Commit {
                change_id: "abc12345".to_string(),
                commit_id: "local_commit".to_string(),
                message: CommitMessage {
                    title: Some("Test commit message".to_string()),
                    body: None,
                },
                parent_change_ids: vec![],
            })
        });
        mock_jj
            .expect_get_trunk_commit_id()
            .returning(|| Ok("trunk123".to_string()));
        mock_jj.expect_is_ancestor().returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![("abc12345".to_string(), "local_commit".to_string())]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| match branch {
                "master" => Ok("main_commit".to_string()),
                "jnb/abc12345" => Ok("remote_commit".to_string()),
                _ => Err(anyhow::anyhow!("Branch not found")),
            });
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("same_tree".to_string()));
        mock_git
            .expect_get_commit_diff()
            .returning(|_| Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment".to_string()));
        mock_git.expect_is_ancestor().returning(|_, _| Ok(true));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh
            .expect_pr_diff()
            .returning(|_| Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment".to_string()));
        mock_gh.expect_pr_is_open().returning(|_| Ok(true));
        mock_gh
            .expect_pr_edit()
            .returning(|_, _| Ok("https://github.com/test/repo/pull/123".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        // Restack should succeed when diffs match
        let mut stdout = Vec::new();
        let result = app.cmd_restack("@", &mut stdout).await;
        if let Err(e) = &result {
            eprintln!("Error: {}", e);
        }
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_restack_errors_when_diffs_differ() {
        // Set up a commit where the diff introduced differs (has local changes)
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj.expect_get_commit().returning(|_| {
            Ok(Commit {
                change_id: "abc12345".to_string(),
                commit_id: "local_commit".to_string(),
                message: CommitMessage {
                    title: Some("Test commit message".to_string()),
                    body: None,
                },
                parent_change_ids: vec![],
            })
        });
        mock_jj
            .expect_get_trunk_commit_id()
            .returning(|| Ok("trunk123".to_string()));
        mock_jj.expect_is_ancestor().returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![("abc12345".to_string(), "local_commit".to_string())]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| match branch {
                "master" => Ok("main_commit".to_string()),
                "jnb/abc12345" => Ok("remote_commit".to_string()),
                _ => Err(anyhow::anyhow!("Branch not found")),
            });
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("tree123".to_string()));
        mock_git
            .expect_get_commit_diff()
            .returning(|commit_id| {
                if commit_id == "local_commit" {
                    Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment\ndiff --git a/src/new.rs b/src/new.rs\nnew file\n--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,1 @@\n+// new file".to_string())
                } else {
                    Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment".to_string())
                }
            });

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh
            .expect_pr_diff()
            .returning(|_| Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        // Restack should error when commit has local changes
        let mut stdout = Vec::new();
        let result = app.cmd_restack("@", &mut stdout).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("local changes"));
    }
}
