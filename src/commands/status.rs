use anyhow::Result;
use colored::Colorize;
use log::warn;

use crate::App;
use crate::stack::Stack;

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

        let stack = Stack::new(
            self.config.clone(),
            self.jj.clone(),
            self.gh.clone(),
            self.git.clone(),
            commits,
        )
        .await?;

        let current_commit = self.jj.get_commit("@").await?;

        for (commit, status) in stack
            .commits
            .iter()
            .zip(stack.sync_statuses().await?.iter())
        {
            let branch = commit
                .change_id
                .branch_name(&self.config.github_branch_prefix);
            let pr_url_result = self.gh.pr_url(&branch).await;

            // Display status symbol + abbreviated change ID (cyan) + title (white) on first line
            let abbreviated_change_id = &commit.change_id.short_id();
            let change_id_colored = abbreviated_change_id.cyan();
            let commit_title = commit.message.title.as_deref().unwrap_or("");
            let is_current = commit.change_id == current_commit.change_id;
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
