#![allow(async_fn_in_trait)]

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
#[cfg(test)]
use mockall::automock;
use tokio::process::Command;

// -----------------------------------------------------------------------------
// GithubOps trait

/// Operations for interacting with GitHub
#[cfg_attr(test, automock)]
pub trait GithubOps {
    async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>>;

    /// Check if an open PR exists for the branch
    async fn pr_is_open(&self, branch: &str) -> Result<bool>;

    /// Get the PR URL for a branch, returns None if no PR exists
    async fn pr_url(&self, branch: &str) -> Result<Option<String>>;

    /// Create a new PR and return the PR URL
    async fn pr_create(
        &self,
        pr_branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String>;

    /// Edit an existing PR and return the PR URL
    async fn pr_edit(&self, pr_branch: &str, base_branch: &str) -> Result<String>;

    /// Get the diff for a PR (cumulative diff from base to head)
    async fn pr_diff(&self, pr_branch: &str) -> Result<String>;

    /// Delete a remote branch
    async fn delete_branch(&self, branch: &str) -> Result<()>;
}

// -----------------------------------------------------------------------------
// RealGithub

/// Real implementation that calls the gh CLI
pub struct RealGithub;

impl GithubOps for RealGithub {
    async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let api_path = format!("/repos/:owner/:repo/git/matching-refs/heads/{}", prefix);

        let output = Command::new("gh")
            .args([
                "api",
                &api_path,
                "--jq",
                ".[].ref | sub(\"^refs/heads/\";\"\")",
            ])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "gh command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let branches: Vec<String> = String::from_utf8(output.stdout)?
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(branches)
    }

    async fn pr_is_open(&self, pr_branch: &str) -> Result<bool> {
        let output = Command::new("gh")
            .args(["pr", "view", pr_branch, "--json", "state", "--jq", ".state"])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            // PR doesn't exist
            return Ok(false);
        }

        let state = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(state == "OPEN")
    }

    async fn pr_url(&self, pr_branch: &str) -> Result<Option<String>> {
        let output = Command::new("gh")
            .args(["pr", "view", pr_branch, "--json", "url", "--jq", ".url"])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            // PR doesn't exist
            return Ok(None);
        }

        let url = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(Some(url))
    }

    async fn pr_create(
        &self,
        pr_branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String> {
        let output = Command::new("gh")
            .args([
                "pr",
                "create",
                "--head",
                pr_branch,
                "--base",
                base_branch,
                "--draft",
                "--title",
                title,
                "--body",
                body,
            ])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "gh command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // gh pr create outputs the PR URL to stdout
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn pr_edit(&self, pr_branch: &str, base_branch: &str) -> Result<String> {
        let output = Command::new("gh")
            .args(["pr", "edit", pr_branch, "--base", base_branch])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "gh command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Get the PR URL after editing
        let url_output = Command::new("gh")
            .args(["pr", "view", pr_branch, "--json", "url", "--jq", ".url"])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !url_output.status.success() {
            return Err(anyhow!(
                "gh command failed: {}",
                String::from_utf8_lossy(&url_output.stderr)
            ));
        }

        Ok(String::from_utf8(url_output.stdout)?.trim().to_string())
    }

    async fn pr_diff(&self, pr_branch: &str) -> Result<String> {
        let output = Command::new("gh")
            .args(["pr", "diff", pr_branch])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "gh command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn delete_branch(&self, branch: &str) -> Result<()> {
        let api_path = format!("/repos/:owner/:repo/git/refs/heads/{}", branch);

        let output = Command::new("gh")
            .args(["api", "-X", "DELETE", &api_path])
            .output()
            .await
            .context("Failed to execute gh command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "gh command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }
}
