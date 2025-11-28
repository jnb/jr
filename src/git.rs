use std::process::Command;

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;

// -----------------------------------------------------------------------------
// GitOps trait

/// Operations for interacting with Git
pub trait GitOps {
    fn get_tree(&self, commit_id: &str) -> Result<String>;
    fn get_branch(&self, branch: &str) -> Result<String>;
    fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<String>;
    fn commit_tree_merge(&self, tree: &str, parents: &[&str], message: &str) -> Result<String>;
    fn update_branch(&self, branch: &str, commit: &str) -> Result<()>;
    fn push_branch(&self, branch: &str) -> Result<()>;

    /// Check if `commit` is an ancestor of `descendant`.
    /// Returns true if `commit` is reachable from `descendant` by following parent links.
    /// In other words, returns true if `descendant` contains all changes from `commit`.
    fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool>;

    /// Get a canonical representation of the changes introduced by a commit.
    /// Returns a string representing the diff (file names and status) that can be compared.
    fn get_commit_diff(&self, commit_id: &str) -> Result<String>;
}

// -----------------------------------------------------------------------------
// RealGithub

/// Real implementation that calls the git CLI
pub struct RealGit;

impl GitOps for RealGit {
    fn get_tree(&self, commit_id: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", &format!("{}^{{tree}}", commit_id)])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn get_branch(&self, branch: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", &format!("origin/{}", branch)])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["commit-tree", tree, "-p", parent, "-m", message])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn commit_tree_merge(&self, tree: &str, parents: &[&str], message: &str) -> Result<String> {
        let mut args = vec!["commit-tree", tree];
        for parent in parents {
            args.push("-p");
            args.push(parent);
        }
        args.push("-m");
        args.push(message);

        let output = Command::new("git")
            .args(&args)
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn update_branch(&self, branch: &str, commit: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["update-ref", &format!("refs/heads/{}", branch), commit])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    fn push_branch(&self, branch: &str) -> Result<()> {
        let refspec = format!("refs/heads/{}:refs/heads/{}", branch, branch);
        let output = Command::new("git")
            .args(["push", "-u", "origin", &refspec])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool> {
        let output = Command::new("git")
            .args(["merge-base", "--is-ancestor", commit, descendant])
            .output()
            .context("Failed to execute git command")?;

        // Exit code 0 means it is an ancestor, 1 means it's not
        Ok(output.status.success())
    }

    fn get_commit_diff(&self, commit_id: &str) -> Result<String> {
        // Use diff-tree to get the full textual diff introduced by this commit
        // -p: generate patch (full diff with +/- lines)
        // --no-commit-id: don't show the commit ID in output
        let output = Command::new("git")
            .args(["diff-tree", "-p", "--no-commit-id", commit_id])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}

// -----------------------------------------------------------------------------
// MockGithub

pub struct MockGit {
    pub tree: String,
    pub trees: std::collections::HashMap<String, String>,
    pub branches: std::collections::HashMap<String, String>,
    pub commits: Vec<String>,
    pub updated_branches: std::cell::RefCell<Vec<(String, String)>>,
    pub pushed_branches: std::cell::RefCell<Vec<String>>,
    pub ancestors: std::collections::HashMap<String, Vec<String>>,
    pub diffs: std::collections::HashMap<String, String>,
}

impl MockGit {
    pub fn new() -> Self {
        Self {
            tree: "tree123".to_string(),
            trees: std::collections::HashMap::new(),
            branches: std::collections::HashMap::new(),
            commits: vec!["commit1".to_string(), "commit2".to_string()],
            updated_branches: std::cell::RefCell::new(Vec::new()),
            pushed_branches: std::cell::RefCell::new(Vec::new()),
            ancestors: std::collections::HashMap::new(),
            diffs: std::collections::HashMap::new(),
        }
    }

    pub fn with_branch(mut self, branch: String, commit: String) -> Self {
        self.branches.insert(branch, commit);
        self
    }

    pub fn with_tree(mut self, commit: String, tree: String) -> Self {
        self.trees.insert(commit, tree);
        self
    }

    pub fn with_ancestor(mut self, commit: String, descendant: String) -> Self {
        self.ancestors
            .entry(descendant)
            .or_insert_with(Vec::new)
            .push(commit);
        self
    }

    pub fn with_diff(mut self, commit: String, diff: String) -> Self {
        self.diffs.insert(commit, diff);
        self
    }
}

impl GitOps for MockGit {
    fn get_tree(&self, commit_id: &str) -> Result<String> {
        Ok(self
            .trees
            .get(commit_id)
            .cloned()
            .unwrap_or_else(|| self.tree.clone()))
    }

    fn get_branch(&self, branch: &str) -> Result<String> {
        self.branches
            .get(branch)
            .cloned()
            .ok_or_else(|| anyhow!("Branch not found: {}", branch))
    }

    fn commit_tree(&self, _tree: &str, _parent: &str, _message: &str) -> Result<String> {
        Ok(self.commits.get(0).unwrap().clone())
    }

    fn commit_tree_merge(&self, _tree: &str, _parents: &[&str], _message: &str) -> Result<String> {
        Ok(self.commits.get(0).unwrap().clone())
    }

    fn update_branch(&self, branch: &str, commit: &str) -> Result<()> {
        self.updated_branches
            .borrow_mut()
            .push((branch.to_string(), commit.to_string()));
        Ok(())
    }

    fn push_branch(&self, branch: &str) -> Result<()> {
        self.pushed_branches.borrow_mut().push(branch.to_string());
        Ok(())
    }

    fn is_ancestor(&self, commit: &str, descendant: &str) -> Result<bool> {
        Ok(self
            .ancestors
            .get(descendant)
            .map(|ancestors| ancestors.contains(&commit.to_string()))
            .unwrap_or(false))
    }

    fn get_commit_diff(&self, commit_id: &str) -> Result<String> {
        self.diffs
            .get(commit_id)
            .cloned()
            .ok_or_else(|| anyhow!("Diff not found for commit: {}", commit_id))
    }
}
