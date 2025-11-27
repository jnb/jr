use std::process::Command;

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;

// -----------------------------------------------------------------------------
// JujutsuOps trait

/// Operations for interacting with Jujutsu version control
pub trait JujutsuOps {
    fn get_commit_id(&self, revision: &str) -> Result<String>;
    fn get_change_id(&self, revision: &str) -> Result<String>;
    fn get_commit_message(&self, revision: &str) -> Result<String>;

    /// Get the parent change IDs for a revision
    fn get_parent_change_ids(&self, revision: &str) -> Result<Vec<String>>;

    /// Get the head commits of the current stack (descendants of @ that aren't ancestors of trunk)
    /// Returns (change_id, commit_id) tuples for each head
    fn get_stack_heads(&self) -> Result<Vec<(String, String)>>;

    /// Get all changes from revision back to (but not including) the main branch
    /// Returns them in order from tip to base as (change_id, commit_id) tuples
    fn get_stack_changes(&self, revision: &str) -> Result<Vec<(String, String)>>;
}

// -----------------------------------------------------------------------------
// RealJujutsu

/// Real implementation that calls the jj CLI
pub struct RealJujutsu;

impl JujutsuOps for RealJujutsu {
    fn get_commit_id(&self, revision: &str) -> Result<String> {
        let output = Command::new("jj")
            .args(["log", "-r", revision, "--no-graph", "-T", "commit_id"])
            .output()
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn get_change_id(&self, revision: &str) -> Result<String> {
        let output = Command::new("jj")
            .args(["log", "-r", revision, "--no-graph", "-T", "change_id"])
            .output()
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn get_commit_message(&self, revision: &str) -> Result<String> {
        let output = Command::new("jj")
            .args(["log", "-r", revision, "--no-graph", "-T", "description"])
            .output()
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
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
    fn get_commit_id(&self, revision: &str) -> Result<String> {
        // First try to find in the map by change_id
        if let Some(commit_id) = self.change_to_commit.get(revision) {
            return Ok(commit_id.clone());
        }
        // Fall back to default
        Ok(self.commit_id.clone())
    }

    fn get_change_id(&self, _revision: &str) -> Result<String> {
        Ok(self.change_id.clone())
    }

    fn get_commit_message(&self, _revision: &str) -> Result<String> {
        Ok(self.commit_message.clone())
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
