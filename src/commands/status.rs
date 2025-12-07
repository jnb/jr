use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;
use futures_util::future::join_all;
use log::debug;

use crate::App;
use crate::clients::jujutsu;
use crate::diff_utils::normalize_diff;

impl App {
    pub async fn cmd_status(
        &self,
        stdout: &mut impl std::io::Write,
        stderr: &mut impl std::io::Write,
    ) -> Result<()> {
        let current_commit = self.jj.get_commit("@").await?;

        let stack_heads = self.jj.get_stack_heads("@").await?;
        let stack_commits = if stack_heads.is_empty() {
            // Current commit is on trunk or no stack exists
            return Ok(());
        } else if stack_heads.len() == 1 {
            let head_commit_id = &stack_heads[0].commit_id.0;
            self.jj.get_stack_ancestors(head_commit_id).await?
        } else {
            writeln!(
                stderr,
                "Warning: Multiple stack heads detected. Showing stack from @ to trunk."
            )?;
            self.jj.get_stack_ancestors("@").await?
        };

        let all_pr_branches = self
            .git
            .find_branches_with_prefix(&self.config.github_branch_prefix)
            .await?;

        // Collect all unique branches we need pr_diffs for (changes + their parents)
        let mut branches_needing_diffs = std::collections::HashSet::new();
        for commit in &stack_commits {
            let expected_branch = commit
                .change_id
                .branch_name(&self.config.github_branch_prefix);
            if all_pr_branches.contains(&expected_branch) {
                branches_needing_diffs.insert(expected_branch);
            }

            // Also collect parent branches
            for parent_change_id in &commit.parent_change_ids {
                let parent_branch = parent_change_id.branch_name(&self.config.github_branch_prefix);
                if all_pr_branches.contains(&parent_branch) {
                    branches_needing_diffs.insert(parent_branch);
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
        let pr_url_futures: Vec<_> = stack_commits
            .iter()
            .map(|commit| {
                let expected_branch = commit
                    .change_id
                    .branch_name(&self.config.github_branch_prefix);
                let branch_exists = all_pr_branches.contains(&expected_branch);

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
        let base_branch_futures: Vec<_> = stack_commits
            .iter()
            .map(|commit| self.find_previous_branch(&commit.change_id.0))
            .collect();
        let base_branches = join_all(base_branch_futures).await;

        // Display results
        for (i, commit) in stack_commits.iter().enumerate() {
            let expected_branch = commit
                .change_id
                .branch_name(&self.config.github_branch_prefix);
            let pr_url_result = &pr_urls[i];
            let base_branch_result = &base_branches[i];

            // Get commit and extract title from message
            let commit = self.jj.get_commit(&commit.commit_id.0).await?;
            let commit_title = commit.message.title.as_deref().unwrap_or("");

            // Get abbreviated change ID (4 chars, matching jj status default)
            let abbreviated_change_id = &commit.change_id.short_id();

            // Check if parent PR is outdated
            let parent_pr_outdated = self
                .is_parent_pr_outdated(&commit.change_id.0, &all_pr_branches, &pr_diffs)
                .await?;

            // Get base branch (or default to empty string if error)
            let base_branch = base_branch_result.as_ref().ok();

            self.show_change_status_with_data(
                &expected_branch,
                &commit,
                &current_commit.change_id,
                &all_pr_branches,
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

    #[allow(clippy::too_many_arguments)]
    async fn show_change_status_with_data(
        &self,
        expected_branch: &str,
        commit: &jujutsu::JujutsuCommit,
        current_change_id: &jujutsu::JujutsuChangeId,
        all_branches: &[String],
        pr_url_result: &Result<Option<String>>,
        commit_title: &str,
        abbreviated_change_id: &str,
        parent_pr_outdated: bool,
        _base_branch: Option<&String>,
        pr_diffs: &HashMap<String, String>,
        stdout: &mut impl std::io::Write,
    ) -> Result<()> {
        let is_current = commit.change_id == *current_change_id;
        let branch_exists = all_branches.contains(&expected_branch.to_string());

        // Determine status symbol
        let status_symbol = if branch_exists {
            // Check if PR exists
            match pr_url_result {
                Ok(Some(_)) => {
                    // Compare local single commit diff vs cumulative PR diff from cache
                    let local_diff = self.git.get_commit_diff(&commit.commit_id).await;

                    match local_diff {
                        Ok(local_diff) => {
                            if let Some(pr_diff) = pr_diffs.get(expected_branch) {
                                // Check if base branch has moved (not an ancestor of PR branch)
                                let base_has_moved = if let Some(base_branch) = _base_branch {
                                    // Get PR branch tip
                                    if let Ok(pr_branch_tip) =
                                        self.git.get_branch_tip(expected_branch).await
                                    {
                                        // Get base branch tip
                                        if let Ok(base_tip) =
                                            self.git.get_branch_tip(base_branch).await
                                        {
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

                                // Normalize both diffs to ignore differences in index line hash lengths
                                let normalized_local = normalize_diff(&local_diff);
                                let normalized_pr = normalize_diff(pr_diff);

                                if normalized_local == normalized_pr {
                                    // Diffs match - check if parent changed or base moved
                                    if parent_pr_outdated || base_has_moved {
                                        "↻" // Needs restack (parent changed or base moved)
                                    } else {
                                        "✓" // Up to date
                                    }
                                } else {
                                    debug!("local_diff: {local_diff}");
                                    debug!("pr_diff {pr_diff}");
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

    /// Check if any parent PR is out of date
    /// A parent is outdated if its local single commit diff differs from cumulative remote diff,
    /// or if the parent itself needs a restack (recursive check)
    async fn is_parent_pr_outdated(
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
            let parent_branch = parent_change_id.branch_name(&self.config.github_branch_prefix);

            // If this parent has a PR branch, check if it's outdated
            if all_branches.contains(&parent_branch) {
                // Compare parent's local single commit diff vs cumulative PR diff from cache
                let parent_commit = self.jj.get_commit(&parent_change_id.0).await?;
                let parent_local_diff = self.git.get_commit_diff(&parent_commit.commit_id).await?;

                if let Some(parent_pr_diff) = pr_diffs.get(&parent_branch) {
                    // Normalize both diffs to ignore differences in index line hash lengths
                    let normalized_parent_local = normalize_diff(&parent_local_diff);
                    let normalized_parent_pr = normalize_diff(parent_pr_diff);

                    if normalized_parent_local != normalized_parent_pr {
                        return Ok(true); // Parent has local changes
                    }
                }

                // Check if parent's base has moved
                if let Ok(parent_base_branch) = self.find_previous_branch(&parent_change_id.0).await
                    && let Ok(parent_pr_commit) = self.git.get_branch_tip(&parent_branch).await
                    && let Ok(base_tip) = self.git.get_branch_tip(&parent_base_branch).await
                {
                    // If base is not an ancestor of parent's PR, base has moved
                    if !self.git.is_ancestor(&base_tip, &parent_pr_commit).await? {
                        return Ok(true); // Parent's base has moved, needs restack
                    }
                }

                // Recursively check if the parent itself needs a restack
                if Box::pin(self.is_parent_pr_outdated(&parent_change_id.0, all_branches, pr_diffs))
                    .await?
                {
                    return Ok(true); // Parent's ancestor is outdated
                }
            }
        }

        Ok(false)
    }
}
