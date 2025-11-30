use anyhow::Context;
use anyhow::Result;

use crate::ops::git::GitOps;
use crate::ops::github::GithubOps;
use crate::ops::jujutsu::JujutsuOps;
use crate::App;

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub async fn cmd_update(
        &self,
        revision: &str,
        message: &str,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get commit information from jj
        let commit = self.jj.get_commit(revision).await?;

        writeln!(stdout, "Change ID: {}", commit.change_id)?;
        writeln!(stdout, "Commit ID: {}", commit.commit_id)?;

        self.validate_not_merged_to_main(&commit).await?;
        self.check_parent_prs_up_to_date(revision).await?;

        // PR branch names: current and base
        let short_change_id = &commit.change_id[..self.config.change_id_length.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", self.config.branch_prefix, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix(&self.config.branch_prefix).await?;
        let base_branch = self.find_previous_branch(revision, &all_branches).await?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id).await?;
        writeln!(stdout, "Tree: {}", tree)?;

        // PR branch must exist for update
        let _existing_pr_branch = self.git.get_branch(&pr_branch).await.context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;

        writeln!(stdout, "PR branch {} exists", pr_branch)?;

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
            let commit = self.git.commit_tree_merge(
                &tree,
                vec![old_pr_tip.clone(), base_tip.clone()],
                commit_message,
            ).await?;
            writeln!(stdout, "Created new merge commit: {}", commit)?;
            commit
        } else {
            // Tree changed but base hasn't - create regular commit with single parent
            let commit = self.git.commit_tree(&tree, &old_pr_tip, commit_message).await?;
            writeln!(stdout, "Created new commit: {}", commit)?;
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
    use crate::app::tests::helpers::*;
    use crate::config::Config;
    use crate::ops::git::MockGitOps;
    use crate::ops::github::MockGithubOps;
    use crate::ops::jujutsu::Commit;
    use crate::ops::jujutsu::CommitMessage;
    use crate::ops::jujutsu::MockJujutsuOps;
    use crate::App;

    #[tokio::test]
    async fn test_cmd_update_updates_existing_pr() {
        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| match branch {
                "master" => Ok("main_commit".to_string()),
                "test/abc12345" => Ok("existing_commit".to_string()),
                _ => Err(anyhow::anyhow!("Branch not found")),
            });
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("tree123".to_string()));
        mock_git.expect_is_ancestor().returning(|_, _| Ok(true));
        mock_git.expect_update_branch().returning(|_, _| Ok(()));
        mock_git.expect_push_branch().returning(|_| Ok(()));

        let mut mock_gh = standard_gh_mock();
        mock_gh
            .expect_pr_edit()
            .withf(|pr_branch, base_branch| pr_branch == "test/abc12345" && base_branch == "master")
            .returning(|_, _| Ok("https://github.com/test/repo/pull/123".to_string()));

        let app = App::new(Config::default_for_tests(), standard_jj_mock(), mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_update("@", "Update from review", &mut stdout).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_update_errors_when_pr_is_closed() {
        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| match branch {
                "master" => Ok("main_commit".to_string()),
                "test/abc12345" => Ok("existing_commit".to_string()),
                _ => Err(anyhow::anyhow!("Branch not found")),
            });
        mock_git.expect_get_tree().returning(|commit_id| {
            if commit_id == "def45678" {
                Ok("new_tree".to_string())
            } else {
                Ok("old_tree".to_string())
            }
        });
        mock_git.expect_is_ancestor().returning(|_, _| Ok(true));
        mock_git
            .expect_commit_tree()
            .returning(|_, _, _| Ok("new_commit_obj".to_string()));
        mock_git.expect_update_branch().returning(|_, _| Ok(()));
        mock_git.expect_push_branch().returning(|_| Ok(()));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));
        mock_gh.expect_pr_is_open().returning(|_| Ok(false));

        let app = App::new(Config::default_for_tests(), standard_jj_mock(), mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app
            .cmd_update("@", "Update after review", &mut stdout)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No open PR found"));
    }

    #[tokio::test]
    async fn test_cmd_update_errors_when_parent_pr_is_outdated() {
        // Set up a stack: A (master) -> B -> C
        // B and C both have PRs
        // B's local diff != remote diff (outdated)
        // Trying to update C should fail
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|revision| match revision {
                "@" | "ccc12345" => Ok(Commit {
                    change_id: "ccc12345".to_string(),
                    commit_id: "commit_c_local".to_string(),
                    message: CommitMessage {
                        title: Some("Commit C message".to_string()),
                        body: None,
                    },
                    parent_change_ids: vec!["bbb12345".to_string()],
                }),
                "bbb12345" => Ok(Commit {
                    change_id: "bbb12345".to_string(),
                    commit_id: "commit_b_local".to_string(),
                    message: CommitMessage {
                        title: Some("Commit B message".to_string()),
                        body: None,
                    },
                    parent_change_ids: vec![],
                }),
                _ => Err(anyhow::anyhow!("Commit not found")),
            });
        mock_jj
            .expect_get_trunk_commit_id()
            .returning(|| Ok("trunk123".to_string()));
        mock_jj.expect_is_ancestor().returning(|_, _| Ok(false));
        mock_jj.expect_get_stack_changes().returning(|_| {
            Ok(vec![
                ("ccc12345".to_string(), "commit_c_local".to_string()),
                ("bbb12345".to_string(), "commit_b_local".to_string()),
            ])
        });

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|_| Ok("branch_commit".to_string()));
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("tree123".to_string()));
        mock_git
            .expect_get_commit_diff()
            .returning(|commit_id| {
                match commit_id {
                    "commit_b_local" => Ok("diff --git a/src/file.rs b/src/file.rs\n--- a/src/file.rs\n+++ b/src/file.rs\n@@ -1,1 +1,2 @@\n content\n+new line\ndiff --git a/src/new.rs b/src/new.rs\nnew file\n--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,1 @@\n+// new".to_string()),
                    _ => Ok("diff --git a/src/other.rs b/src/other.rs\n--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1,1 +1,2 @@\n content\n+change".to_string()),
                }
            });

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["test/bbb12345".to_string(), "test/ccc12345".to_string()]));
        mock_gh
            .expect_pr_diff()
            .returning(|branch| {
                if branch == "test/bbb12345" {
                    Ok("diff --git a/src/file.rs b/src/file.rs\n--- a/src/file.rs\n+++ b/src/file.rs\n@@ -1,1 +1,2 @@\n content\n+old line".to_string())
                } else {
                    Ok("diff --git a/src/other.rs b/src/other.rs\n--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1,1 +1,2 @@\n content\n+change".to_string())
                }
            });

        let app = App::new(Config::default_for_tests(), mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_update("@", "Update commit C", &mut stdout).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("parent PR"));
        assert!(error_msg.contains("out of date"));
        assert!(error_msg.contains("test/bbb12345"));
    }
}
