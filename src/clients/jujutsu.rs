use std::path;

use anyhow::Context;
use anyhow::bail;
use tokio::process::Command;

use super::git;

// -----------------------------------------------------------------------------
// Types

/// Jujutsu client.
///
/// This is solely used for retrieving commits.  All other operations should be
/// delegated to the Git client.
pub struct JujutsuClient {
    path: path::PathBuf,
}

/// A Jujutsu commit.
#[derive(Clone)]
pub struct JujutsuCommit {
    pub change_id: String,
    pub commit_id: git::CommitId,
    pub message: JujutsuCommitMessage,
    pub parent_change_ids: Vec<String>,
}

/// A Jujutsu commit message with title and body.
#[derive(Clone)]
pub struct JujutsuCommitMessage {
    pub title: Option<String>,
    pub body: Option<String>,
}

// -----------------------------------------------------------------------------
// JujutsuClient impl

impl JujutsuClient {
    pub fn new(path: path::PathBuf) -> Self {
        Self { path }
    }

    /// Get the head commit(s) of a stack.
    pub async fn get_stack_heads(&self, revset: &str) -> anyhow::Result<Vec<JujutsuCommit>> {
        self.get_commits(&format!(
            "heads(descendants({revset}) ~ ancestors(trunk()))"
        ))
        .await
    }

    /// Get all ancestors commits in a stack.
    pub async fn get_stack_ancestors(&self, revset: &str) -> anyhow::Result<Vec<JujutsuCommit>> {
        self.get_commits(&format!("ancestors({revset}) ~ ancestors(trunk())"))
            .await
    }

    /// Get the trunk commit.
    pub async fn get_trunk(&self) -> anyhow::Result<JujutsuCommit> {
        self.get_commit("trunk()").await
    }

    /// Get the single commit matching a revset.
    pub async fn get_commit(&self, revset: &str) -> anyhow::Result<JujutsuCommit> {
        let mut commits = self.get_commits(revset).await?;

        if commits.is_empty() {
            bail!("No commits found matching revset: {}", revset);
        }

        if commits.len() > 1 {
            bail!(
                "Expected exactly one commit for revset '{}', but found {}",
                revset,
                commits.len()
            );
        }

        Ok(commits.remove(0))
    }

    /// Get all commits matching a revset.
    async fn get_commits(&self, revset: &str) -> anyhow::Result<Vec<JujutsuCommit>> {
        // Get commit_id, change_id, description, and parent change IDs in a single jj command
        // Use \x00 as record separator to handle multi-line descriptions
        let output = Command::new("jj").current_dir(&self.path)
            .args([
                "log",
                "-r",
                revset,
                "--no-graph",
                "-T",
                r#"commit_id ++ "|" ++ change_id ++ "|" ++ description ++ "|" ++ parents.map(|p| p.change_id()).join(",") ++ "\x00""#,
            ])
            .output()
            .await
            .context("Failed to execute jj command")?;

        if !output.status.success() {
            bail!(
                "jj command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let output_str = String::from_utf8(output.stdout)?;
        let mut commits = Vec::new();

        // Parse each record (separated by null bytes) as a separate commit
        for record in output_str.split('\x00') {
            let record = record.trim();
            if record.is_empty() {
                continue;
            }

            let parts: Vec<&str> = record.splitn(4, '|').collect();

            if parts.len() != 4 {
                bail!(
                    "Unexpected jj output format for revset {revset}: expected 4 parts, got {}: {record}, {parts:?}",
                    parts.len(),
                );
            }

            let commit_id = git::CommitId(parts[0].to_string());
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

            commits.push(JujutsuCommit {
                change_id,
                commit_id,
                message: JujutsuCommitMessage { title, body },
                parent_change_ids,
            });
        }

        Ok(commits)
    }
}

// -----------------------------------------------------------------------------
// JujutuCommit impl

impl JujutsuCommit {
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
