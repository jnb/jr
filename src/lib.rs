pub mod git;
pub mod github;
pub mod jujutsu;

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use colored::Colorize;
use futures::future::join_all;
use git::GitOps;
use github::GithubOps;
use jujutsu::JujutsuOps;

/// Prefix used for all branches
pub const GLOBAL_BRANCH_PREFIX: &str = "jnb/";

/// Number of characters from the change ID to use in branch names
const CHANGE_ID_LENGTH: usize = 8;

pub struct App<J: JujutsuOps, G: GitOps, H: GithubOps> {
    pub jj: J,
    pub git: G,
    pub gh: H,
}

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub fn new(jj: J, git: G, gh: H) -> Self {
        Self { jj, git, gh }
    }

    pub async fn cmd_create(&self, revision: &str, stdout: &mut impl std::io::Write) -> Result<()> {
        // Get commit information from jj
        let commit = self.jj.get_commit(revision)?;

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

        self.validate_not_merged_to_main(&commit)?;

        // PR branch names: current and base
        let short_change_id = &commit.change_id[..CHANGE_ID_LENGTH.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;
        let base_branch = self.find_previous_branch(revision, &all_branches)?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id)?;
        writeln!(stdout, "Tree: {}", tree)?;

        // Check if PR branch already exists - if so, check if it's up to date
        if let Ok(existing_branch_tip) = self.git.get_branch(&pr_branch) {
            let existing_tree = self.git.get_tree(&existing_branch_tip)?;
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
            .context(format!("Base branch {} does not exist", base_branch))?;

        // Create new commit with jj commit message
        let new_commit = self
            .git
            .commit_tree(&tree, &parent, &commit.full_message())?;
        writeln!(stdout, "Created new commit: {}", new_commit)?;

        // Update PR branch to point to new commit
        self.git.update_branch(&pr_branch, &new_commit)?;
        writeln!(stdout, "Updated PR branch {}", pr_branch)?;

        // Push PR branch
        self.git.push_branch(&pr_branch)?;
        writeln!(stdout, "Pushed PR branch {}", pr_branch)?;

        // Create PR
        let pr_url = self
            .gh
            .pr_create(&pr_branch, &base_branch, pr_title, &pr_body)
            .await?;
        writeln!(
            stdout,
            "Created PR for {} with base {}",
            pr_branch, base_branch
        )?;
        writeln!(stdout, "PR URL: {}", pr_url)?;

        Ok(())
    }

    pub async fn cmd_update(
        &self,
        revision: &str,
        message: &str,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get commit information from jj
        let commit = self.jj.get_commit(revision)?;

        writeln!(stdout, "Change ID: {}", commit.change_id)?;
        writeln!(stdout, "Commit ID: {}", commit.commit_id)?;

        self.validate_not_merged_to_main(&commit)?;
        self.check_parent_prs_up_to_date(revision).await?;

        // PR branch names: current and base
        let short_change_id = &commit.change_id[..CHANGE_ID_LENGTH.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;
        let base_branch = self.find_previous_branch(revision, &all_branches)?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id)?;
        writeln!(stdout, "Tree: {}", tree)?;

        // PR branch must exist for update
        let _existing_pr_branch = self.git.get_branch(&pr_branch).context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;

        writeln!(stdout, "PR branch {} exists", pr_branch)?;

        // Get both parents for merge commit
        let old_pr_tip = self
            .git
            .get_branch(&pr_branch)
            .context(format!("PR branch {} does not exist", pr_branch))?;
        let base_tip = self
            .git
            .get_branch(&base_branch)
            .context(format!("Base branch {} does not exist", base_branch))?;

        // Use the provided commit message
        let commit_message = message;

        // Check if we need to create a new commit
        let old_pr_tree = self.git.get_tree(&old_pr_tip)?;
        let base_has_changed = !self.git.is_ancestor(&base_tip, &old_pr_tip)?;

        let new_commit = if tree == old_pr_tree && !base_has_changed {
            writeln!(
                stdout,
                "Tree unchanged and base hasn't moved, reusing old PR tip commit"
            )?;
            old_pr_tip.clone()
        } else if base_has_changed {
            // Create merge commit with old PR tip and base as parents
            let commit =
                self.git
                    .commit_tree_merge(&tree, vec![old_pr_tip.clone(), base_tip.clone()], &commit_message)?;
            writeln!(stdout, "Created new merge commit: {}", commit)?;
            commit
        } else {
            // Tree changed but base hasn't - create regular commit with single parent
            let commit = self.git.commit_tree(&tree, &old_pr_tip, &commit_message)?;
            writeln!(stdout, "Created new commit: {}", commit)?;
            commit
        };

        // Only update if there are actual changes
        if new_commit == old_pr_tip {
            writeln!(stdout, "No changes to push - PR is already up to date")?;
            return Ok(());
        }

        // Update PR branch to point to new commit
        self.git.update_branch(&pr_branch, &new_commit)?;
        writeln!(stdout, "Updated PR branch {}", pr_branch)?;

        // Push PR branch
        self.git.push_branch(&pr_branch)?;
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

    pub async fn cmd_restack(
        &self,
        revision: &str,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get commit information from jj
        let commit = self.jj.get_commit(revision)?;

        writeln!(stdout, "Change ID: {}", commit.change_id)?;
        writeln!(stdout, "Commit ID: {}", commit.commit_id)?;

        self.validate_not_merged_to_main(&commit)?;
        self.check_parent_prs_up_to_date(revision).await?;

        // PR branch names: current and base
        let short_change_id = &commit.change_id[..CHANGE_ID_LENGTH.min(commit.change_id.len())];
        let pr_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;
        let base_branch = self.find_previous_branch(revision, &all_branches)?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit.commit_id)?;
        writeln!(stdout, "Tree: {}", tree)?;

        // PR branch must exist for restack
        let _existing_pr_branch = self.git.get_branch(&pr_branch).context(format!(
            "PR branch {} does not exist. Use 'jr create' to create a new PR.",
            pr_branch
        ))?;

        writeln!(stdout, "PR branch {} exists", pr_branch)?;

        // Check if this is a pure restack (no local changes)
        let local_change_diff = self.git.get_commit_diff(&commit.commit_id)?;
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
            .context(format!("PR branch {} does not exist", pr_branch))?;
        let base_tip = self
            .git
            .get_branch(&base_branch)
            .context(format!("Base branch {} does not exist", base_branch))?;

        let commit_message = "Restack";

        // Check if we need to create a new commit
        let old_pr_tree = self.git.get_tree(&old_pr_tip)?;
        let base_has_changed = !self.git.is_ancestor(&base_tip, &old_pr_tip)?;

        let new_commit = if tree == old_pr_tree && !base_has_changed {
            writeln!(
                stdout,
                "Tree unchanged and base hasn't moved, reusing old PR tip commit"
            )?;
            old_pr_tip.clone()
        } else {
            // Create merge commit with old PR tip and base as parents
            let commit =
                self.git
                    .commit_tree_merge(&tree, vec![old_pr_tip.clone(), base_tip.clone()], commit_message)?;
            writeln!(stdout, "Created new merge commit: {}", commit)?;
            commit
        };

        // Only update if there are actual changes
        if new_commit == old_pr_tip {
            writeln!(stdout, "No changes to push - PR is already up to date")?;
            return Ok(());
        }

        // Update PR branch to point to new commit
        self.git.update_branch(&pr_branch, &new_commit)?;
        writeln!(stdout, "Updated PR branch {}", pr_branch)?;

        // Push PR branch
        self.git.push_branch(&pr_branch)?;
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

    pub async fn cmd_status(
        &self,
        stdout: &mut impl std::io::Write,
        stderr: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get the current commit to mark it in the output
        let current_commit = self.jj.get_commit("@")?;

        // Find the head(s) of the current stack
        let heads = self.jj.get_stack_heads()?;

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
            self.jj.get_stack_changes(head_commit_id)?
        } else {
            // Multiple heads detected - show from @ to trunk with warning
            writeln!(
                stderr,
                "Warning: Multiple stack heads detected. Showing stack from @ to trunk."
            )?;
            self.jj.get_stack_changes("@")?
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
            if let Ok(commit) = self.jj.get_commit(change_id) {
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

        // Get base branches (no longer async, uses cached all_branches)
        let base_branches: Vec<_> = changes
            .iter()
            .map(|(change_id, _commit_id)| self.find_previous_branch(change_id, &all_branches))
            .collect();

        // Display results
        for (i, (change_id, commit_id)) in changes.iter().enumerate() {
            let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
            let expected_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);
            let pr_url_result = &pr_urls[i];
            let base_branch_result = &base_branches[i];

            // Get commit and extract title from message
            let commit = self.jj.get_commit(commit_id)?;
            let commit_title = commit.message.title.as_deref().unwrap_or("");

            // Get abbreviated change ID (4 chars, matching jj status default)
            let abbreviated_change_id = &change_id[..4.min(change_id.len())];

            // Check if parent PR is outdated
            let parent_pr_outdated =
                self.is_parent_pr_outdated(change_id, &all_branches, &pr_diffs)?;

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

    /// Check if any parent PR is out of date
    /// A parent is outdated if its local single commit diff differs from cumulative remote diff,
    /// or if the parent itself needs a restack (recursive check)
    fn is_parent_pr_outdated(
        &self,
        revision: &str,
        all_branches: &[String],
        pr_diffs: &HashMap<String, String>,
    ) -> Result<bool> {
        // Get parent change IDs from commit
        let commit = self.jj.get_commit(revision)?;
        let parent_change_ids = commit.parent_change_ids;

        // For each parent, check if it has a PR and if it's outdated
        for parent_change_id in parent_change_ids {
            let short_parent_id = &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
            let parent_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_parent_id);

            // If this parent has a PR branch, check if it's outdated
            if all_branches.contains(&parent_branch) {
                // Compare parent's local single commit diff vs cumulative PR diff from cache
                let parent_commit = self.jj.get_commit(&parent_change_id)?;
                let parent_local_diff = self.git.get_commit_diff(&parent_commit.commit_id)?;

                if let Some(parent_pr_diff) = pr_diffs.get(&parent_branch) {
                    if &parent_local_diff != parent_pr_diff {
                        return Ok(true); // Parent has local changes
                    }
                }

                // Check if parent's base has moved
                if let Ok(parent_base_branch) =
                    self.find_previous_branch(&parent_change_id, all_branches)
                {
                    if let Ok(parent_pr_commit) = self.git.get_branch(&parent_branch) {
                        if let Ok(base_tip) = self.git.get_branch(&parent_base_branch) {
                            // If base is not an ancestor of parent's PR, base has moved
                            if !self.git.is_ancestor(&base_tip, &parent_pr_commit)? {
                                return Ok(true); // Parent's base has moved, needs restack
                            }
                        }
                    }
                }

                // Recursively check if the parent itself needs a restack
                if self.is_parent_pr_outdated(&parent_change_id, all_branches, pr_diffs)? {
                    return Ok(true); // Parent's ancestor is outdated
                }
            }
        }

        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    async fn show_change_status_with_data(
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
                    let local_diff = self.git.get_commit_diff(commit_id);

                    match local_diff {
                        Ok(local_diff) => {
                            if let Some(pr_diff) = pr_diffs.get(expected_branch) {
                                // Check if base branch has moved (not an ancestor of PR branch)
                                let base_has_moved = if let Some(base_branch) = _base_branch {
                                    // Get PR branch tip
                                    if let Ok(pr_branch_tip) = self.git.get_branch(expected_branch)
                                    {
                                        // Get base branch tip
                                        if let Ok(base_tip) = self.git.get_branch(base_branch) {
                                            // If base is not an ancestor of PR, base has moved
                                            !self
                                                .git
                                                .is_ancestor(&base_tip, &pr_branch_tip)
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

    /// Validate that a commit is not already merged to trunk
    fn validate_not_merged_to_main(&self, commit: &jujutsu::Commit) -> Result<()> {
        let trunk_commit = self.jj.get_trunk_commit_id()?;

        if self.jj.is_ancestor(&commit.commit_id, &trunk_commit)? {
            return Err(anyhow::anyhow!(
                "Cannot create PR: commit {} is an ancestor of trunk. This commit is already merged.",
                commit.commit_id
            ));
        }

        Ok(())
    }

    /// Find the previous PR branch in the stack based on parent change IDs from jujutsu
    fn find_previous_branch(&self, revision: &str, all_branches: &[String]) -> Result<String> {
        // Get parent change IDs from commit
        let commit = self.jj.get_commit(revision)?;
        let parent_change_ids = commit.parent_change_ids;

        // For each parent, check if a PR branch exists
        for parent_change_id in parent_change_ids {
            let short_parent_id = &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
            let parent_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_parent_id);

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
    async fn check_parent_prs_up_to_date(&self, revision: &str) -> Result<()> {
        // Get all changes in the stack from revision back to trunk
        let commit = self.jj.get_commit(revision)?;
        let stack_changes = self.jj.get_stack_changes(&commit.commit_id)?;

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;

        // Collect all branches that exist in the stack (excluding current revision)
        let branches_to_check: Vec<_> = stack_changes
            .iter()
            .filter(|(_, commit_id_in_stack)| commit_id_in_stack != &commit.commit_id)
            .filter_map(|(change_id, _commit_id_in_stack)| {
                let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
                let expected_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);
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
        let pr_diff_results = join_all(pr_diff_futures).await;

        // Check each change in the stack
        for ((change_id, _, expected_branch), (_, pr_diff_result)) in
            branches_to_check.iter().zip(pr_diff_results.iter())
        {
            // Get the commit for this change
            let commit_in_stack = self.jj.get_commit(change_id)?;

            // Compare local single commit diff vs cumulative PR diff from GitHub
            let local_diff = self.git.get_commit_diff(&commit_in_stack.commit_id)?;
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
}

// =============================================================================
// Tests with mock implementations
// =============================================================================

#[cfg(test)]
mod tests {
    use git::MockGitOps;
    use github::MockGithubOps;
    use jujutsu::{Commit, CommitMessage, MockJujutsuOps};

    use super::*;

    // Disable colors for all tests to get clean output
    #[ctor::ctor]
    fn init() {
        colored::control::set_override(false);
    }

    #[tokio::test]
    async fn test_cmd_create_creates_new_pr() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "def45678".to_string(),
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
            .returning(|_, _| Ok(false));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("tree123".to_string()));
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                if branch == "master" {
                    Ok("base_commit".to_string())
                } else {
                    Err(anyhow::anyhow!("Branch not found"))
                }
            });
        mock_git
            .expect_commit_tree()
            .returning(|_, _, _| Ok("new_commit".to_string()));
        mock_git.expect_update_branch().returning(|_, _| Ok(()));
        mock_git.expect_push_branch().returning(|_| Ok(()));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));
        mock_gh
            .expect_pr_create()
            .withf(|pr_branch, base_branch, _title, _body| {
                pr_branch == "jnb/abc12345" && base_branch == "master"
            })
            .returning(|_, _, _, _| Ok("https://github.com/test/repo/pull/123".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_ok());

        // Verify output
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("Change ID: abc12345"));
        assert!(output.contains("PR branch: jnb/abc12345"));
        assert!(output.contains("Base branch: master"));
    }

    #[tokio::test]
    async fn test_cmd_update_updates_existing_pr() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "def45678".to_string(),
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
            .returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                match branch {
                    "master" => Ok("main_commit".to_string()),
                    "jnb/abc12345" => Ok("existing_commit".to_string()),
                    _ => Err(anyhow::anyhow!("Branch not found")),
                }
            });
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("tree123".to_string()));
        mock_git
            .expect_is_ancestor()
            .returning(|_, _| Ok(true));
        mock_git
            .expect_update_branch()
            .returning(|_, _| Ok(()));
        mock_git.expect_push_branch().returning(|_| Ok(()));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));
        mock_gh
            .expect_pr_is_open()
            .returning(|_| Ok(true));
        mock_gh
            .expect_pr_edit()
            .withf(|pr_branch, base_branch| {
                pr_branch == "jnb/abc12345" && base_branch == "master"
            })
            .returning(|_, _| Ok("https://github.com/test/repo/pull/123".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_update("@", "Update from review", &mut stdout).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_find_previous_branch() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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

        let mock_git = MockGitOps::new();
        let mock_gh = MockGithubOps::new();

        let app = App::new(mock_jj, mock_git, mock_gh);

        let all_branches = vec!["jnb/abc12345".to_string()];
        let result = app.find_previous_branch("@", &all_branches);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "jnb/abc12345");
    }

    #[test]
    fn test_find_previous_branch_defaults_to_master() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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

        let mock_git = MockGitOps::new();
        let mock_gh = MockGithubOps::new();

        let app = App::new(mock_jj, mock_git, mock_gh);

        let all_branches = vec!["jnb/abc12345".to_string()];
        let result = app.find_previous_branch("@", &all_branches);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "master");
    }

    #[tokio::test]
    async fn test_cmd_status_branch_exists_up_to_date() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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
            .expect_get_stack_heads()
            .returning(|| Ok(vec![]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                if branch == "jnb/abc12345" {
                    Ok("remote_commit".to_string())
                } else {
                    Err(anyhow::anyhow!("Branch not found"))
                }
            });
        mock_git
            .expect_get_commit_diff()
            .returning(|_| Ok("M\tsrc/main.rs".to_string()));
        mock_git
            .expect_is_ancestor()
            .returning(|_, _| Ok(true));

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
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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
            .expect_get_stack_heads()
            .returning(|| Ok(vec![]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                if branch == "jnb/abc12345" {
                    Ok("remote_commit".to_string())
                } else {
                    Err(anyhow::anyhow!("Branch not found"))
                }
            });
        mock_git
            .expect_get_commit_diff()
            .returning(|commit_id| {
                if commit_id == "local_commit" {
                    Ok("M\tsrc/main.rs\nA\tsrc/new.rs".to_string())
                } else {
                    Ok("M\tsrc/main.rs".to_string())
                }
            });
        mock_git
            .expect_is_ancestor()
            .returning(|_, _| Ok(true));

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
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "def45678".to_string(),
                    message: CommitMessage {
                        title: Some("Test commit message".to_string()),
                        body: None,
                    },
                    parent_change_ids: vec![],
                })
            });
        mock_jj
            .expect_get_stack_heads()
            .returning(|| Ok(vec![]));

        let mock_git = MockGitOps::new();

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/other123".to_string()]));
        mock_gh
            .expect_pr_url()
            .returning(|_| Ok(None));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_status_branch_exists_no_pr() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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
            .expect_get_stack_heads()
            .returning(|| Ok(vec![]));

        let mock_git = MockGitOps::new();

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh
            .expect_pr_url()
            .returning(|_| Ok(None));
        mock_gh
            .expect_pr_diff()
            .returning(|_| Ok("".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_create_rejects_ancestor_of_main() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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

        let mock_git = MockGitOps::new();
        let mock_gh = MockGithubOps::new();

        let app = App::new(mock_jj, mock_git, mock_gh);

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
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "new_commit".to_string(),
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
            .returning(|_, _| Ok(false));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("tree123".to_string()));
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                if branch == "master" {
                    Ok("main_commit".to_string())
                } else {
                    Err(anyhow::anyhow!("Branch not found"))
                }
            });
        mock_git
            .expect_commit_tree()
            .returning(|_, _, _| Ok("new_commit_obj".to_string()));
        mock_git
            .expect_update_branch()
            .returning(|_, _| Ok(()));
        mock_git
            .expect_push_branch()
            .returning(|_| Ok(()));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));
        mock_gh
            .expect_pr_create()
            .returning(|_, _, _, _| Ok("https://github.com/test/repo/pull/123".to_string()));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_update_errors_when_pr_is_closed() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "def45678".to_string(),
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
            .returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                match branch {
                    "master" => Ok("main_commit".to_string()),
                    "jnb/abc12345" => Ok("existing_commit".to_string()),
                    _ => Err(anyhow::anyhow!("Branch not found")),
                }
            });
        mock_git
            .expect_get_tree()
            .returning(|commit_id| {
                if commit_id == "def45678" {
                    Ok("new_tree".to_string())
                } else {
                    Ok("old_tree".to_string())
                }
            });
        mock_git
            .expect_is_ancestor()
            .returning(|_, _| Ok(true));
        mock_git
            .expect_commit_tree()
            .returning(|_, _, _| Ok("new_commit_obj".to_string()));
        mock_git
            .expect_update_branch()
            .returning(|_, _| Ok(()));
        mock_git
            .expect_push_branch()
            .returning(|_| Ok(()));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));
        mock_gh
            .expect_pr_is_open()
            .returning(|_| Ok(false));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app
            .cmd_update("@", "Update after review", &mut stdout)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No open PR found"));
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_branch_exists_and_up_to_date() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "def45678".to_string(),
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
            .returning(|_, _| Ok(false));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("same_tree".to_string()));
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                match branch {
                    "master" => Ok("main_commit".to_string()),
                    "jnb/abc12345" => Ok("existing_commit".to_string()),
                    _ => Err(anyhow::anyhow!("Branch not found")),
                }
            });

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("up to date"));
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_branch_exists_with_different_content() {
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
                Ok(Commit {
                    change_id: "abc12345".to_string(),
                    commit_id: "def45678".to_string(),
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
            .returning(|_, _| Ok(false));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_tree()
            .returning(|commit| {
                if commit == "def45678" {
                    Ok("new_tree".to_string())
                } else {
                    Ok("old_tree".to_string())
                }
            });
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                match branch {
                    "master" => Ok("main_commit".to_string()),
                    "jnb/abc12345" => Ok("existing_commit".to_string()),
                    _ => Err(anyhow::anyhow!("Branch not found")),
                }
            });

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec![]));

        let app = App::new(mock_jj, mock_git, mock_gh);

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
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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

        let mock_git = MockGitOps::new();
        let mock_gh = MockGithubOps::new();

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("empty description"));
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
            .returning(|revision| {
                match revision {
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
                }
            });
        mock_jj
            .expect_get_trunk_commit_id()
            .returning(|| Ok("trunk123".to_string()));
        mock_jj
            .expect_is_ancestor()
            .returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![
                ("ccc12345".to_string(), "commit_c_local".to_string()),
                ("bbb12345".to_string(), "commit_b_local".to_string()),
            ]));

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
            .returning(|_| Ok(vec!["jnb/bbb12345".to_string(), "jnb/ccc12345".to_string()]));
        mock_gh
            .expect_pr_diff()
            .returning(|branch| {
                if branch == "jnb/bbb12345" {
                    Ok("diff --git a/src/file.rs b/src/file.rs\n--- a/src/file.rs\n+++ b/src/file.rs\n@@ -1,1 +1,2 @@\n content\n+old line".to_string())
                } else {
                    Ok("diff --git a/src/other.rs b/src/other.rs\n--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1,1 +1,2 @@\n content\n+change".to_string())
                }
            });

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_update("@", "Update commit C", &mut stdout).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("parent PR"));
        assert!(error_msg.contains("out of date"));
        assert!(error_msg.contains("jnb/bbb12345"));
    }

    #[tokio::test]
    async fn test_cmd_restack_works_when_diffs_match() {
        // Set up a commit where the diff introduced by each commit matches (pure restack)
        let mut mock_jj = MockJujutsuOps::new();
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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
        mock_jj
            .expect_is_ancestor()
            .returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![("abc12345".to_string(), "local_commit".to_string())]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                match branch {
                    "master" => Ok("main_commit".to_string()),
                    "jnb/abc12345" => Ok("remote_commit".to_string()),
                    _ => Err(anyhow::anyhow!("Branch not found")),
                }
            });
        mock_git
            .expect_get_tree()
            .returning(|_| Ok("same_tree".to_string()));
        mock_git
            .expect_get_commit_diff()
            .returning(|_| Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment".to_string()));
        mock_git
            .expect_is_ancestor()
            .returning(|_, _| Ok(true));

        let mut mock_gh = MockGithubOps::new();
        mock_gh
            .expect_find_branches_with_prefix()
            .returning(|_| Ok(vec!["jnb/abc12345".to_string()]));
        mock_gh
            .expect_pr_diff()
            .returning(|_| Ok("diff --git a/src/main.rs b/src/main.rs\nindex 123..456\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// comment".to_string()));
        mock_gh
            .expect_pr_is_open()
            .returning(|_| Ok(true));
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
        mock_jj
            .expect_get_commit()
            .returning(|_| {
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
        mock_jj
            .expect_is_ancestor()
            .returning(|_, _| Ok(false));
        mock_jj
            .expect_get_stack_changes()
            .returning(|_| Ok(vec![("abc12345".to_string(), "local_commit".to_string())]));

        let mut mock_git = MockGitOps::new();
        mock_git
            .expect_get_branch()
            .returning(|branch| {
                match branch {
                    "master" => Ok("main_commit".to_string()),
                    "jnb/abc12345" => Ok("remote_commit".to_string()),
                    _ => Err(anyhow::anyhow!("Branch not found")),
                }
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
