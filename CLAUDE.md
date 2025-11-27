# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Project Overview

`jr` (Jujutsu Review) is a Rust-based tool for managing Git branches and GitHub
PRs in a stacked workflow, designed to integrate with Jujutsu (jj) version control.
The tool automates the process of creating branches with a specific naming convention
and managing PRs with proper base branch relationships.

## Key Concepts

- **PR Branch Naming**: All PR branches use the global prefix `jnb/` defined in
  `GLOBAL_BRANCH_PREFIX`, followed by the first 8 characters of the jj change ID
  (e.g., `jnb/rzqmwvsz`)
- **Stacked PRs**: The tool creates and manages chains of PRs where each PR's
  base is the previous PR branch in the stack
- **Integration Points**: The codebase interacts with three systems:
  - `jj` (Jujutsu) for commit/change IDs
  - `git` for low-level tree/commit operations
  - `gh` (GitHub CLI) for PR management

## Architecture

The codebase is structured around three integration layers in `src/main.rs`:

1. **Jujutsu functions** (`jj_*`): Extract commit and change IDs from the
   current revision (@)
2. **Git functions** (`git_*`): Handle low-level Git operations (tree parsing,
   commit creation, branch updates, pushing)
3. **GitHub functions** (`gh_*`): Manage PR lifecycle (search branches,
   view/create/edit PRs)

The main workflow follows this pattern:
- Validate current commit state
- Get tree object from current commit
- Create new commit tree with appropriate parent
- Update and push PR branch
- Create or update PR with correct base branch

## Development Commands

```bash
# Build the project
cargo build

# Build and run
cargo run

# Build in release mode
cargo build --release

# Check code without building
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy
```

## Project State

The project is functional and includes:
- Full implementation of `create`, `update`, and `status` commands
  - `create`: Creates a new PR using the jj commit message
  - `update -m "message"`: Updates an existing PR with a custom commit message
  - `status`: Shows the status of stacked PRs
- Trait-based architecture with real and mock implementations for testing
- Validation to prevent PRs for commits already merged to main
- Support for both creating new PRs and updating existing ones
- Automatic detection of closed PRs to create new ones instead of editing
- Comprehensive test coverage
