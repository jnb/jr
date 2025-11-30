use anyhow::Context;
use anyhow::Result;

use crate::app::CHANGE_ID_LENGTH;
use crate::ops::git::GitOps;
use crate::ops::github::GithubOps;
use crate::ops::jujutsu::JujutsuOps;
use crate::App;

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub async fn cmd_create(&self, revision: &str, stdout: &mut impl std::io::Write) -> Result<()> {
        // Get commit information from jj
        let commit = self.jj.get_commit(revision).await?;

        // Validate commit message is not empty
        if commit.message.title.is_none() {
            return Err(anyhow::anyhow!(
                "Cannot create PR: commit has empty description. Add a description with 'jj describe'."
            ));
        }

        let pr_title = commit.message.title.as_deref().unwrap_or("");
        let pr_body = commit.message.body.as_deref().unwrap_or("");

        writeln!(stdout, "Change ID: {}", commit.change_id)?;
        writeln!(stdout, "Commit ID: {}", commit.commit_id)?;

        self.validate_not_merged_to_main(&commit).await?;

        // PR branch names: current and base
        let short_change_id = &commit.change_id[..CHANGE_ID_LENGTH.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", self.config.branch_prefix, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix(&self.config.branch_prefix).await?;
        let base_branch = self.find_previous_branch(revision, &all_branches).await?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id).await?;
        writeln!(stdout, "Tree: {}", tree)?;

        // Check if PR branch already exists - if so, check if it's up to date
        if let Ok(existing_branch_tip) = self.git.get_branch(&pr_branch).await {
            let existing_tree = self.git.get_tree(&existing_branch_tip).await?;
            if tree == existing_tree {
                return Err(anyhow::anyhow!(
                    "PR branch {} already exists and is up to date.",
                    pr_branch
                ));
            } else {
                return Err(anyhow::anyhow!(
                    "PR branch {} already exists with different content. Use 'jr update -m \"message\"' to update it.",
                    pr_branch
                ));
            }
        }

        // Use base branch as parent for new PR
        let parent = self
            .git
            .get_branch(&base_branch)
            .await
            .context(format!("Base branch {} does not exist", base_branch))?;

        // Create new commit with jj commit message
        let new_commit = self
            .git
            .commit_tree(&tree, &parent, &commit.full_message())
            .await?;
        writeln!(stdout, "Created new commit: {}", new_commit)?;

        // Update PR branch to point to new commit
        self.git.update_branch(&pr_branch, &new_commit).await?;
        writeln!(stdout, "Updated PR branch {}", pr_branch)?;

        // Push PR branch
        self.git.push_branch(&pr_branch).await?;
        writeln!(stdout, "Pushed PR branch {}", pr_branch)?;

        // Create PR
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
    async fn test_cmd_create_creates_new_pr() {
        let mut mock_gh = standard_gh_mock();
        mock_gh
            .expect_pr_create()
            .withf(|pr_branch, base_branch, _title, _body| {
                pr_branch == "test/abc12345" && base_branch == "master"
            })
            .returning(|_, _, _, _| Ok("https://github.com/test/repo/pull/123".to_string()));

        let app = App::new(Config::default_for_tests(), standard_jj_mock(), standard_git_mock(), mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_ok());

        // Verify output
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("Change ID: abc12345"));
        assert!(output.contains("PR branch: test/abc12345"));
        assert!(output.contains("Base branch: master"));
    }

    #[tokio::test]
    async fn test_cmd_create_rejects_ancestor_of_main() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj.expect_get_commit().returning(|_| {
            Ok(Commit {
                change_id: "abc12345".to_string(),
                commit_id: "old_commit".to_string(),
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
        mock_jj
            .expect_is_ancestor()
            .withf(|commit, descendant| commit == "old_commit" && descendant == "trunk123")
            .returning(|_, _| Ok(true));

        let app = App::new(Config::default_for_tests(), mock_jj, MockGitOps::new(), MockGithubOps::new());

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ancestor of trunk"));
    }

    #[tokio::test]
    async fn test_cmd_create_accepts_non_ancestor() {
        let app = App::new(Config::default_for_tests(), standard_jj_mock(), standard_git_mock(), standard_gh_mock());

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_branch_exists_and_up_to_date() {
        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("same_tree".to_string()));
        mock_git
            .expect_get_branch()
            .returning(|branch| match branch {
                "master" => Ok("main_commit".to_string()),
                "test/abc12345" => Ok("existing_commit".to_string()),
                _ => Err(anyhow::anyhow!("Branch not found")),
            });

        let app = App::new(Config::default_for_tests(), standard_jj_mock(), mock_git, standard_gh_mock());

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("up to date"));
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_branch_exists_with_different_content() {
        let mut mock_git = MockGitOps::new();
        mock_git.expect_get_tree().returning(|commit| {
            if commit == "def45678" {
                Ok("new_tree".to_string())
            } else {
                Ok("old_tree".to_string())
            }
        });
        mock_git
            .expect_get_branch()
            .returning(|branch| match branch {
                "master" => Ok("main_commit".to_string()),
                "test/abc12345" => Ok("existing_commit".to_string()),
                _ => Err(anyhow::anyhow!("Branch not found")),
            });

        let app = App::new(Config::default_for_tests(), standard_jj_mock(), mock_git, standard_gh_mock());

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("different content"));
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_description_is_empty() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj.expect_get_commit().returning(|_| {
            Ok(Commit {
                change_id: "abc12345".to_string(),
                commit_id: "def45678".to_string(),
                message: CommitMessage {
                    title: None,
                    body: None,
                },
                parent_change_ids: vec![],
            })
        });

        let app = App::new(Config::default_for_tests(), mock_jj, MockGitOps::new(), MockGithubOps::new());

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("empty description"));
    }
}
