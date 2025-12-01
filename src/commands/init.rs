use anyhow::Result;
use dialoguer::Input;

use crate::App;
use crate::config::Config;
use crate::ops::git::GitOps;
use crate::ops::github::GithubOps;
use crate::ops::jujutsu::JujutsuOps;

impl<J: JujutsuOps, G: GitOps, H: GithubOps> App<J, G, H> {
    pub async fn cmd_init(&self, stdout: &mut impl std::io::Write) -> Result<()> {
        // Try to load existing config or use defaults
        let current_config = Config::load()
            .unwrap_or_else(|_| Config::new(Config::default_github_branch_prefix(), String::new()));

        // Prompt for GitHub branch prefix with current value as default
        let github_branch_prefix: String = Input::new()
            .with_prompt("GitHub branch prefix")
            .default(current_config.github_branch_prefix)
            .interact_text()?;

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
        let github_token: String = Input::new()
            .with_prompt("GitHub Personal Access Token")
            .default(current_config.github_token)
            .interact_text()?;

        // Create new config with user's input
        let new_config = Config::new(github_branch_prefix, github_token);

        // Save the config
        new_config.save()?;

        writeln!(stdout, "Configuration saved to .git/config")?;

        Ok(())
    }
}
