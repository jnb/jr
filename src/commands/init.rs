use std::io;
use std::io::Write;

use anyhow::Result;

use crate::App;
use crate::config::Config;

impl App {
    #[rustfmt::skip]
    pub async fn cmd_init(&self, stdout: &mut impl std::io::Write) -> Result<()> {
        // Query the default branch from git
        let detected_default_branch = self.git.get_default_branch().await
            .unwrap_or_else(|_| "main".to_string());

        let current_config = Config::load()
            .unwrap_or_else(|_| Config::new(
                Config::default_github_branch_prefix(),
                String::new(),
                detected_default_branch.clone(),
            ));

        let github_branch_prefix =
            prompt_with_default("GitHub branch prefix", current_config.github_branch_prefix)?;

        let default_branch =
            prompt_with_default("Default branch", current_config.default_branch)?;

        writeln!(stdout)?;
        writeln!(stdout, "Either:")?;
        writeln!(stdout)?;
        writeln!(stdout, " - Create a fine-grained Personal Access Token for this repository at:")?;
        writeln!(stdout, "   https://github.com/settings/personal-access-tokens/new")?;
        writeln!(stdout)?;
        writeln!(stdout, "   Required permissions:")?;
        writeln!(stdout, "    - Contents: Read and write")?;
        writeln!(stdout, "    - Pull requests: Read and write")?;
        writeln!(stdout)?;
        writeln!(stdout, " - Or, create a classic Personal Access Token at:")?;
        writeln!(stdout, "   https://github.com/settings/tokens/new")?;
        writeln!(stdout)?;
        writeln!(stdout, "   Required scopes:")?;
        writeln!(stdout, "    - Repo")?;
        writeln!(stdout)?;

        let github_token =
            prompt_with_default("GitHub Personal Access Token", current_config.github_token)?;

        Config::new(github_branch_prefix, github_token, default_branch).save()?;

        writeln!(stdout, "Configuration saved to .git/config")?;

        Ok(())
    }
}

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
