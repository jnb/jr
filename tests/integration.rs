//! Integration tests; run as
//!
//!   cargo test --test integration --include-ignored -- --nocapture
//!
//! Prefix with DEBUG_TESTS=1 to keep local repos around.

mod macros;
mod utils;

use std::sync::LazyLock;

use jr::github::GithubOps as _;
use jr::github::RealGithub;
use serial_test::serial;
use tracing::instrument;

const GITHUB_USER: &str = "jnb";
const GITHUB_REPO: &str = "test_repo";

const GIT_BRANCH_PREFIX: &str = "jnb/";

/// Normalize IDs for snapshot comparisons.
static INSTA_FILTERS: LazyLock<Vec<(&'static str, &'static str)>> = LazyLock::new(|| {
    vec![
        // Change ID
        (r"(\s)[k-z]{32}(\s)", "$1[CHGID]$2"),
        // Abbreviated change ID
        (r"(\s)[k-z]{4}(\s)", "$1[CHGID]$2"),
        // Git object ID
        (r"(\s)[0-9a-f]{40}(\s)", "$1[OBJID]$2"),
        // Branch
        (
            Box::leak(format!("{}[k-z]{{8}}", GIT_BRANCH_PREFIX).into_boxed_str()),
            "[BRANCH]",
        ),
        // Pull request ID
        (
            Box::leak(
                format!(
                    r"(https://github.com)/{}/{}/pull/\d+",
                    GITHUB_USER, GITHUB_REPO
                )
                .into_boxed_str(),
            ),
            "$1/[USER]/[REPO]/[PRID]",
        ),
    ]
});

#[ctor::ctor]
fn init() {
    // Disable colors for all integration tests to get clean output
    colored::control::set_override(false);
    utils::setup_logging().unwrap();
}

// TODO Cleanup
#[instrument(skip_all)]
async fn setup(temp_path: &std::path::Path) -> anyhow::Result<()> {
    utils::create_git_repo(temp_path).await?;
    utils::setup_git_remote(temp_path, "git@github.com:jnb/test_repo.git").await?;
    utils::init_jujutsu(temp_path).await?;
    utils::jj_git_fetch(temp_path).await?;
    utils::track_branch(temp_path, "master", "origin").await?;

    std::env::set_current_dir(temp_path)?;

    // Find all branches and delete them
    let github = RealGithub::new("jnb/".to_string());
    let branches = github.find_branches_with_prefix("").await?;
    println!("Found {} branches to delete", branches.len());
    for branch in branches {
        println!("Deleting branch: {}", branch);
        github.delete_branch(&branch).await?;
    }

    // Update git repo again because we deleted remote branches
    utils::jj_git_fetch(&temp_path).await?;

    utils::jj_new(&temp_path, "master").await?;

    utils::create_jj_commit(temp_path, "Alpha", "alpha", "alpha\n").await?;
    utils::create_jj_commit(temp_path, "Beta", "beta", "beta\n").await?;
    utils::create_jj_commit(temp_path, "Gamma", "gamma", "gamma\n").await?;

    let log_output = utils::jj_log(temp_path).await?;
    insta::assert_snapshot!(log_output, @r"
    @  (no description)
    ○  Gamma
    ○  Beta
    ○  Alpha
    ◆  bar
    │
    ~
    ");

    Ok(())
}

/// Test the happy-path workflow for stacked PRs with our tree of three commits
/// (see `setup`):
///
/// - Initially all commits show `?` (no PRs exist)
/// - Create PRs for Alpha, Beta, and Gamma (all show ✓)
/// - Edit Alpha (Alpha shows ✗, Beta and Gamma show ↻)
/// - Update Alpha's PR (Alpha shows ✓, Beta and Gamma still show ↻)
/// - Restack Beta without a message (Beta shows ✓, Gamma still shows ↻)
/// - Restack Gamma without a message (all show ✓)
///
/// This validates that status symbols correctly propagate through the stack and
/// that auto-restack detection works for commits that haven't been modified.
#[tokio::test]
#[ignore]
#[serial]
async fn test_stacked_workflow() -> anyhow::Result<()> {
    let test_dir = utils::TestDir::new()?;
    insta::assert_snapshot!("", @""); // Display insta code lense

    setup(test_dir.path()).await?;

    let app = jr::App::new(
        jr::jujutsu::RealJujutsu,
        jr::git::RealGit,
        jr::github::RealGithub::new("jnb/".to_string()), // FIXME
    );

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ? [CHGID]
        ? [CHGID] Gamma
        ? [CHGID] Beta
        ? [CHGID] Alpha
    ");

    // -------------------------------------------------------------------------
    // Create PR for Alpha

    let (out, _) = run_and_capture!(|out, _| app.cmd_create("description(Alpha)", out));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        Change ID: [CHGID]
        Commit ID: [OBJID]
        PR branch: [BRANCH]
        Base branch: master
        Tree: [OBJID]
        Created new commit: [OBJID]
        Updated PR branch [BRANCH]
        Pushed PR branch [BRANCH]
        Created PR for [BRANCH] with base master
        PR URL: https://github.com/[USER]/[REPO]/[PRID]
    ");

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ? [CHGID]
        ? [CHGID] Gamma
        ? [CHGID] Beta
        ✓ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    // -------------------------------------------------------------------------
    // Create PR for Beta

    let (out, _) = run_and_capture!(|out, _| app.cmd_create("description(Beta)", out));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        Change ID: [CHGID]
        Commit ID: [OBJID]
        PR branch: [BRANCH]
        Base branch: [BRANCH]
        Tree: [OBJID]
        Created new commit: [OBJID]
        Updated PR branch [BRANCH]
        Pushed PR branch [BRANCH]
        Created PR for [BRANCH] with base [BRANCH]
        PR URL: https://github.com/[USER]/[REPO]/[PRID]
    ");

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ? [CHGID]
        ? [CHGID] Gamma
        ✓ [CHGID] Beta
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    // -------------------------------------------------------------------------
    // Create PR for Gamma

    let (out, _) = run_and_capture!(|out, _| app.cmd_create("description(Gamma)", out));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        Change ID: [CHGID]
        Commit ID: [OBJID]
        PR branch: [BRANCH]
        Base branch: [BRANCH]
        Tree: [OBJID]
        Created new commit: [OBJID]
        Updated PR branch [BRANCH]
        Pushed PR branch [BRANCH]
        Created PR for [BRANCH] with base [BRANCH]
        PR URL: https://github.com/[USER]/[REPO]/[PRID]
    ");

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ? [CHGID]
        ✓ [CHGID] Gamma
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Beta
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    // -------------------------------------------------------------------------
    // Edit Alpha

    utils::jj_edit(test_dir.path(), "description(Alpha) & mine()").await?;
    tokio::fs::write(test_dir.path().join("alpha"), "alpha1\n").await?;

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ↻ [CHGID] Gamma
          https://github.com/[USER]/[REPO]/[PRID]
        ↻ [CHGID] Beta
          https://github.com/[USER]/[REPO]/[PRID]
        ✗ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    // -------------------------------------------------------------------------
    // Update Alpha

    let (out, _) = run_and_capture!(|out, _| app.cmd_update(
        "description(Alpha) & mine()",
        "Update alpha",
        out
    ));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
    Change ID: [CHGID]
    Commit ID: [OBJID]
    PR branch: [BRANCH]
    Base branch: master
    Tree: [OBJID]
    PR branch [BRANCH] exists
    Created new commit: [OBJID]
    Updated PR branch [BRANCH]
    Pushed PR branch [BRANCH]
    Updated PR for [BRANCH] with base master
    PR URL: https://github.com/[USER]/[REPO]/[PRID]
    ");

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ↻ [CHGID] Gamma
          https://github.com/[USER]/[REPO]/[PRID]
        ↻ [CHGID] Beta
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    // -------------------------------------------------------------------------
    // Restack Beta

    let (out, _) = run_and_capture!(|out, _| app.cmd_restack("description(Beta) & mine()", out));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        Change ID: [CHGID]
        Commit ID: [OBJID]
        PR branch: [BRANCH]
        Base branch: [BRANCH]
        Tree: [OBJID]
        PR branch [BRANCH] exists
        Detected pure restack (no changes to this commit)
        Created new merge commit: [OBJID]
        Updated PR branch [BRANCH]
        Pushed PR branch [BRANCH]
        Updated PR for [BRANCH] with base [BRANCH]
        PR URL: https://github.com/[USER]/[REPO]/[PRID]
    ");

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ↻ [CHGID] Gamma
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Beta
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    // -------------------------------------------------------------------------
    // Restack Gamma

    let (out, _) = run_and_capture!(|out, _| app.cmd_restack("description(Gamma) & mine()", out));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        Change ID: [CHGID]
        Commit ID: [OBJID]
        PR branch: [BRANCH]
        Base branch: [BRANCH]
        Tree: [OBJID]
        PR branch [BRANCH] exists
        Detected pure restack (no changes to this commit)
        Created new merge commit: [OBJID]
        Updated PR branch [BRANCH]
        Pushed PR branch [BRANCH]
        Updated PR for [BRANCH] with base [BRANCH]
        PR URL: https://github.com/[USER]/[REPO]/[PRID]
    ");

    let (out, _) = run_and_capture!(|out, err| app.cmd_status(out, err));
    assert_snapshot_filtered!(out, INSTA_FILTERS, @r"
        ✓ [CHGID] Gamma
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Beta
          https://github.com/[USER]/[REPO]/[PRID]
        ✓ [CHGID] Alpha
          https://github.com/[USER]/[REPO]/[PRID]
    ");

    Ok(())
}
