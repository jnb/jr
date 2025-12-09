use anyhow::Result;
use colored::Colorize;
use futures_util::future::try_join_all;
use log::warn;

use crate::App;
use crate::commit::CommitInfo;
use crate::commit::SyncStatus;

impl App {
    pub async fn cmd_status(&self, stdout: &mut impl std::io::Write) -> Result<()> {
        // Get stack commits
        let heads = self.jj.get_stack_heads("@").await?;
        let commits = if heads.is_empty() {
            // Current commit is on trunk
            vec![]
        } else if heads.len() == 1 {
            let head_commit_id = &heads[0].commit_id.0;
            self.jj.get_stack_ancestors(head_commit_id).await?
        } else {
            warn!("Warning: Multiple stack heads detected. Showing stack from rev to trunk.");
            self.jj.get_stack_ancestors("@").await?
        };

        // Build CommitInfo for each commit
        let commit_futures = commits
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

        let current_commit = self.jj.get_commit("@").await?;

        for (commit_info, status) in commit_infos.iter().zip(statuses.iter()) {
            let branch = &commit_info.pr_branch;
            let pr_url_result = self.gh.pr_url(branch).await;

            // Display status symbol + abbreviated change ID (cyan) + title (white) on first line
            let abbreviated_change_id = commit_info.short_id();
            let change_id_colored = abbreviated_change_id.cyan();
            let commit_title = commit_info.commit.message.title.as_deref().unwrap_or("");
            let is_current = commit_info.commit.change_id == current_commit.change_id;
            let commit_title = if is_current {
                commit_title.white().bold()
            } else {
                commit_title.white()
            };
            let out = format!("{} {} {}", status, change_id_colored, commit_title);
            writeln!(stdout, "{}", out.trim_end())?;

            // Display URL on second line if PR exists (dimmed to be less prominent)
            if let Ok(Some(pr_url)) = pr_url_result {
                let url_line = format!("  {}", pr_url);
                writeln!(stdout, "{}", url_line.dimmed())?;
            }
        }
        Ok(())
    }
}
