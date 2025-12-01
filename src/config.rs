use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub github_branch_prefix: String,
}

impl Config {
    /// Load config from .git/config
    pub fn load() -> Result<Self> {
        let output = std::process::Command::new("git")
            .args(["config", "--get", "jr.githubBranchPrefix"])
            .output()?;

        if !output.status.success() {
            anyhow::bail!("Config not found in .git/config. Run 'jr init' to create one.");
        }

        let github_branch_prefix = String::from_utf8(output.stdout)?.trim().to_string();

        Ok(Self {
            github_branch_prefix,
        })
    }

    /// Save config to .git/config
    pub fn save(&self) -> Result<()> {
        let output = std::process::Command::new("git")
            .args([
                "config",
                "jr.githubBranchPrefix",
                &self.github_branch_prefix,
            ])
            .output()?;

        if !output.status.success() {
            anyhow::bail!("Failed to save config to .git/config");
        }

        Ok(())
    }

    /// Create a new config with explicit values (useful for tests)
    pub fn new(github_branch_prefix: String) -> Self {
        Self {
            github_branch_prefix,
        }
    }

    /// Default config for tests
    pub fn default_for_tests() -> Self {
        Self {
            github_branch_prefix: "test/".to_string(),
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
        let config = Config::new("custom/".to_string());
        assert_eq!(config.github_branch_prefix, "custom/");
    }

    #[test]
    fn test_default_github_branch_prefix() {
        let prefix = Config::default_github_branch_prefix();
        // Should be $USER/ or "dev/" if USER not set
        assert!(prefix.ends_with('/'));
    }
}
