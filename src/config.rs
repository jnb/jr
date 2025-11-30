use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub branch_prefix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub branch_prefix: String,
}

impl Config {
    /// Load config from <repo_root>/.jr.yaml
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            anyhow::bail!(
                "Config file not found at {}. Run 'jr init' to create one.",
                config_path.display()
            );
        }

        let contents = std::fs::read_to_string(&config_path)?;
        let config_file = serde_yml::from_str::<ConfigFile>(&contents)?;

        Ok(Self {
            branch_prefix: config_file
                .branch_prefix
                .unwrap_or_else(Self::default_branch_prefix),
        })
    }

    /// Save config to <repo_root>/.jr.yaml
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        let config_file = ConfigFile {
            branch_prefix: Some(self.branch_prefix.clone()),
        };

        let contents = serde_yml::to_string(&config_file)?;
        std::fs::write(&config_path, contents)?;

        Ok(())
    }

    /// Create a new config with explicit values (useful for tests)
    pub fn new(branch_prefix: String) -> Self {
        Self { branch_prefix }
    }

    /// Default config for tests
    pub fn default_for_tests() -> Self {
        Self {
            branch_prefix: "test/".to_string(),
        }
    }

    /// Get the config file path (<repo_root>/.jr.yaml)
    fn config_path() -> Result<PathBuf> {
        let repo_root = Self::repo_root()?;
        Ok(repo_root.join(".jr.yaml"))
    }

    /// Find the git repository root
    fn repo_root() -> Result<PathBuf> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()?;

        if !output.status.success() {
            anyhow::bail!("Not in a git repository");
        }

        let path = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(PathBuf::from(path))
    }

    /// Default branch prefix based on current user
    pub fn default_branch_prefix() -> String {
        std::env::var("USER").unwrap_or_else(|_| "dev".to_string()) + "/"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_for_tests() {
        let config = Config::default_for_tests();
        assert_eq!(config.branch_prefix, "test/");
    }

    #[test]
    fn test_new() {
        let config = Config::new("custom/".to_string());
        assert_eq!(config.branch_prefix, "custom/");
    }

    #[test]
    fn test_default_branch_prefix() {
        let prefix = Config::default_branch_prefix();
        // Should be $USER/ or "dev/" if USER not set
        assert!(prefix.ends_with('/'));
    }
}
