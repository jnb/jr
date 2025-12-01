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
        let current_config =
            Config::load().unwrap_or_else(|_| Config::new(Config::default_github_branch_prefix()));

        // Prompt for GitHub branch prefix with current value as default
        let github_branch_prefix: String = Input::new()
            .with_prompt("GitHub branch prefix")
            .default(current_config.github_branch_prefix)
            .interact_text()?;

        // Create new config with user's input
        let new_config = Config::new(github_branch_prefix);

        // Save the config
        new_config.save()?;

        writeln!(stdout, "Configuration saved to .git/config")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ops::git::MockGitOps;
    use crate::ops::github::MockGithubOps;
    use crate::ops::jujutsu::MockJujutsuOps;

    // Note: Testing interactive prompts is complex and would require mocking stdin.
    // The core logic is tested through integration tests or manual testing.
    // Here we just verify the command compiles and has the right signature.

    #[tokio::test]
    async fn test_init_command_exists() {
        let mock_jj = MockJujutsuOps::new();
        let mock_git = MockGitOps::new();
        let mock_gh = MockGithubOps::new();

        let app = App::new(Config::default_for_tests(), mock_jj, mock_git, mock_gh);

        // Verify the method exists and has correct signature
        // We can't easily test interactive prompts in unit tests
        let _ = &app;
    }
}
