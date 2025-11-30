use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;
#[cfg(test)]
use mockall::automock;

// -----------------------------------------------------------------------------
// GitOps trait

/// Operations for interacting with Git
#[cfg_attr(test, automock)]
#[async_trait(?Send)]
pub trait GitOps {
    async fn get_tree(&self, commit_id: &str) -> Result<String>;
    async fn get_branch(&self, branch: &str) -> Result<String>;
    async fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<String>;
    async fn commit_tree_merge(&self, tree: &str, parents: Vec<String>, message: &str) -> Result<String>;
    async fn update_branch(&self, branch: &str, commit: &str) -> Result<()>;
    async fn push_branch(&self, branch: &str) -> Result<()>;

    /// Check if `commit` is an ancestor of `descendant`.
    /// Returns true if `commit` is reachable from `descendant` by following parent links.
    /// In other words, returns true if `descendant` contains all changes from `commit`.
    async fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool>;

    /// Get a canonical representation of the changes introduced by a commit.
    /// Returns a string representing the diff (file names and status) that can be compared.
    async fn get_commit_diff(&self, commit_id: &str) -> Result<String>;
}

// -----------------------------------------------------------------------------
// RealGithub

/// Real implementation that calls the git CLI
pub struct RealGit;

#[async_trait(?Send)]
impl GitOps for RealGit {
    async fn get_tree(&self, commit_id: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", &format!("{}^{{tree}}", commit_id)])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn get_branch(&self, branch: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", &format!("origin/{}", branch)])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["commit-tree", tree, "-p", parent, "-m", message])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn commit_tree_merge(&self, tree: &str, parents: Vec<String>, message: &str) -> Result<String> {
        let mut args = vec!["commit-tree".to_string(), tree.to_string()];
        for parent in &parents {
            args.push("-p".to_string());
            args.push(parent.clone());
        }
        args.push("-m".to_string());
        args.push(message.to_string());

        let output = Command::new("git")
            .args(&args)
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn update_branch(&self, branch: &str, commit: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["update-ref", &format!("refs/heads/{}", branch), commit])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    async fn push_branch(&self, branch: &str) -> Result<()> {
        let refspec = format!("refs/heads/{}:refs/heads/{}", branch, branch);
        let output = Command::new("git")
            .args(["push", "-u", "origin", &refspec])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    async fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool> {
        let output = Command::new("git")
            .args(["merge-base", "--is-ancestor", commit, descendant])
            .output()
            .await
            .context("Failed to execute git command")?;

        // Exit code 0 means it is an ancestor, 1 means it's not
        Ok(output.status.success())
    }

    async fn get_commit_diff(&self, commit_id: &str) -> Result<String> {
        // Use diff-tree to get the full textual diff introduced by this commit
        // -p: generate patch (full diff with +/- lines)
        // --no-commit-id: don't show the commit ID in output
        let output = Command::new("git")
            .args(["diff-tree", "-p", "--no-commit-id", commit_id])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}
