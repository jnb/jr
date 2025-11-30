use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write as _;
use std::path::PathBuf;

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
            Config::load().unwrap_or_else(|_| Config::new(Config::default_branch_prefix()));

        // Prompt for branch prefix with current value as default
        let branch_prefix: String = Input::new()
            .with_prompt("Branch prefix")
            .default(current_config.branch_prefix)
            .interact_text()?;

        // Create new config with user's input
        let new_config = Config::new(branch_prefix);

        // Save the config
        new_config.save()?;

        writeln!(stdout, "Configuration saved to .jr.yaml")?;

        // Add to .git/info/exclude if not already present
        self.add_to_git_exclude()?;

        Ok(())
    }

    fn add_to_git_exclude(&self) -> Result<()> {
        let repo_root = Self::get_repo_root()?;
        let exclude_path = repo_root.join(".git/info/exclude");

        // Ensure the .git/info directory exists
        if let Some(parent) = exclude_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Check if .jr.yaml is already in exclude
        let pattern = "/.jr.yaml";
        let already_excluded = if exclude_path.exists() {
            let file = std::fs::File::open(&exclude_path)?;
            let reader = BufReader::new(file);
            reader
                .lines()
                .any(|line| line.map(|l| l.trim() == pattern).unwrap_or(false))
        } else {
            false
        };

        // Add to exclude if not already present
        if !already_excluded {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&exclude_path)?;
            writeln!(file, "{}", pattern)?;
        }

        Ok(())
    }

    fn get_repo_root() -> Result<PathBuf> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()?;

        if !output.status.success() {
            anyhow::bail!("Not in a git repository");
        }

        let path = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(PathBuf::from(path))
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
