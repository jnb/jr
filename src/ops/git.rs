#![allow(async_fn_in_trait)]

use std::fmt::Display;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
#[cfg(test)]
use mockall::automock;
use tokio::process::Command;

// -----------------------------------------------------------------------------
// GitOps trait

/// Operations for interacting with Git
#[cfg_attr(test, automock)]
pub trait GitOps {
    async fn get_tree(&self, commit_id: &CommitId) -> Result<String>;
    async fn get_branch_tip(&self, branch: &str) -> Result<CommitId>;
    async fn commit_tree(&self, tree: &str, parent: &CommitId, message: &str) -> Result<CommitId>;
    async fn commit_tree_merge(
        &self,
        tree: &str,
        parents: Vec<CommitId>,
        message: &str,
    ) -> Result<CommitId>;
    async fn update_branch(&self, branch: &str, commit_id: &CommitId) -> Result<()>;
    async fn push_branch(&self, branch: &str) -> Result<()>;
    async fn delete_local_branch(&self, branch: &str) -> Result<()>;

    /// Check if `commit` is an ancestor of `descendant`.
    /// Returns true if `commit` is reachable from `descendant` by following parent links.
    /// In other words, returns true if `descendant` contains all changes from `commit`.
    async fn is_ancestor(&self, commit: &CommitId, descendant: &CommitId) -> Result<bool>;

    /// Get a canonical representation of the changes introduced by a commit.
    /// Returns a string representing the diff (file names and status) that can be compared.
    async fn get_commit_diff(&self, commit_id: &CommitId) -> Result<String>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct CommitId(pub String);

impl Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// -----------------------------------------------------------------------------
// RealGithub

/// Real implementation that calls the git CLI
pub struct RealGit;

impl GitOps for RealGit {
    async fn get_tree(&self, commit_id: &CommitId) -> Result<String> {
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

    async fn get_branch_tip(&self, branch: &str) -> Result<CommitId> {
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

        Ok(CommitId(
            String::from_utf8(output.stdout)?.trim().to_string(),
        ))
    }

    async fn commit_tree(&self, tree: &str, parent: &CommitId, message: &str) -> Result<CommitId> {
        let output = Command::new("git")
            .args(["commit-tree", tree, "-p", &parent.0, "-m", message])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(CommitId(
            String::from_utf8(output.stdout)?.trim().to_string(),
        ))
    }

    async fn commit_tree_merge(
        &self,
        tree: &str,
        parents: Vec<CommitId>,
        message: &str,
    ) -> Result<CommitId> {
        let mut args = vec!["commit-tree".to_string(), tree.to_string()];
        for parent in &parents {
            args.push("-p".to_string());
            args.push(parent.clone().0);
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

        Ok(CommitId(
            String::from_utf8(output.stdout)?.trim().to_string(),
        ))
    }

    async fn update_branch(&self, branch: &str, commit_id: &CommitId) -> Result<()> {
        let output = Command::new("git")
            .args([
                "update-ref",
                &format!("refs/heads/{}", branch),
                &commit_id.0,
            ])
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

    async fn delete_local_branch(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["update-ref", "-d", &format!("refs/heads/{}", branch)])
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

    async fn is_ancestor(&self, commit: &CommitId, descendant: &CommitId) -> Result<bool> {
        let output = Command::new("git")
            .args(["merge-base", "--is-ancestor", &commit.0, &descendant.0])
            .output()
            .await
            .context("Failed to execute git command")?;

        // Exit code 0 means it is an ancestor, 1 means it's not
        Ok(output.status.success())
    }

    async fn get_commit_diff(&self, commit_id: &CommitId) -> Result<String> {
        // Use diff-tree to get the full textual diff introduced by this commit
        // -p: generate patch (full diff with +/- lines)
        // --no-commit-id: don't show the commit ID in output
        let output = Command::new("git")
            .args(["diff-tree", "-p", "--no-commit-id", &commit_id.0])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Don't trim - we want to preserve trailing newlines to match GitHub API diff format
        Ok(String::from_utf8(output.stdout)?)
    }
}
