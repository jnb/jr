#![allow(async_fn_in_trait)]

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
#[cfg(test)]
use mockall::automock;
use serde::Deserialize;
use serde::Serialize;
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
// GitHub API types

#[derive(Debug, Deserialize)]
struct GitRef {
    #[serde(rename = "ref")]
    ref_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PullRequest {
    number: u64,
    html_url: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct CreatePullRequest {
    title: String,
    body: String,
    head: String,
    base: String,
    draft: bool,
}

#[derive(Debug, Serialize)]
struct UpdatePullRequest {
    base: String,
}

// -----------------------------------------------------------------------------
// RealGithub

/// Real implementation that makes HTTP requests to GitHub API via curl
pub struct RealGithub {
    token: String,
    owner: String,
    repo: String,
}

impl RealGithub {
    pub fn new(token: String) -> Result<Self> {
        let (owner, repo) = Self::detect_owner_and_repo()?;

        Ok(Self { token, owner, repo })
    }

    /// Detect owner and repo from git remote URL
    fn detect_owner_and_repo() -> Result<(String, String)> {
        let output = std::process::Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .output()
            .context("Failed to get git remote URL")?;

        if !output.status.success() {
            anyhow::bail!("No git remote 'origin' configured");
        }

        let url = String::from_utf8(output.stdout)?.trim().to_string();

        // Parse URLs like:
        // git@github.com:owner/repo.git
        // https://github.com/owner/repo.git
        let parts = if url.starts_with("git@github.com:") {
            url.strip_prefix("git@github.com:")
                .context("Invalid GitHub URL format")?
        } else if url.starts_with("https://github.com/") {
            url.strip_prefix("https://github.com/")
                .context("Invalid GitHub URL format")?
        } else {
            anyhow::bail!("Remote URL is not a GitHub URL: {}", url);
        };

        let parts = parts.strip_suffix(".git").unwrap_or(parts);
        let mut split = parts.split('/');
        let owner = split
            .next()
            .context("Could not parse owner from GitHub URL")?
            .to_string();
        let repo = split
            .next()
            .context("Could not parse repo from GitHub URL")?
            .to_string();

        Ok((owner, repo))
    }

    /// Make a GET request to GitHub API
    async fn curl_get(&self, url: &str, accept: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                &format!("Accept: {}", accept),
                "-H",
                "User-Agent: jr-cli",
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?)
    }

    /// Make a POST request to GitHub API
    async fn curl_post(&self, url: &str, json_data: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-X",
                "POST",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "Content-Type: application/json",
                "-H",
                "User-Agent: jr-cli",
                "-d",
                json_data,
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?)
    }

    /// Make a PATCH request to GitHub API
    async fn curl_patch(&self, url: &str, json_data: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-X",
                "PATCH",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "Content-Type: application/json",
                "-H",
                "User-Agent: jr-cli",
                "-d",
                json_data,
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?)
    }

    /// Make a DELETE request to GitHub API
    async fn curl_delete(&self, url: &str) -> Result<()> {
        let output = Command::new("curl")
            .args([
                "-s",
                "-X",
                "DELETE",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "User-Agent: jr-cli",
                url,
            ])
            .output()
            .await
            .context("Failed to execute curl command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "curl command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Helper to get PR number from branch name
    async fn get_pr_number(&self, branch: &str) -> Result<Option<u64>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls?head={}:{}&state=all",
            self.owner, self.repo, self.owner, branch
        );

        let response = self.curl_get(&url, "application/vnd.github+json").await?;
        let prs: Vec<PullRequest> = serde_json::from_str(&response)?;
        Ok(prs.first().map(|pr| pr.number))
    }
}

impl GithubOps for RealGithub {
    async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/git/matching-refs/heads/{}",
            self.owner, self.repo, prefix
        );

        let response = self.curl_get(&url, "application/vnd.github+json").await?;
        let refs: Vec<GitRef> = serde_json::from_str(&response)?;

        let branches = refs
            .into_iter()
            .map(|r| {
                r.ref_name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&r.ref_name)
                    .to_string()
            })
            .collect();

        Ok(branches)
    }

    async fn pr_is_open(&self, pr_branch: &str) -> Result<bool> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls?head={}:{}&state=open",
            self.owner, self.repo, self.owner, pr_branch
        );

        let response = self.curl_get(&url, "application/vnd.github+json").await;
        match response {
            Ok(resp) => {
                let prs: Vec<PullRequest> = serde_json::from_str(&resp)?;
                Ok(!prs.is_empty())
            }
            Err(_) => Ok(false),
        }
    }

    async fn pr_url(&self, pr_branch: &str) -> Result<Option<String>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls?head={}:{}&state=all",
            self.owner, self.repo, self.owner, pr_branch
        );

        let response = self.curl_get(&url, "application/vnd.github+json").await;
        match response {
            Ok(resp) => {
                let prs: Vec<PullRequest> = serde_json::from_str(&resp)?;
                Ok(prs.first().map(|pr| pr.html_url.clone()))
            }
            Err(_) => Ok(None),
        }
    }

    async fn pr_create(
        &self,
        pr_branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls",
            self.owner, self.repo
        );

        let request_body = CreatePullRequest {
            title: title.to_string(),
            body: body.to_string(),
            head: pr_branch.to_string(),
            base: base_branch.to_string(),
            draft: true,
        };

        let json_data = serde_json::to_string(&request_body)?;
        let response = self.curl_post(&url, &json_data).await?;
        let pr: PullRequest = serde_json::from_str(&response)?;
        Ok(pr.html_url)
    }

    async fn pr_edit(&self, pr_branch: &str, base_branch: &str) -> Result<String> {
        let pr_number = self
            .get_pr_number(pr_branch)
            .await?
            .context("PR not found for branch")?;

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            self.owner, self.repo, pr_number
        );

        let request_body = UpdatePullRequest {
            base: base_branch.to_string(),
        };

        let json_data = serde_json::to_string(&request_body)?;
        let response = self.curl_patch(&url, &json_data).await?;
        let pr: PullRequest = serde_json::from_str(&response)?;
        Ok(pr.html_url)
    }

    async fn pr_diff(&self, pr_branch: &str) -> Result<String> {
        let pr_number = self
            .get_pr_number(pr_branch)
            .await?
            .context("PR not found for branch")?;

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            self.owner, self.repo, pr_number
        );

        self.curl_get(&url, "application/vnd.github.diff").await
    }

    async fn delete_branch(&self, branch: &str) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/git/refs/heads/{}",
            self.owner, self.repo, branch
        );

        self.curl_delete(&url).await
    }
}
