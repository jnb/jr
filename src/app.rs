use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;

use crate::config::Config;
use crate::ops::git::GitOps;
use crate::ops::github::GithubOps;
use crate::ops::jujutsu::JujutsuOps;

/// Length of the change ID to use in branch names
pub const CHANGE_ID_LENGTH: usize = 8;

pub struct App<J: JujutsuOps, G: GitOps, H: GithubOps> {
    pub config: Config,
    pub jj: J,
    pub git: G,
    pub gh: H,
}

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub fn new(config: Config, jj: J, git: G, gh: H) -> Self {
        Self { config, jj, git, gh }
    }
}

/// Shared helper methods for App
impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    /// Validate that a commit is not already merged to trunk
    pub(crate) async fn validate_not_merged_to_main(
        &self,
        commit: &crate::ops::jujutsu::Commit,
    ) -> Result<()> {
        let trunk_commit = self.jj.get_trunk_commit_id().await?;

        if self.jj.is_ancestor(&commit.commit_id, &trunk_commit).await? {
            return Err(anyhow::anyhow!(
                "Cannot create PR: commit {} is an ancestor of trunk. This commit is already merged.",
                commit.commit_id
            ));
        }

        Ok(())
    }

    /// Find the previous PR branch in the stack based on parent change IDs from jujutsu
    pub(crate) async fn find_previous_branch(
        &self,
        revision: &str,
        all_branches: &[String],
    ) -> Result<String> {
        // Get parent change IDs from commit
        let commit = self.jj.get_commit(revision).await?;
        let parent_change_ids = commit.parent_change_ids;

        // For each parent, check if a PR branch exists
        for parent_change_id in parent_change_ids {
            let short_parent_id = &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
            let parent_branch = format!("{}{}", self.config.branch_prefix, short_parent_id);

            // Check if this PR branch exists
            if all_branches.contains(&parent_branch) {
                return Ok(parent_branch);
            }
        }

        // Default to master if no parent PR branch found
        Ok("master".to_string())
    }

    /// Check if any parent PRs in the stack are outdated
    /// Returns an error if any parent PR has a local single commit diff that doesn't match cumulative remote diff
    pub(crate) async fn check_parent_prs_up_to_date(&self, revision: &str) -> Result<()> {
        // Get all changes in the stack from revision back to trunk
        let commit = self.jj.get_commit(revision).await?;
        let stack_changes = self.jj.get_stack_changes(&commit.commit_id).await?;

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix(&self.config.branch_prefix).await?;

        // Collect all branches that exist in the stack (excluding current revision)
        let branches_to_check: Vec<_> = stack_changes
            .iter()
            .filter(|(_, commit_id_in_stack)| commit_id_in_stack != &commit.commit_id)
            .filter_map(|(change_id, _commit_id_in_stack)| {
                let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
                let expected_branch = format!("{}{}", self.config.branch_prefix, short_change_id);
                if all_branches.contains(&expected_branch) {
                    Some((
                        change_id.clone(),
                        _commit_id_in_stack.clone(),
                        expected_branch,
                    ))
                } else {
                    None
                }
            })
            .collect();

        // Fetch all pr_diffs in parallel
        let pr_diff_futures: Vec<_> = branches_to_check
            .iter()
            .map(|(_, _, branch)| async move {
                let diff = self.gh.pr_diff(branch).await;
                (branch.clone(), diff)
            })
            .collect();
        let pr_diff_results = futures_util::future::join_all(pr_diff_futures).await;

        // Check each change in the stack
        for ((change_id, _, expected_branch), (_, pr_diff_result)) in
            branches_to_check.iter().zip(pr_diff_results.iter())
        {
            // Get the commit for this change
            let commit_in_stack = self.jj.get_commit(change_id).await?;

            // Compare local single commit diff vs cumulative PR diff from GitHub
            let local_diff = self.git.get_commit_diff(&commit_in_stack.commit_id).await?;
            let pr_diff = pr_diff_result
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            if &local_diff != pr_diff {
                return Err(anyhow::anyhow!(
                    "Cannot update PR: parent PR {} is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    expected_branch
                ));
            }
        }

        Ok(())
    }

    /// Check if any parent PR is out of date
    /// A parent is outdated if its local single commit diff differs from cumulative remote diff,
    /// or if the parent itself needs a restack (recursive check)
    pub(crate) async fn is_parent_pr_outdated(
        &self,
        revision: &str,
        all_branches: &[String],
        pr_diffs: &HashMap<String, String>,
    ) -> Result<bool> {
        // Get parent change IDs from commit
        let commit = self.jj.get_commit(revision).await?;
        let parent_change_ids = commit.parent_change_ids;

        // For each parent, check if it has a PR and if it's outdated
        for parent_change_id in parent_change_ids {
            let short_parent_id = &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
            let parent_branch = format!("{}{}", self.config.branch_prefix, short_parent_id);

            // If this parent has a PR branch, check if it's outdated
            if all_branches.contains(&parent_branch) {
                // Compare parent's local single commit diff vs cumulative PR diff from cache
                let parent_commit = self.jj.get_commit(&parent_change_id).await?;
                let parent_local_diff = self.git.get_commit_diff(&parent_commit.commit_id).await?;

                if let Some(parent_pr_diff) = pr_diffs.get(&parent_branch) {
                    if &parent_local_diff != parent_pr_diff {
                        return Ok(true); // Parent has local changes
                    }
                }

                // Check if parent's base has moved
                if let Ok(parent_base_branch) =
                    self.find_previous_branch(&parent_change_id, all_branches).await
                {
                    if let Ok(parent_pr_commit) = self.git.get_branch(&parent_branch).await {
                        if let Ok(base_tip) = self.git.get_branch(&parent_base_branch).await {
                            // If base is not an ancestor of parent's PR, base has moved
                            if !self.git.is_ancestor(&base_tip, &parent_pr_commit).await? {
                                return Ok(true); // Parent's base has moved, needs restack
                            }
                        }
                    }
                }

                // Recursively check if the parent itself needs a restack
                if Box::pin(self.is_parent_pr_outdated(&parent_change_id, all_branches, pr_diffs))
                    .await?
                {
                    return Ok(true); // Parent's ancestor is outdated
                }
            }
        }

        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn show_change_status_with_data(
        &self,
        expected_branch: &str,
        change_id: &str,
        commit_id: &str,
        current_change_id: &str,
        all_branches: &[String],
        pr_url_result: &Result<Option<String>>,
        commit_title: &str,
        abbreviated_change_id: &str,
        parent_pr_outdated: bool,
        _base_branch: Option<&String>,
        pr_diffs: &HashMap<String, String>,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        let is_current = change_id == current_change_id;
        let branch_exists = all_branches.contains(&expected_branch.to_string());

        // Determine status symbol
        let status_symbol = if branch_exists {
            // Check if PR exists
            match pr_url_result {
                Ok(Some(_)) => {
                    // Compare local single commit diff vs cumulative PR diff from cache
                    let local_diff = self.git.get_commit_diff(commit_id).await;

                    match local_diff {
                        Ok(local_diff) => {
                            if let Some(pr_diff) = pr_diffs.get(expected_branch) {
                                // Check if base branch has moved (not an ancestor of PR branch)
                                let base_has_moved = if let Some(base_branch) = _base_branch {
                                    // Get PR branch tip
                                    if let Ok(pr_branch_tip) = self.git.get_branch(expected_branch).await
                                    {
                                        // Get base branch tip
                                        if let Ok(base_tip) = self.git.get_branch(base_branch).await {
                                            // If base is not an ancestor of PR, base has moved
                                            !self
                                                .git
                                                .is_ancestor(&base_tip, &pr_branch_tip)
                                                .await
                                                .unwrap_or(false)
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if &local_diff == pr_diff {
                                    // Diffs match - check if parent changed or base moved
                                    if parent_pr_outdated || base_has_moved {
                                        "↻" // Needs restack (parent changed or base moved)
                                    } else {
                                        "✓" // Up to date
                                    }
                                } else {
                                    "✗" // Has local changes
                                }
                            } else {
                                "?"
                            }
                        }
                        _ => "?",
                    }
                }
                Ok(None) | Err(_) => "?",
            }
        } else {
            "?"
        };

        // Display status symbol + abbreviated change ID (cyan) + title (white) on first line
        let change_id_colored = abbreviated_change_id.cyan();
        if is_current {
            let out = format!(
                "{} {} {}",
                status_symbol,
                change_id_colored,
                commit_title.white().bold(),
            );
            writeln!(stdout, "{}", out.trim_end())?;
        } else {
            let out = format!(
                "{} {} {}",
                status_symbol,
                change_id_colored,
                commit_title.white(),
            );
            writeln!(stdout, "{}", out.trim_end())?;
        }

        // Display URL on second line if PR exists (dimmed to be less prominent)
        if let Ok(Some(pr_url)) = pr_url_result {
            let url_line = format!("  {}", pr_url);
            writeln!(stdout, "{}", url_line.dimmed())?;
        }

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers {
        use crate::ops::git::MockGitOps;
        use crate::ops::github::MockGithubOps;
        use crate::ops::jujutsu::Commit;
        use crate::ops::jujutsu::CommitMessage;
        use crate::ops::jujutsu::MockJujutsuOps;

        /// Returns a MockJujutsuOps with sensible defaults for a typical commit
        pub fn standard_jj_mock() -> MockJujutsuOps {
            let mut mock = MockJujutsuOps::new();
            mock.expect_get_commit()
                .returning(|_| Ok(standard_commit()));
            mock.expect_get_trunk_commit_id()
                .returning(|| Ok("trunk123".to_string()));
            mock.expect_is_ancestor().returning(|_, _| Ok(false));
            mock.expect_get_stack_changes().returning(|_| Ok(vec![]));
            mock.expect_get_stack_heads().returning(|| Ok(vec![]));
            mock
        }

        /// Returns a standard test commit
        pub fn standard_commit() -> Commit {
            Commit {
                change_id: "abc12345".to_string(),
                commit_id: "def45678".to_string(),
                message: CommitMessage {
                    title: Some("Test commit message".to_string()),
                    body: None,
                },
                parent_change_ids: vec![],
            }
        }

        /// Standard git mock with master branch
        pub fn standard_git_mock() -> MockGitOps {
            let mut mock = MockGitOps::new();
            mock.expect_get_tree()
                .returning(|_| Ok("tree123".to_string()));
            mock.expect_get_branch().returning(|b| {
                if b == "master" {
                    Ok("base_commit".to_string())
                } else {
                    Err(anyhow::anyhow!("Branch not found"))
                }
            });
            mock.expect_is_ancestor().returning(|_, _| Ok(true));
            mock.expect_commit_tree()
                .returning(|_, _, _| Ok("new_commit".to_string()));
            mock.expect_commit_tree_merge()
                .returning(|_, _, _| Ok("new_merge_commit".to_string()));
            mock.expect_get_commit_diff()
                .returning(|_| Ok("M\tsrc/main.rs".to_string()));
            mock.expect_update_branch().returning(|_, _| Ok(()));
            mock.expect_push_branch().returning(|_| Ok(()));
            mock
        }

        /// Standard GitHub mock with no existing branches
        pub fn standard_gh_mock() -> MockGithubOps {
            const PR_URL: &str = "https://github.com/test/repo/pull/123";
            let mut mock = MockGithubOps::new();
            mock.expect_find_branches_with_prefix()
                .returning(|_| Ok(vec![]));
            mock.expect_pr_create()
                .returning(|_, _, _, _| Ok(PR_URL.to_string()));
            mock.expect_pr_is_open().returning(|_| Ok(true));
            mock.expect_pr_edit()
                .returning(|_, _| Ok(PR_URL.to_string()));
            mock.expect_pr_url()
                .returning(|_| Ok(Some(PR_URL.to_string())));
            mock.expect_pr_diff()
                .returning(|_| Ok("M\tsrc/main.rs".to_string()));
            mock
        }
    }

    use super::*;
    use crate::ops::git::MockGitOps;
    use crate::ops::github::MockGithubOps;
    use crate::ops::jujutsu::Commit;
    use crate::ops::jujutsu::CommitMessage;
    use crate::ops::jujutsu::MockJujutsuOps;

    #[tokio::test]
    async fn test_find_previous_branch() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj.expect_get_commit().returning(|_| {
            Ok(Commit {
                change_id: "abc12345".to_string(),
                commit_id: "def45678".to_string(),
                message: CommitMessage {
                    title: Some("Test commit message".to_string()),
                    body: None,
                },
                parent_change_ids: vec!["abc1234567890".to_string()],
            })
        });

        let app = App::new(Config::default_for_tests(), mock_jj, MockGitOps::new(), MockGithubOps::new());

        let all_branches = vec!["test/abc12345".to_string()];
        let result = app.find_previous_branch("@", &all_branches).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test/abc12345");
    }

    #[tokio::test]
    async fn test_find_previous_branch_defaults_to_master() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj.expect_get_commit().returning(|_| {
            Ok(Commit {
                change_id: "xyz78901".to_string(),
                commit_id: "def45678".to_string(),
                message: CommitMessage {
                    title: Some("Test commit message".to_string()),
                    body: None,
                },
                parent_change_ids: vec!["nonexistent123".to_string()],
            })
        });

        let app = App::new(Config::default_for_tests(), mock_jj, MockGitOps::new(), MockGithubOps::new());

        let all_branches = vec!["test/abc12345".to_string()];
        let result = app.find_previous_branch("@", &all_branches).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "master");
    }
}
