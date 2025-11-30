use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;
#[cfg(test)]
use mockall::automock;

// -----------------------------------------------------------------------------
// Types

/// Operations for interacting with Jujutsu version control
#[cfg_attr(test, automock)]
#[async_trait(?Send)]
pub trait JujutsuOps {
    /// Get complete commit information for a revision
    async fn get_commit(&self, revision: &str) -> Result<Commit>;

    /// Get the head commits of the current stack (descendants of @ that aren't ancestors of trunk)
    /// Returns (change_id, commit_id) tuples for each head
    async fn get_stack_heads(&self) -> Result<Vec<(String, String)>>;

    /// Get all changes from revision back to (but not including) the main branch
    /// Returns them in order from tip to base as (change_id, commit_id) tuples
    async fn get_stack_changes(&self, revision: &str) -> Result<Vec<(String, String)>>;

    /// Get the commit ID of the trunk branch (main/master)
    async fn get_trunk_commit_id(&self) -> Result<String>;

    /// Check if `commit` is an ancestor of `descendant` using Jujutsu revsets
    async fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool>;
}

/// Represents a commit with its IDs and message
pub struct Commit {
    pub change_id: String,
    pub commit_id: String,
    pub message: CommitMessage,
    pub parent_change_ids: Vec<String>,
}

/// Represents a commit message with title and body
pub struct CommitMessage {
    pub title: Option<String>,
    pub body: Option<String>,
}

impl Commit {
    /// Reconstruct the full commit message from title and body
    pub fn full_message(&self) -> String {
        match (&self.message.title, &self.message.body) {
            (Some(title), Some(body)) => format!("{}\n\n{}", title, body),
            (Some(title), None) => title.clone(),
            (None, Some(body)) => body.clone(),
            (None, None) => String::new(),
        }
    }
}

// -----------------------------------------------------------------------------
// RealJujutsu

/// Real implementation that calls the jj CLI
pub struct RealJujutsu;

#[async_trait(?Send)]
impl JujutsuOps for RealJujutsu {
    async fn get_commit(&self, revision: &str) -> Result<Commit> {
        // Get commit_id, change_id, description, and parent change IDs in a single jj command
        let output = Command::new("jj")
            .args([
                "log",
                "-r",
                revision,
                "--no-graph",
                "-T",
                r#"commit_id ++ "|" ++ change_id ++ "|" ++ description ++ "|" ++ parents.map(|p| p.change_id()).join(",")"#,
            ])
            .output()
            .await
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let output_str = String::from_utf8(output.stdout)?.trim().to_string();
        let parts: Vec<&str> = output_str.splitn(4, '|').collect();

        if parts.len() != 4 {
            return Err(anyhow!(
                "Unexpected jj output format: expected 4 parts, got {}",
                parts.len()
            ));
        }

        let commit_id = parts[0].to_string();
        let change_id = parts[1].to_string();
        let description = parts[2].to_string();
        let parent_ids_str = parts[3];

        // Parse parent change IDs (comma-separated, may be empty)
        let parent_change_ids: Vec<String> = if parent_ids_str.is_empty() {
            vec![]
        } else {
            parent_ids_str.split(',').map(|s| s.to_string()).collect()
        };

        // Parse commit message into title and body
        let lines: Vec<&str> = description.lines().collect();
        let title = if lines.is_empty() {
            None
        } else {
            let first_line = lines[0].trim();
            if first_line.is_empty() {
                None
            } else {
                Some(first_line.to_string())
            }
        };

        let body = if lines.len() > 1 {
            let body_text = lines[1..].join("\n").trim().to_string();
            if body_text.is_empty() {
                None
            } else {
                Some(body_text)
            }
        } else {
            None
        };

        Ok(Commit {
            change_id,
            commit_id,
            message: CommitMessage { title, body },
            parent_change_ids,
        })
    }

    async fn get_stack_heads(&self) -> Result<Vec<(String, String)>> {
        // Find head commits in the current stack
        // These are commits descended from @ that aren't on trunk
        let heads_revset = "heads(descendants(@) ~ ancestors(trunk()))";
        let output = Command::new("jj")
            .args([
                "log",
                "-r",
                heads_revset,
                "--no-graph",
                "-T",
                r#"change_id ++ "|" ++ commit_id ++ "\n""#,
            ])
            .output()
            .await
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let heads: Vec<(String, String)> = String::from_utf8(output.stdout)?
            .lines()
            .filter(|s| !s.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() == 2 {
                    Some((parts[0].to_string(), parts[1].to_string()))
                } else {
                    None
                }
            })
            .collect();

        Ok(heads)
    }

    async fn get_stack_changes(&self, revision: &str) -> Result<Vec<(String, String)>> {
        // Get all ancestors of revision that are not ancestors of trunk (main/master)
        // trunk() is a jj built-in that automatically detects the main branch
        let stack_revset = format!("ancestors({}) ~ ancestors(trunk())", revision);
        let output = Command::new("jj")
            .args([
                "log",
                "-r",
                &stack_revset,
                "--no-graph",
                "-T",
                r#"change_id ++ "|" ++ commit_id ++ "\n""#,
                "--reversed",
            ])
            .output()
            .await
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let changes: Vec<(String, String)> = String::from_utf8(output.stdout)?
            .lines()
            .filter(|s| !s.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() == 2 {
                    Some((parts[0].to_string(), parts[1].to_string()))
                } else {
                    None
                }
            })
            .collect();

        // Reverse to get tip-to-base order (from most recent to oldest)
        Ok(changes.into_iter().rev().collect())
    }

    async fn get_trunk_commit_id(&self) -> Result<String> {
        let output = Command::new("jj")
            .args(["log", "-r", "trunk()", "--no-graph", "-T", "commit_id"])
            .output()
            .await
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    async fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool> {
        // Check if commit is in ancestors(descendant) using Jujutsu revsets
        let revset = format!("ancestors({}) & {}", descendant, commit);
        let output = Command::new("jj")
            .args(["log", "-r", &revset, "--no-graph", "-T", "commit_id"])
            .output()
            .await
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // If output is non-empty, commit is an ancestor of descendant
        Ok(!String::from_utf8(output.stdout)?.trim().is_empty())
    }
}
