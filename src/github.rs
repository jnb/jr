use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;

// -----------------------------------------------------------------------------
// GithubOps trait

/// Operations for interacting with GitHub
#[async_trait(?Send)]
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
}

// -----------------------------------------------------------------------------
// RealGithub

/// Real implementation that calls the gh CLI
pub struct RealGithub {
    /// Global branch prefix used when searching for branches
    pub branch_prefix: String,
}

impl RealGithub {
    pub fn new(branch_prefix: String) -> Self {
        Self { branch_prefix }
    }
}

#[async_trait(?Send)]
impl GithubOps for RealGithub {
    async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let search_prefix = format!("{}{}", self.branch_prefix, prefix);
        let api_path = format!(
            "/repos/:owner/:repo/git/matching-refs/heads/{}",
            search_prefix
        );

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
}

// -----------------------------------------------------------------------------
// MockGithub

#[cfg(test)]
pub struct MockGithub {
    pub branch_prefix: String,
    pub branches: Vec<String>,
    pub prs: std::cell::RefCell<std::collections::HashSet<String>>,
    pub open_prs: std::cell::RefCell<std::collections::HashSet<String>>,
    pub pr_urls: std::cell::RefCell<std::collections::HashMap<String, String>>,
    pub created_prs: std::cell::RefCell<Vec<(String, String)>>,
    pub edited_prs: std::cell::RefCell<Vec<(String, String)>>,
    pub pr_diffs: std::cell::RefCell<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl MockGithub {
    pub fn new(branch_prefix: String) -> Self {
        Self {
            branch_prefix,
            branches: Vec::new(),
            prs: std::cell::RefCell::new(std::collections::HashSet::new()),
            open_prs: std::cell::RefCell::new(std::collections::HashSet::new()),
            pr_urls: std::cell::RefCell::new(std::collections::HashMap::new()),
            created_prs: std::cell::RefCell::new(Vec::new()),
            edited_prs: std::cell::RefCell::new(Vec::new()),
            pr_diffs: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    pub fn with_branches(mut self, branches: Vec<String>) -> Self {
        self.branches = branches;
        self
    }

    pub fn with_pr(self, branch: String) -> Self {
        self.prs.borrow_mut().insert(branch.clone());
        self.open_prs.borrow_mut().insert(branch.clone());
        self.pr_urls.borrow_mut().insert(
            branch.clone(),
            format!("https://github.com/test/repo/pull/{}", branch),
        );
        self
    }

    pub fn with_closed_pr(self, branch: String) -> Self {
        self.prs.borrow_mut().insert(branch.clone());
        self.pr_urls.borrow_mut().insert(
            branch.clone(),
            format!("https://github.com/test/repo/pull/{}", branch),
        );
        self
    }

    pub fn with_pr_diff(self, branch: String, diff: String) -> Self {
        self.pr_diffs.borrow_mut().insert(branch, diff);
        self
    }
}

#[cfg(test)]
#[async_trait(?Send)]
impl GithubOps for MockGithub {
    async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let search_prefix = format!("{}{}", self.branch_prefix, prefix);
        Ok(self
            .branches
            .iter()
            .filter(|b| b.starts_with(&search_prefix))
            .cloned()
            .collect())
    }

    async fn pr_is_open(&self, branch: &str) -> Result<bool> {
        Ok(self.open_prs.borrow().contains(branch))
    }

    async fn pr_url(&self, branch: &str) -> Result<Option<String>> {
        Ok(self.pr_urls.borrow().get(branch).cloned())
    }

    async fn pr_create(
        &self,
        pr_branch: &str,
        base_branch: &str,
        _title: &str,
        _body: &str,
    ) -> Result<String> {
        self.created_prs
            .borrow_mut()
            .push((pr_branch.to_string(), base_branch.to_string()));
        self.prs.borrow_mut().insert(pr_branch.to_string());
        self.open_prs.borrow_mut().insert(pr_branch.to_string());
        let url = format!("https://github.com/test/repo/pull/123");
        self.pr_urls
            .borrow_mut()
            .insert(pr_branch.to_string(), url.clone());
        Ok(url)
    }

    async fn pr_edit(&self, pr_branch: &str, base_branch: &str) -> Result<String> {
        self.edited_prs
            .borrow_mut()
            .push((pr_branch.to_string(), base_branch.to_string()));
        Ok(format!("https://github.com/test/repo/pull/123"))
    }

    async fn pr_diff(&self, pr_branch: &str) -> Result<String> {
        self.pr_diffs
            .borrow()
            .get(pr_branch)
            .cloned()
            .ok_or_else(|| anyhow!("PR diff not found for branch: {}", pr_branch))
    }
}
