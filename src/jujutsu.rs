use std::process::Command;

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;

// -----------------------------------------------------------------------------
// Types

/// Operations for interacting with Jujutsu version control
pub trait JujutsuOps {
    /// Get complete commit information for a revision
    fn get_commit(&self, revision: &str) -> Result<Commit>;

    /// Get the parent change IDs for a revision
    fn get_parent_change_ids(&self, revision: &str) -> Result<Vec<String>>;

    /// Get the head commits of the current stack (descendants of @ that aren't ancestors of trunk)
    /// Returns (change_id, commit_id) tuples for each head
    fn get_stack_heads(&self) -> Result<Vec<(String, String)>>;

    /// Get all changes from revision back to (but not including) the main branch
    /// Returns them in order from tip to base as (change_id, commit_id) tuples
    fn get_stack_changes(&self, revision: &str) -> Result<Vec<(String, String)>>;
}

/// Represents a commit with its IDs and message
pub struct Commit {
    pub change_id: String,
    pub commit_id: String,
    pub message: CommitMessage,
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

impl JujutsuOps for RealJujutsu {
    fn get_commit(&self, revision: &str) -> Result<Commit> {
        // Get commit_id, change_id, and description in a single jj command
        let output = Command::new("jj")
            .args([
                "log",
                "-r",
                revision,
                "--no-graph",
                "-T",
                r#"commit_id ++ "|" ++ change_id ++ "|" ++ description"#,
            ])
            .output()
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let output_str = String::from_utf8(output.stdout)?.trim().to_string();
        let parts: Vec<&str> = output_str.splitn(3, '|').collect();

        if parts.len() != 3 {
            return Err(anyhow!(
                "Unexpected jj output format: expected 3 parts, got {}",
                parts.len()
            ));
        }

        let commit_id = parts[0].to_string();
        let change_id = parts[1].to_string();
        let description = parts[2].to_string();

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
        })
    }

    fn get_parent_change_ids(&self, revision: &str) -> Result<Vec<String>> {
        let parent_revset = format!("parents({})", revision);
        let output = Command::new("jj")
            .args(["log", "-r", &parent_revset, "--no-graph", "-T", "change_id"])
            .output()
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let parent_ids: Vec<String> = String::from_utf8(output.stdout)?
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(parent_ids)
    }

    fn get_stack_heads(&self) -> Result<Vec<(String, String)>> {
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

    fn get_stack_changes(&self, revision: &str) -> Result<Vec<(String, String)>> {
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
}

// -----------------------------------------------------------------------------
// MockJujutsu

pub struct MockJujutsu {
    pub change_id: String,
    pub commit_id: String,
    pub commit_message: String,
    pub parent_change_ids: Vec<String>,
    pub stack_heads: Vec<(String, String)>,
    pub stack_changes: Vec<(String, String)>,
    pub change_to_commit: std::collections::HashMap<String, String>,
    pub change_to_parents: std::collections::HashMap<String, Vec<String>>,
}

impl JujutsuOps for MockJujutsu {
    fn get_commit(&self, revision: &str) -> Result<Commit> {
        // Get commit_id from map or default
        let commit_id = if let Some(commit_id) = self.change_to_commit.get(revision) {
            commit_id.clone()
        } else {
            self.commit_id.clone()
        };

        // Parse commit message into title and body
        let lines: Vec<&str> = self.commit_message.lines().collect();
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
            change_id: self.change_id.clone(),
            commit_id,
            message: CommitMessage { title, body },
        })
    }

    fn get_parent_change_ids(&self, revision: &str) -> Result<Vec<String>> {
        // First try to find in the map by change_id
        if let Some(parents) = self.change_to_parents.get(revision) {
            return Ok(parents.clone());
        }
        // Fall back to default
        Ok(self.parent_change_ids.clone())
    }

    fn get_stack_heads(&self) -> Result<Vec<(String, String)>> {
        Ok(self.stack_heads.clone())
    }

    fn get_stack_changes(&self, _revision: &str) -> Result<Vec<(String, String)>> {
        Ok(self.stack_changes.clone())
    }
}
