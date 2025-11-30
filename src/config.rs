use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub branch_prefix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub branch_prefix: String,
}

impl Config {
    /// Load config from ~/.jr.yaml with sensible defaults
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        let config_file = if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            serde_yml::from_str::<ConfigFile>(&contents)?
        } else {
            ConfigFile {
                branch_prefix: None,
            }
        };

        Ok(Self {
            branch_prefix: config_file
                .branch_prefix
                .unwrap_or_else(Self::default_branch_prefix),
        })
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

    /// Get the config file path (~/.jr.yaml)
    fn config_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
        Ok(PathBuf::from(home).join(".jr.yaml"))
    }

    /// Default branch prefix based on current user
    fn default_branch_prefix() -> String {
        std::env::var("USER")
            .unwrap_or_else(|_| "dev".to_string())
            + "/"
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
