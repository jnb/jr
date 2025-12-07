//! Operations modules for interacting with external version control systems.
//!
//! This module contains the integration layers for the three systems that `jr` coordinates:
//!
//! - [`git`]: Low-level Git operations (tree parsing, commit creation, branch updates, pushing)
//! - [`github`]: GitHub PR management via GitHub CLI
//! - [`github_curl`]: Curl-based HTTP client for making GitHub API requests
//! - [`jujutsu`]: Jujutsu operations for extracting commit and change IDs
//!
//! Each submodule provides trait-based abstractions with real and mock implementations
//! to support both production use and testing.

pub mod git;
pub mod github;
pub mod github_curl;
pub mod jujutsu;
