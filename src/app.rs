use std::path;
use std::sync::Arc;

use anyhow::Result;
use anyhow::bail;
use futures_util::future::try_join_all;

use crate::clients::git::GitClient;
use crate::clients::github::GithubClient;
use crate::clients::jujutsu::JujutsuClient;
use crate::commit::CommitInfo;
use crate::commit::SyncStatus;
use crate::config::Config;

pub struct App {
    pub config: Arc<Config>,
    pub gh: Arc<GithubClient>,
    pub jj: Arc<JujutsuClient>,
    pub git: Arc<GitClient>,
}

impl App {
    pub fn new(config: Config, gh: GithubClient, path: path::PathBuf) -> Self {
        Self {
            config: Arc::new(config),
            gh: Arc::new(gh),
            jj: Arc::new(JujutsuClient::new(path.clone())),
            git: Arc::new(GitClient::new(path)),
        }
    }
}

/// Shared helper methods for App
impl App {
    /// Check if any parent PRs in the stack are outdated or need restacking.
    pub(crate) async fn check_parent_prs_up_to_date(&self, revision: &str) -> Result<()> {
        let commit = self.jj.get_commit(revision).await?;
        let stack_changes = self
            .jj
            .get_stack_ancestors_exclusive(&commit.commit_id.0)
            .await?;

        // Build CommitInfo for each commit
        let commit_futures = stack_changes
            .into_iter()
            .map(|commit| CommitInfo::new(commit, &self.config, &self.jj, &self.gh, &self.git));
        let commit_infos = try_join_all(commit_futures).await?;

        // Calculate sync statuses with propagation from parent to child
        // Iterate from parent to child (oldest to youngest)
        let commits_rev = commit_infos.iter().rev().collect::<Vec<_>>();
        let mut statuses: Vec<SyncStatus> = vec![];
        let mut restack = false;

        for commit_info in commits_rev.iter() {
            let status = commit_info.status();

            // If any ancestor needs restacking, all descendants need restacking
            match status {
                SyncStatus::Unknown | SyncStatus::Changed | SyncStatus::Restack => {
                    restack = true;
                    statuses.push(status);
                }
                SyncStatus::Synced => {
                    if restack {
                        statuses.push(SyncStatus::Restack);
                    } else {
                        statuses.push(SyncStatus::Synced);
                    }
                }
            }
        }

        // Reverse statuses to match original commit order (child to parent)
        statuses.reverse();

        // Check statuses in order from parent to child (oldest to youngest)
        for status in statuses.iter().rev() {
            match status {
                SyncStatus::Unknown => {
                    bail!("Parent commit has no PR branch. Create parent PR first (bottom-up).",);
                }
                SyncStatus::Restack => {
                    // bail!(
                    //     "Cannot update PR: parent PR {} needs restacking. Its base branch '{}' has been updated. Run 'jr restack' on the parent first.",
                    //     expected_branch,
                    //     base_branch
                    // );
                    bail!(
                        "Cannot update PR: parent PR needs restacking. Its base branch has been updated. Run 'jr restack' on the parent first.",
                    );
                }
                SyncStatus::Changed => {
                    // bail!(
                    //     "Cannot update PR: parent PR {} is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    //     expected_branch
                    // );
                    bail!(
                        "Cannot update PR: parent PR is out of date. Update parent PRs first (starting from the bottom of the stack).",
                    );
                }
                SyncStatus::Synced => {}
            }
        }

        Ok(())
    }
}
