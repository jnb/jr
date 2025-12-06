#![allow(async_fn_in_trait)]

use std::path;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tokio::process::Command;
use tracing::instrument;

use super::github_curl::GithubCurlClient;

// -----------------------------------------------------------------------------
// Types

/// Client to interact with GitHub API.
pub struct GithubClient {
    owner: String,
    repo: String,
    http_client: GithubCurlClient,
}

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
// GithubClient impl

impl GithubClient {
    pub async fn new(token: String, path: path::PathBuf) -> Result<Self> {
        let (owner, repo) = Self::detect_owner_and_repo(&path).await?;
        let http_client = GithubCurlClient::new(token);

        Ok(Self {
            owner,
            repo,
            http_client,
        })
    }

    /// Detect owner and repo from git remote URL
    async fn detect_owner_and_repo(path: &path::Path) -> Result<(String, String)> {
        let output = Command::new("git")
            .current_dir(path)
            .args(["config", "--get", "remote.origin.url"])
            .output()
            .await
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

    /// Helper to get PR number from branch name
    #[instrument(skip_all)]
    async fn get_pr_number(&self, branch: &str) -> Result<Option<u64>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls?head={}:{}&state=all",
            self.owner, self.repo, self.owner, branch
        );

        let response = self
            .http_client
            .get(&url, "application/vnd.github+json")
            .await?;
        let prs: Vec<PullRequest> = serde_json::from_str(&response)?;
        Ok(prs.first().map(|pr| pr.number))
    }

    #[instrument(skip_all)]
    pub async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/git/matching-refs/heads/{}",
            self.owner, self.repo, prefix
        );

        let response = self
            .http_client
            .get(&url, "application/vnd.github+json")
            .await?;
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

    /// Check if an open PR exists for the branch
    #[instrument(skip_all)]
    pub async fn pr_is_open(&self, pr_branch: &str) -> Result<bool> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls?head={}:{}&state=open",
            self.owner, self.repo, self.owner, pr_branch
        );

        let response = self
            .http_client
            .get(&url, "application/vnd.github+json")
            .await;
        match response {
            Ok(resp) => {
                let prs: Vec<PullRequest> = serde_json::from_str(&resp)?;
                Ok(!prs.is_empty())
            }
            Err(_) => Ok(false),
        }
    }

    /// Get the PR URL for a branch, returns None if no PR exists
    #[instrument(skip_all)]
    pub async fn pr_url(&self, pr_branch: &str) -> Result<Option<String>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls?head={}:{}&state=all",
            self.owner, self.repo, self.owner, pr_branch
        );

        let response = self
            .http_client
            .get(&url, "application/vnd.github+json")
            .await;
        match response {
            Ok(resp) => {
                let prs: Vec<PullRequest> = serde_json::from_str(&resp)?;
                Ok(prs.first().map(|pr| pr.html_url.clone()))
            }
            Err(_) => Ok(None),
        }
    }

    /// Create a new PR and return the PR URL
    #[instrument(skip_all)]
    pub async fn pr_create(
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
        let response = self.http_client.post(&url, &json_data).await?;
        let pr: PullRequest = serde_json::from_str(&response)?;
        Ok(pr.html_url)
    }

    /// Edit an existing PR and return the PR URL
    #[instrument(skip_all)]
    pub async fn pr_edit(&self, pr_branch: &str, base_branch: &str) -> Result<String> {
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
        let response = self.http_client.patch(&url, &json_data).await?;
        let pr: PullRequest = serde_json::from_str(&response)?;
        Ok(pr.html_url)
    }

    /// Get the diff for a PR (cumulative diff from base to head)
    #[instrument(skip_all)]
    pub async fn pr_diff(&self, pr_branch: &str) -> Result<String> {
        let pr_number = self
            .get_pr_number(pr_branch)
            .await?
            .context("PR not found for branch")?;

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            self.owner, self.repo, pr_number
        );

        self.http_client
            .get(&url, "application/vnd.github.diff")
            .await
    }

    /// Delete a remote branch
    #[instrument(skip_all)]
    pub async fn delete_branch(&self, branch: &str) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/git/refs/heads/{}",
            self.owner, self.repo, branch
        );

        self.http_client.delete(&url).await
    }
}
