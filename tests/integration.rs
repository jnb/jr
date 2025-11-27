mod utils;

use jr::github::GithubOps as _;
use jr::github::RealGithub;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn hello() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path().to_path_buf();

    utils::create_git_repo(&temp_path).await?;
    utils::setup_git_remote(&temp_path, "git@github.com:jnb/test_repo.git").await?;
    utils::init_jujutsu(&temp_path).await?;
    utils::jj_git_fetch(&temp_path).await?;
    utils::track_branch(&temp_path, "master", "origin").await?;

    std::env::set_current_dir(&temp_path)?;

    // Create a new change on top of master
    utils::jj_new(&temp_path, "master").await?;

    // Create three commits
    utils::create_jj_commit(&temp_path, "Alpha", "alpha", "alpha\n").await?;
    utils::create_jj_commit(&temp_path, "Beta", "beta", "beta\n").await?;
    utils::create_jj_commit(&temp_path, "Gamma", "gamma", "gamma\n").await?;

    // Snapshot the jj log output
    let log_output = utils::jj_log(&temp_path).await?;
    insta::assert_snapshot!(log_output, @r"
    @  (no description)
    ○  Gamma
    ○  Beta
    ○  Alpha
    ◆  bar
    │
    ~
    ");

    // TODO Now snapshot the jr output

    let github = RealGithub::new("jnb/".to_string());

    // Find all branches and delete them
    let branches = github.find_branches_with_prefix("").await?;
    println!("Found {} branches to delete", branches.len());

    for branch in branches {
        println!("Deleting branch: {}", branch);
        github.delete_branch(&branch).await?;
    }

    // // // // Prevent cleanup for debugging
    // let temp_path = temp_dir.keep();
    // println!("Test directory: {}", temp_path.display());

    Ok(())
}
