use std::fmt::Display;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use tokio::process::Command;

// -----------------------------------------------------------------------------
// Types

/// Git client.
pub struct GitClient {
    path: std::path::PathBuf,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CommitId(pub String);

impl Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// -----------------------------------------------------------------------------
// GitClient impl

impl GitClient {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    pub async fn get_tree(&self, commit_id: &CommitId) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["rev-parse", &format!("{}^{{tree}}", commit_id)])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    pub async fn get_branch_tip(&self, branch: &str) -> Result<CommitId> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["rev-parse", &format!("origin/{}", branch)])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(CommitId(
            String::from_utf8(output.stdout)?.trim().to_string(),
        ))
    }

    pub async fn commit_tree(
        &self,
        tree: &str,
        parents: Vec<&CommitId>,
        message: &str,
    ) -> Result<CommitId> {
        let mut args = vec!["commit-tree".to_string(), tree.to_string()];
        for parent in &parents {
            args.push("-p".to_string());
            args.push(parent.0.clone());
        }
        args.push("-m".to_string());
        args.push(message.to_string());

        let output = Command::new("git")
            .current_dir(&self.path)
            .args(&args)
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(CommitId(
            String::from_utf8(output.stdout)?.trim().to_string(),
        ))
    }

    pub async fn update_branch(&self, branch: &str, commit_id: &CommitId) -> Result<()> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args([
                "update-ref",
                &format!("refs/heads/{}", branch),
                &commit_id.0,
            ])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    pub async fn push_branch(&self, branch: &str) -> Result<()> {
        let refspec = format!("refs/heads/{}:refs/heads/{}", branch, branch);
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["push", "-u", "origin", &refspec])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    pub async fn delete_local_branch(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["update-ref", "-d", &format!("refs/heads/{}", branch)])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Push a commit directly to a remote branch without creating a local branch
    pub async fn push_commit_to_branch(&self, commit_id: &CommitId, branch: &str) -> Result<()> {
        let refspec = format!("{}:refs/heads/{}", commit_id.0, branch);
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["push", "-u", "origin", &refspec])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Delete a remote branch
    pub async fn delete_branch(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["push", "origin", "--delete", branch])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Check if `commit` is an ancestor of `descendant`.
    /// Returns true if `commit` is reachable from `descendant` by following parent links.
    /// In other words, returns true if `descendant` contains all changes from `commit`.
    pub async fn is_ancestor(&self, commit: &CommitId, descendant: &CommitId) -> Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["merge-base", "--is-ancestor", &commit.0, &descendant.0])
            .output()
            .await
            .context("Failed to execute git command")?;

        // Exit code 0 means it is an ancestor, 1 means it's not
        Ok(output.status.success())
    }

    /// Get a canonical representation of the changes introduced by a commit.
    /// Returns a string representing the diff (file names and status) that can be compared.
    pub async fn get_commit_diff(&self, commit_id: &CommitId) -> Result<String> {
        // Use diff-tree to get the full textual diff introduced by this commit
        // -p: generate patch (full diff with +/- lines)
        // --no-commit-id: don't show the commit ID in output
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(["diff-tree", "-p", "--no-commit-id", &commit_id.0])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Don't trim - we want to preserve trailing newlines to match GitHub API diff format
        Ok(String::from_utf8(output.stdout)?)
    }

    /// Get the remote git branches for a commit.
    /// Returns branch names with "origin/" prefix stripped (e.g., ["main", "test/abc12345"])
    pub async fn get_git_remote_branches(&self, commit_id: &CommitId) -> Result<Vec<String>> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args([
                "branch",
                "-r",
                "--points-at",
                &commit_id.0,
                "--format=%(refname:short)",
            ])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let output_str = String::from_utf8(output.stdout)?.trim().to_string();

        // Parse git refs and filter for remote branches only
        let branches: Vec<String> = output_str
            .lines()
            .filter_map(|line| line.strip_prefix("origin/").map(|s| s.to_string()))
            .collect();

        Ok(branches)
    }

    /// Find remote branches matching a prefix.
    /// Returns branch names with "origin/" prefix stripped (e.g., ["test/abc123", "test/xyz789"])
    pub async fn find_branches_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = format!("refs/remotes/origin/{}", prefix);
        let output = Command::new("git")
            .current_dir(&self.path)
            .args([
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("{}*", pattern),
            ])
            .output()
            .await
            .context("Failed to execute git command")?;

        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let output_str = String::from_utf8(output.stdout)?.trim().to_string();

        let branches: Vec<String> = output_str
            .lines()
            .filter_map(|line| line.strip_prefix("origin/").map(|s| s.to_string()))
            .collect();

        Ok(branches)
    }
}
