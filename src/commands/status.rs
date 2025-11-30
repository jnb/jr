use std::collections::HashMap;

use anyhow::Result;
use futures::future::join_all;

use crate::app::CHANGE_ID_LENGTH;
use crate::app::GLOBAL_BRANCH_PREFIX;
use crate::git::GitOps;
use crate::github::GithubOps;
use crate::jujutsu::JujutsuOps;
use crate::App;

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub async fn cmd_status(
        &self,
        stdout: &mut impl std::io::Write,
        stderr: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get the current commit to mark it in the output
        let current_commit = self.jj.get_commit("@").await?;

        // Find the head(s) of the current stack
        let heads = self.jj.get_stack_heads().await?;

        // Collect all changes to process
        let changes: Vec<(String, String)> = if heads.is_empty() {
            // Current commit is on trunk or no stack exists
            vec![(
                current_commit.change_id.clone(),
                current_commit.commit_id.clone(),
            )]
        } else if heads.len() == 1 {
            // Single head - show from head back to trunk
            let (_head_change_id, head_commit_id) = &heads[0];
            self.jj.get_stack_changes(head_commit_id).await?
        } else {
            // Multiple heads detected - show from @ to trunk with warning
            writeln!(
                stderr,
                "Warning: Multiple stack heads detected. Showing stack from @ to trunk."
            )?;
            self.jj.get_stack_changes("@").await?
        };

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;

        // Collect all unique branches we need pr_diffs for (changes + their parents)
        let mut branches_needing_diffs = std::collections::HashSet::new();
        for (change_id, _commit_id) in &changes {
            let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
            let expected_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);
            if all_branches.contains(&expected_branch) {
                branches_needing_diffs.insert(expected_branch);
            }

            // Also collect parent branches
            if let Ok(commit) = self.jj.get_commit(change_id).await {
                for parent_change_id in commit.parent_change_ids {
                    let short_parent_id =
                        &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
                    let parent_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_parent_id);
                    if all_branches.contains(&parent_branch) {
                        branches_needing_diffs.insert(parent_branch);
                    }
                }
            }
        }

        // Fetch all pr_diffs in parallel
        let pr_diff_futures: Vec<_> = branches_needing_diffs
            .iter()
            .map(|branch| async move {
                let diff = self.gh.pr_diff(branch).await;
                (branch.clone(), diff)
            })
            .collect();
        let pr_diff_results = join_all(pr_diff_futures).await;
        let pr_diffs: HashMap<String, String> = pr_diff_results
            .into_iter()
            .filter_map(|(branch, result)| result.ok().map(|diff| (branch, diff)))
            .collect();

        // Prepare tasks to fetch PR URLs concurrently
        let pr_url_futures: Vec<_> = changes
            .iter()
            .map(|(change_id, _commit_id)| {
                let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
                let expected_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);
                let branch_exists = all_branches.contains(&expected_branch);

                async move {
                    if branch_exists {
                        self.gh.pr_url(&expected_branch).await
                    } else {
                        Ok(None)
                    }
                }
            })
            .collect();

        // Fetch all PR URLs concurrently
        let pr_urls = join_all(pr_url_futures).await;

        // Get base branches concurrently
        let base_branch_futures: Vec<_> = changes
            .iter()
            .map(|(change_id, _commit_id)| self.find_previous_branch(change_id, &all_branches))
            .collect();
        let base_branches = join_all(base_branch_futures).await;

        // Display results
        for (i, (change_id, commit_id)) in changes.iter().enumerate() {
            let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
            let expected_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);
            let pr_url_result = &pr_urls[i];
            let base_branch_result = &base_branches[i];

            // Get commit and extract title from message
            let commit = self.jj.get_commit(commit_id).await?;
            let commit_title = commit.message.title.as_deref().unwrap_or("");

            // Get abbreviated change ID (4 chars, matching jj status default)
            let abbreviated_change_id = &change_id[..4.min(change_id.len())];

            // Check if parent PR is outdated
            let parent_pr_outdated =
                self.is_parent_pr_outdated(change_id, &all_branches, &pr_diffs).await?;

            // Get base branch (or default to empty string if error)
            let base_branch = base_branch_result.as_ref().ok();

            self.show_change_status_with_data(
                &expected_branch,
                change_id,
                commit_id,
                &current_commit.change_id,
                &all_branches,
                pr_url_result,
                commit_title,
                abbreviated_change_id,
                parent_pr_outdated,
                base_branch,
                &pr_diffs,
                stdout,
            )
            .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::helpers::*;
    use crate::git::MockGitOps;
    use crate::github::MockGithubOps;
    use crate::jujutsu::Commit;
    use crate::jujutsu::CommitMessage;
    use crate::jujutsu::MockJujutsuOps;
    use crate::App;

    #[tokio::test]
    async fn test_cmd_status_branch_exists_up_to_date() {
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
        mock_jj.expect_get_stack_heads().returning(|| Ok(vec![]));

        let mut mock_git = MockGitOps::new();
        mock_git.expect_get_branch().returning(|branch| {
            if branch == "jnb/abc12345" {
                Ok("remote_commit".to_string())
            } else {
                Err(anyhow::anyhow!("Branch not found"))
            }
        });
        mock_git
            .expect_get_commit_diff()
            .returning(|_| Ok("M\tsrc/main.rs".to_string()));
        mock_git.expect_is_ancestor().returning(|_, _| Ok(true));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh
            .expect_pr_url()
            .returning(|_| Ok(Some("https://github.com/test/repo/pull/123".to_string())));
        mock_gh
            .expect_pr_diff()
            .returning(|_| Ok("M\tsrc/main.rs".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());

        // Verify output is plain text without ANSI codes
        let output = String::from_utf8(stdout).unwrap();
        assert!(!output.contains("\x1b[")); // No ANSI escape sequences
        assert!(output.contains("✓ abc1 Test commit message"));
    }

    #[tokio::test]
    async fn test_cmd_status_branch_exists_out_of_sync() {
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
        mock_jj.expect_get_stack_heads().returning(|| Ok(vec![]));

        let mut mock_git = MockGitOps::new();
        mock_git.expect_get_branch().returning(|branch| {
            if branch == "jnb/abc12345" {
                Ok("remote_commit".to_string())
            } else {
                Err(anyhow::anyhow!("Branch not found"))
            }
        });
        mock_git.expect_get_commit_diff().returning(|commit_id| {
            if commit_id == "local_commit" {
                Ok("M\tsrc/main.rs\nA\tsrc/new.rs".to_string())
            } else {
                Ok("M\tsrc/main.rs".to_string())
            }
        });
        mock_git.expect_is_ancestor().returning(|_, _| Ok(true));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh
            .expect_pr_url()
            .returning(|_| Ok(Some("https://github.com/test/repo/pull/123".to_string())));
        mock_gh
            .expect_pr_diff()
            .returning(|_| Ok("M\tsrc/main.rs".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_status_branch_does_not_exist() {
        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/other123".to_string()]));
        mock_gh.expect_pr_url().returning(|_| Ok(None));

        let app = App::new(standard_jj_mock(), MockGitOps::new(), mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_status_branch_exists_no_pr() {
        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh.expect_pr_url().returning(|_| Ok(None));
        mock_gh.expect_pr_diff().returning(|_| Ok("".to_string()));

        let app = App::new(standard_jj_mock(), MockGitOps::new(), mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }
}
