pub mod git;
pub mod github;
pub mod jujutsu;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use colored::Colorize;
use futures::future::join_all;
use git::GitOps;
use github::GithubOps;
use jujutsu::JujutsuOps;

/// Prefix used for all branches
pub const GLOBAL_BRANCH_PREFIX: &str = "jnb/";

/// Number of characters from the change ID to use in branch names
const CHANGE_ID_LENGTH: usize = 8;

#[derive(Parser)]
#[command(name = "jr")]
#[command(about = "Jujutsu Review: Manage Git branches and GitHub PRs in a stacked workflow", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new PR (uses jj commit message)
    Create {
        /// Revision to use (defaults to @)
        #[arg(short, long, default_value = "@")]
        revision: String,
    },
    /// Update an existing PR
    Update {
        /// Revision to use (defaults to @)
        #[arg(short, long, default_value = "@")]
        revision: String,
        /// Commit message describing the update (optional; auto-generates "Restack" if this is a pure restack)
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Show status of stacked PRs
    Status,
}

// =============================================================================
// App structure with dependency injection
// =============================================================================

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
        // Get change ID and commit message from jj
        let change_id = self.jj.get_change_id(revision)?;
        let commit_id = self.jj.get_commit_id(revision)?;
        let commit_message = self.jj.get_commit_message(revision)?;

        // Validate commit message is not empty
        if commit_message.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "Cannot create PR: commit has empty description. Add a description with 'jj describe'."
            ));
        }

        // Split commit message into title (first line) and body (rest)
        let lines: Vec<&str> = commit_message.lines().collect();
        let pr_title = lines.first().unwrap_or(&"").trim();
        let pr_body = if lines.len() > 1 {
            lines[1..].join("\n").trim().to_string()
        } else {
            String::new()
        };

        writeln!(stdout, "Change ID: {}", change_id)?;
        writeln!(stdout, "Commit ID: {}", commit_id)?;

        // Validate that this commit is not an ancestor of main
        // (i.e., check if main is a descendant of this commit, which means this commit is already merged)
        let main_commit = self
            .git
            .get_branch("master")
            .or_else(|_| self.git.get_branch("main"))?;
        if self.git.is_ancestor(&commit_id, &main_commit)? {
            return Err(anyhow::anyhow!(
                "Cannot create PR: commit {} is an ancestor of main branch. This commit is already merged.",
                commit_id
            ));
        }

        // PR branch names: current and base
        let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
        let pr_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;
        let base_branch = self.find_previous_branch(revision, &all_branches)?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit_id)?;
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
        let new_commit = self.git.commit_tree(&tree, &parent, &commit_message)?;
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
        message: Option<&str>,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        // Get change ID from jj
        let change_id = self.jj.get_change_id(revision)?;
        let commit_id = self.jj.get_commit_id(revision)?;

        writeln!(stdout, "Change ID: {}", change_id)?;
        writeln!(stdout, "Commit ID: {}", commit_id)?;

        // Check that all parent PRs in the stack are up to date
        self.check_parent_prs_up_to_date(revision).await?;

        // Validate that this commit is not an ancestor of main
        // (i.e., check if main is a descendant of this commit, which means this commit is already merged)
        let main_commit = self
            .git
            .get_branch("master")
            .or_else(|_| self.git.get_branch("main"))?;
        if self.git.is_ancestor(&commit_id, &main_commit)? {
            return Err(anyhow::anyhow!(
                "Cannot update PR: commit {} is an ancestor of main branch. This commit is already merged.",
                commit_id
            ));
        }

        // PR branch names: current and base
        let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
        let pr_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;
        let base_branch = self.find_previous_branch(revision, &all_branches)?;

        writeln!(stdout, "PR branch: {}", pr_branch)?;
        writeln!(stdout, "Base branch: {}", base_branch)?;

        // Get the tree from the current Jujutsu commit (represents current state)
        let tree = self.git.get_tree(&commit_id)?;
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

        // Determine the commit message to use
        let commit_message = if let Some(msg) = message {
            msg.to_string()
        } else {
            // No message provided - check if this is a pure restack
            // Compare local tree vs PR branch tree
            let pr_commit_id = self.git.get_branch(&pr_branch)?;
            let pr_tree = self.git.get_tree(&pr_commit_id)?;

            if tree == pr_tree {
                // Pure restack - the tree content is the same
                writeln!(stdout, "Detected pure restack (no changes to this commit)")?;
                "Restack".to_string()
            } else {
                return Err(anyhow::anyhow!(
                    "Cannot update PR: commit has changes but no message provided. Use -m to specify a commit message."
                ));
            }
        };

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
                    .commit_tree_merge(&tree, &[&old_pr_tip, &base_tip], &commit_message)?;
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
        // Get the current change ID to mark it in the output
        let current_change_id = self.jj.get_change_id("@")?;
        let current_commit_id = self.jj.get_commit_id("@")?;

        // Find the head(s) of the current stack
        let heads = self.jj.get_stack_heads()?;

        // Collect all changes to process
        let changes: Vec<(String, String)> = if heads.is_empty() {
            // Current commit is on trunk or no stack exists
            let change_id = self.jj.get_change_id("@")?;
            vec![(change_id, current_commit_id.clone())]
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

            // Get commit message and extract title (first line)
            let commit_message = self.jj.get_commit_message(commit_id)?;
            let commit_title = commit_message.lines().next().unwrap_or("").trim();

            // Get abbreviated change ID (4 chars, matching jj status default)
            let abbreviated_change_id = &change_id[..4.min(change_id.len())];

            // Check if parent PR is outdated
            let parent_pr_outdated = self.is_parent_pr_outdated(change_id, &all_branches)?;

            // Get base branch (or default to empty string if error)
            let base_branch = base_branch_result.as_ref().ok();

            self.show_change_status_with_data(
                &expected_branch,
                change_id,
                commit_id,
                &current_change_id,
                &all_branches,
                pr_url_result,
                commit_title,
                abbreviated_change_id,
                parent_pr_outdated,
                base_branch,
                stdout,
            )
            .await?;
        }

        Ok(())
    }

    /// Check if any parent PR is out of date
    /// A parent is outdated if its local tree differs from the PR branch tree,
    /// or if the parent itself needs a restack (recursive check)
    fn is_parent_pr_outdated(&self, revision: &str, all_branches: &[String]) -> Result<bool> {
        // Get parent change IDs from jujutsu
        let parent_change_ids = self.jj.get_parent_change_ids(revision)?;

        // For each parent, check if it has a PR and if it's outdated
        for parent_change_id in parent_change_ids {
            let short_parent_id = &parent_change_id[..CHANGE_ID_LENGTH.min(parent_change_id.len())];
            let parent_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_parent_id);

            // If this parent has a PR branch, check if it's outdated
            if all_branches.contains(&parent_branch) {
                // Compare parent's local tree vs PR branch tree
                let parent_commit_id = self.jj.get_commit_id(&parent_change_id)?;
                let parent_local_tree = self.git.get_tree(&parent_commit_id)?;

                if let Ok(pr_commit_id) = self.git.get_branch(&parent_branch) {
                    let pr_tree = self.git.get_tree(&pr_commit_id)?;
                    if parent_local_tree != pr_tree {
                        return Ok(true); // Parent has local changes
                    }
                }

                // Recursively check if the parent itself needs a restack
                if self.is_parent_pr_outdated(&parent_change_id, all_branches)? {
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
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        let is_current = change_id == current_change_id;
        let branch_exists = all_branches.contains(&expected_branch.to_string());

        // Determine status symbol
        let status_symbol = if branch_exists {
            // Check if PR exists
            match pr_url_result {
                Ok(Some(_)) => {
                    // Compare local tree vs PR branch tree
                    let local_tree = self.git.get_tree(commit_id);

                    match local_tree {
                        Ok(local_tree) => {
                            if let Ok(pr_commit_id) = self.git.get_branch(expected_branch) {
                                if let Ok(pr_tree) = self.git.get_tree(&pr_commit_id) {
                                    // Check if base branch has moved (not an ancestor of PR branch)
                                    let base_has_moved = if let Some(base_branch) = _base_branch {
                                        // Get base branch tip
                                        if let Ok(base_tip) = self.git.get_branch(base_branch) {
                                            // If base is not an ancestor of PR, base has moved
                                            !self
                                                .git
                                                .is_ancestor(&base_tip, &pr_commit_id)
                                                .unwrap_or(false)
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    };

                                    if local_tree == pr_tree {
                                        // Trees match - check if parent changed or base moved
                                        if parent_pr_outdated || base_has_moved {
                                            "↻" // Needs restack (parent changed or base moved)
                                        } else {
                                            "✓" // Up to date
                                        }
                                    } else {
                                        "✗" // Has local changes
                                    }
                                } else {
                                    "-"
                                }
                            } else {
                                "-"
                            }
                        }
                        _ => "-",
                    }
                }
                Ok(None) | Err(_) => "-",
            }
        } else {
            "-"
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

    /// Find the previous PR branch in the stack based on parent change IDs from jujutsu
    fn find_previous_branch(&self, revision: &str, all_branches: &[String]) -> Result<String> {
        // Get parent change IDs from jujutsu
        let parent_change_ids = self.jj.get_parent_change_ids(revision)?;

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
        let commit_id = self.jj.get_commit_id(revision)?;
        let stack_changes = self.jj.get_stack_changes(&commit_id)?;

        // Fetch all branches once
        let all_branches = self.gh.find_branches_with_prefix("").await?;

        // Collect all branches that exist in the stack (excluding current revision)
        let branches_to_check: Vec<_> = stack_changes
            .iter()
            .filter(|(_, commit_id_in_stack)| commit_id_in_stack != &commit_id)
            .filter_map(|(change_id, commit_id_in_stack)| {
                let short_change_id = &change_id[..CHANGE_ID_LENGTH.min(change_id.len())];
                let expected_branch = format!("{}{}", GLOBAL_BRANCH_PREFIX, short_change_id);
                if all_branches.contains(&expected_branch) {
                    Some((
                        change_id.clone(),
                        commit_id_in_stack.clone(),
                        expected_branch,
                    ))
                } else {
                    None
                }
            })
            .collect();

        // Check each change in the stack
        for (_change_id, commit_id_in_stack, expected_branch) in branches_to_check {
            // Compare local tree vs PR branch tree
            let local_tree = self.git.get_tree(&commit_id_in_stack)?;
            let pr_commit_id = self.git.get_branch(&expected_branch)?;
            let pr_tree = self.git.get_tree(&pr_commit_id)?;

            if local_tree != pr_tree {
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
    use git::MockGit;
    use github::MockGithub;
    use jujutsu::MockJujutsu;

    use super::*;

    // Disable colors for all tests to get clean output
    #[ctor::ctor]
    fn init() {
        colored::control::set_override(false);
    }

    #[tokio::test]
    async fn test_cmd_create_creates_new_pr() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new().with_branch("master".to_string(), "base_commit".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_ok());

        // Verify output
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("Change ID: abc12345"));
        assert!(output.contains("PR branch: jnb/abc12345"));
        assert!(output.contains("Base branch: master"));

        // Verify PR was created
        let created_prs = app.gh.created_prs.borrow();
        assert_eq!(created_prs.len(), 1);
        assert_eq!(created_prs[0].0, "jnb/abc12345");
        assert_eq!(created_prs[0].1, "master");
    }

    #[tokio::test]
    async fn test_cmd_update_updates_existing_pr() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/abc12345".to_string(), "existing_commit".to_string());
        let mock_gh =
            MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string()).with_pr("jnb/abc12345".to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app
            .cmd_update("@", Some("Update from review"), &mut stdout)
            .await;
        assert!(result.is_ok());

        // Verify PR was edited, not created
        let created_prs = app.gh.created_prs.borrow();
        let edited_prs = app.gh.edited_prs.borrow();
        assert_eq!(created_prs.len(), 0);
        assert_eq!(edited_prs.len(), 1);
        assert_eq!(edited_prs[0].0, "jnb/abc12345");
    }

    #[test]
    fn test_find_previous_branch() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec!["abc1234567890".to_string()],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new();
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()]);

        let app = App::new(mock_jj, mock_git, mock_gh);

        let all_branches = vec!["jnb/abc12345".to_string()];
        let result = app.find_previous_branch("@", &all_branches);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "jnb/abc12345");
    }

    #[test]
    fn test_find_previous_branch_defaults_to_master() {
        let mock_jj = MockJujutsu {
            change_id: "xyz78901".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec!["nonexistent123".to_string()],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new();
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()]);

        let app = App::new(mock_jj, mock_git, mock_gh);

        let all_branches = vec!["jnb/abc12345".to_string()];
        let result = app.find_previous_branch("@", &all_branches);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "master");
    }

    #[tokio::test]
    async fn test_cmd_status_branch_exists_up_to_date() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "local_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("jnb/abc12345".to_string(), "remote_commit".to_string())
            .with_tree("local_commit".to_string(), "same_tree".to_string())
            .with_tree("remote_commit".to_string(), "same_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()])
            .with_pr("jnb/abc12345".to_string());

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
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "local_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("jnb/abc12345".to_string(), "remote_commit".to_string())
            .with_tree("local_commit".to_string(), "new_tree".to_string())
            .with_tree("remote_commit".to_string(), "old_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()])
            .with_pr("jnb/abc12345".to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_status_branch_does_not_exist() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new();
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/other123".to_string()]);

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_status_branch_exists_no_pr() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "local_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("jnb/abc12345".to_string(), "remote_commit".to_string())
            .with_tree("local_commit".to_string(), "same_tree".to_string())
            .with_tree("remote_commit".to_string(), "same_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()]);

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = app.cmd_status(&mut stdout, &mut stderr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_create_rejects_ancestor_of_main() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "old_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_ancestor("old_commit".to_string(), "main_commit".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ancestor of main branch"));
    }

    #[tokio::test]
    async fn test_cmd_create_accepts_non_ancestor() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "new_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new().with_branch("master".to_string(), "main_commit".to_string());
        // Don't add new_commit as an ancestor of main_commit
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_update_errors_when_pr_is_closed() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/abc12345".to_string(), "existing_commit".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_closed_pr("jnb/abc12345".to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app
            .cmd_update("@", Some("Update after review"), &mut stdout)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No open PR found"));
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_branch_exists_and_up_to_date() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/abc12345".to_string(), "existing_commit".to_string())
            .with_tree("def45678".to_string(), "same_tree".to_string())
            .with_tree("existing_commit".to_string(), "same_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app.cmd_create("@", &mut stdout).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("up to date"));
    }

    #[tokio::test]
    async fn test_cmd_create_errors_when_branch_exists_with_different_content() {
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/abc12345".to_string(), "existing_commit".to_string())
            .with_tree("def45678".to_string(), "new_tree".to_string())
            .with_tree("existing_commit".to_string(), "old_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string());

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
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "def45678".to_string(),
            commit_message: "".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new().with_branch("master".to_string(), "main_commit".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string());

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
        let mut change_to_commit = std::collections::HashMap::new();
        change_to_commit.insert("bbb12345".to_string(), "commit_b_local".to_string());
        change_to_commit.insert("ccc12345".to_string(), "commit_c_local".to_string());

        let mut change_to_parents = std::collections::HashMap::new();
        change_to_parents.insert("bbb12345".to_string(), vec![]); // B's parent is master (not in map)
        change_to_parents.insert("ccc12345".to_string(), vec!["bbb12345".to_string()]); // C's parent is B

        let mock_jj = MockJujutsu {
            change_id: "ccc12345".to_string(),       // C's change ID
            commit_id: "commit_c_local".to_string(), // C's local commit
            commit_message: "Commit C message".to_string(),
            parent_change_ids: vec!["bbb12345".to_string()], // C's parent is B
            stack_heads: vec![],
            stack_changes: vec![
                // Stack from C back to trunk (tip-to-base order)
                ("ccc12345".to_string(), "commit_c_local".to_string()),
                ("bbb12345".to_string(), "commit_b_local".to_string()),
            ],
            change_to_commit,
            change_to_parents,
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/bbb12345".to_string(), "commit_b_remote".to_string()) // B's PR branch
            .with_branch("jnb/ccc12345".to_string(), "commit_c_remote".to_string()) // C's PR branch
            // B's local tree differs from remote tree (outdated)
            .with_tree("commit_b_local".to_string(), "tree_b_new".to_string())
            .with_tree("commit_b_remote".to_string(), "tree_b_old".to_string())
            // C's trees
            .with_tree("commit_c_local".to_string(), "tree_c".to_string())
            .with_tree("commit_c_remote".to_string(), "tree_c".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/bbb12345".to_string(), "jnb/ccc12345".to_string()])
            .with_pr("jnb/ccc12345".to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        let mut stdout = Vec::new();
        let result = app
            .cmd_update("@", Some("Update commit C"), &mut stdout)
            .await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("parent PR"));
        assert!(error_msg.contains("out of date"));
        assert!(error_msg.contains("jnb/bbb12345"));
    }

    #[tokio::test]
    async fn test_cmd_update_auto_restack_when_diffs_match() {
        // Set up a commit where the diff introduced by each commit matches (pure restack)
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "local_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![("abc12345".to_string(), "local_commit".to_string())],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/abc12345".to_string(), "remote_commit".to_string())
            .with_tree("local_commit".to_string(), "same_tree".to_string())
            .with_tree("remote_commit".to_string(), "same_tree".to_string())
            .with_tree("main_commit".to_string(), "main_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()])
            .with_pr("jnb/abc12345".to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        // Update without message - should auto-detect restack
        let mut stdout = Vec::new();
        let result = app.cmd_update("@", None, &mut stdout).await;
        if let Err(e) = &result {
            eprintln!("Error: {}", e);
        }
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_update_errors_when_no_message_and_diffs_differ() {
        // Set up a commit where the diff introduced differs (not a pure restack)
        let mock_jj = MockJujutsu {
            change_id: "abc12345".to_string(),
            commit_id: "local_commit".to_string(),
            commit_message: "Test commit message".to_string(),
            parent_change_ids: vec![],
            stack_heads: vec![],
            stack_changes: vec![("abc12345".to_string(), "local_commit".to_string())],
            change_to_commit: std::collections::HashMap::new(),
            change_to_parents: std::collections::HashMap::new(),
        };
        let mock_git = MockGit::new()
            .with_branch("master".to_string(), "main_commit".to_string())
            .with_branch("jnb/abc12345".to_string(), "remote_commit".to_string())
            .with_tree("local_commit".to_string(), "new_tree".to_string())
            .with_tree("remote_commit".to_string(), "old_tree".to_string())
            .with_tree("main_commit".to_string(), "main_tree".to_string());
        let mock_gh = MockGithub::new(GLOBAL_BRANCH_PREFIX.to_string())
            .with_branches(vec!["jnb/abc12345".to_string()])
            .with_pr("jnb/abc12345".to_string());

        let app = App::new(mock_jj, mock_git, mock_gh);

        // Update without message - should error since diffs differ
        let mut stdout = Vec::new();
        let result = app.cmd_update("@", None, &mut stdout).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("no message provided"));
    }
}
