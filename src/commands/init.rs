use std::io::Write;
use std::io::{self};

use anyhow::Result;

use crate::App;
use crate::config::Config;
use crate::ops::git::GitOps;
use crate::ops::github::GithubOps;
use crate::ops::jujutsu::JujutsuOps;

fn prompt_with_default(prompt: &str, default: String) -> Result<String> {
    print!("{} [{}]: ", prompt, default);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();

    Ok(if trimmed.is_empty() {
        default
    } else {
        trimmed.to_string()
    })
}

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub async fn cmd_init(&self, stdout: &mut impl std::io::Write) -> Result<()> {
        // Try to load existing config or use defaults
        let current_config = Config::load()
            .unwrap_or_else(|_| Config::new(Config::default_github_branch_prefix(), String::new()));

        // Prompt for GitHub branch prefix with current value as default
        let github_branch_prefix =
            prompt_with_default("GitHub branch prefix", current_config.github_branch_prefix)?;

        // Show instructions for creating a GitHub token
        writeln!(stdout)?;
        writeln!(
            stdout,
            "Create a fine-grained personal access token for this repository at:"
        )?;
        writeln!(
            stdout,
            "https://github.com/settings/personal-access-tokens/new"
        )?;
        writeln!(stdout)?;
        writeln!(stdout, "Required permissions:")?;
        writeln!(stdout, "  - Contents: Read and write")?;
        writeln!(stdout, "  - Pull requests: Read and write")?;
        writeln!(stdout)?;

        // Prompt for GitHub token
        let github_token =
            prompt_with_default("GitHub Personal Access Token", current_config.github_token)?;

        // Create new config with user's input
        let new_config = Config::new(github_branch_prefix, github_token);

        // Save the config
        new_config.save()?;

        writeln!(stdout, "Configuration saved to .git/config")?;

        Ok(())
    }
}
