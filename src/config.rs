use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub github_branch_prefix: String,
    pub github_token: String,
}

impl Config {
    /// Load config from .git/config
    pub fn load() -> Result<Self> {
        let prefix_output = std::process::Command::new("git")
            .args(["config", "--get", "jr.githubBranchPrefix"])
            .output()?;

        if !prefix_output.status.success() {
            anyhow::bail!("Config not found in .git/config. Run 'jr init' to create one.");
        }

        let token_output = std::process::Command::new("git")
            .args(["config", "--get", "jr.githubToken"])
            .output()?;

        if !token_output.status.success() {
            anyhow::bail!("GitHub token not found in .git/config. Run 'jr init' to configure.");
        }

        let github_branch_prefix = String::from_utf8(prefix_output.stdout)?.trim().to_string();
        let github_token = String::from_utf8(token_output.stdout)?.trim().to_string();

        Ok(Self {
            github_branch_prefix,
            github_token,
        })
    }

    /// Save config to .git/config
    pub fn save(&self) -> Result<()> {
        let prefix_output = std::process::Command::new("git")
            .args([
                "config",
                "jr.githubBranchPrefix",
                &self.github_branch_prefix,
            ])
            .output()?;

        if !prefix_output.status.success() {
            anyhow::bail!("Failed to save github_branch_prefix to .git/config");
        }

        let token_output = std::process::Command::new("git")
            .args(["config", "jr.githubToken", &self.github_token])
            .output()?;

        if !token_output.status.success() {
            anyhow::bail!("Failed to save github_token to .git/config");
        }

        Ok(())
    }

    /// Create a new config with explicit values (useful for tests)
    pub fn new(github_branch_prefix: String, github_token: String) -> Self {
        Self {
            github_branch_prefix,
            github_token,
        }
    }

    /// Default config for tests
    pub fn default_for_tests() -> Self {
        Self {
            github_branch_prefix: "test/".to_string(),
            github_token: "test_token".to_string(),
        }
    }

    /// Default GitHub branch prefix based on current user
    pub fn default_github_branch_prefix() -> String {
        std::env::var("USER").unwrap_or_else(|_| "dev".to_string()) + "/"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_for_tests() {
        let config = Config::default_for_tests();
        assert_eq!(config.github_branch_prefix, "test/");
    }

    #[test]
    fn test_new() {
        let config = Config::new("custom/".to_string(), "token123".to_string());
        assert_eq!(config.github_branch_prefix, "custom/");
        assert_eq!(config.github_token, "token123");
    }

    #[test]
    fn test_default_github_branch_prefix() {
        let prefix = Config::default_github_branch_prefix();
        // Should be $USER/ or "dev/" if USER not set
        assert!(prefix.ends_with('/'));
    }
}
