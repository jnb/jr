use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::Layer as _;

/// Creates a git repository in the given directory.
///
/// This initializes the repo and sets basic git config needed for commits.
/// The directory should already exist.
pub async fn create_git_repo(dir: &Path) -> anyhow::Result<()> {
    // Initialize git repo
    let status = Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "git init failed");

    // Set git config for commits
    let status = Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "git config user.name failed");

    let status = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "git config user.email failed");

    Ok(())
}

/// Sets up a git remote origin for the repository.
pub async fn setup_git_remote(dir: &Path, remote_url: &str) -> anyhow::Result<()> {
    let status = Command::new("git")
        .args(["remote", "add", "origin", remote_url])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "git remote add origin failed");

    Ok(())
}

/// Initializes jujutsu in an existing git repository.
pub async fn init_jujutsu(dir: &Path) -> anyhow::Result<()> {
    let status = Command::new("jj")
        .args(["git", "init", "--colocate"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "jj git init failed");

    Ok(())
}

/// Tracks a remote branch in jujutsu.
pub async fn track_branch(dir: &Path, branch: &str, remote: &str) -> anyhow::Result<()> {
    let branch_ref = format!("{}@{}", branch, remote);
    let status = Command::new("jj")
        .args(["bookmark", "track", &branch_ref])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "jj branch track failed");

    Ok(())
}

/// Fetches from git remote using jujutsu.
pub async fn jj_git_fetch(dir: &Path) -> anyhow::Result<()> {
    let status = Command::new("jj")
        .args(["git", "fetch"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "jj git fetch failed");

    Ok(())
}

/// Creates a new jujutsu change on top of a specific revision.
pub async fn jj_new(dir: &Path, revision: &str) -> anyhow::Result<()> {
    let status = Command::new("jj")
        .args(["new", revision])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "jj new failed");

    Ok(())
}

/// Checks out (edits) a specific jujutsu revision.
pub async fn jj_edit(dir: &Path, revision: &str) -> anyhow::Result<()> {
    let status = Command::new("jj")
        .args(["edit", revision])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "jj edit failed");

    Ok(())
}

/// Creates a jujutsu commit with a file.
pub async fn create_jj_commit(
    dir: &Path,
    message: &str,
    filename: &str,
    contents: &str,
) -> anyhow::Result<()> {
    // Write the file
    let file_path = dir.join(filename);
    tokio::fs::write(&file_path, contents).await?;

    // Commit the change
    let status = Command::new("jj")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "jj commit failed");

    Ok(())
}

/// Gets the output of jj log with a simple template (no timestamps/IDs).
pub async fn jj_log(dir: &Path) -> anyhow::Result<String> {
    let output = Command::new("jj")
        .args([
            "log",
            "-r trunk()::",
            "-T",
            r#"if(description, description.first_line(), "(no description)") ++ "\n""#,
        ])
        .current_dir(dir)
        .output()
        .await?;
    anyhow::ensure!(output.status.success(), "jj log failed");

    Ok(String::from_utf8(output.stdout)?)
}

pub fn setup_logging() -> anyhow::Result<()> {
    let timer = tracing_subscriber::fmt::time::ChronoLocal::new("%H:%M:%S%.3f".into());
    let format = tracing_subscriber::fmt::format().with_timer(timer);
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;
    let subscriber = tracing_subscriber::fmt::layer()
        .event_format(format)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(filter);
    tracing_subscriber::registry().with(subscriber).init();
    Ok(())
}

pub enum TestDir {
    Temp(tempfile::TempDir),
    Kept(std::path::PathBuf),
}

impl TestDir {
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = tempfile::tempdir()?;

        if std::env::var("DEBUG_TESTS").is_ok() {
            let path = temp_dir.keep();
            eprintln!("Test directory kept at: {}", path.display());
            Ok(TestDir::Kept(path))
        } else {
            Ok(TestDir::Temp(temp_dir))
        }
    }

    pub fn path(&self) -> &std::path::Path {
        match self {
            TestDir::Temp(t) => t.path(),
            TestDir::Kept(p) => p.as_path(),
        }
    }
}
