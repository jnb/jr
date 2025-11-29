use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use jr::git::RealGit;
use jr::github::RealGithub;
use jr::jujutsu::RealJujutsu;
use jr::App;
use jr::GLOBAL_BRANCH_PREFIX;

#[derive(Parser)]
#[command(name = "jr")]
#[command(about = "Jujutsu Review: Manage Git branches and GitHub PRs in a stacked workflow", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new PR (uses jj commit message)
    Create {
        /// Revision to use (defaults to @)
        #[arg(short, long, default_value = "@")]
        revision: String,
    },
    /// Update an existing PR with local changes
    Update {
        /// Revision to use (defaults to @)
        #[arg(short, long, default_value = "@")]
        revision: String,
        /// Commit message describing the changes
        #[arg(short, long)]
        message: String,
    },
    /// Restack an existing PR on updated parent (only works if no local changes)
    Restack {
        /// Revision to use (defaults to @)
        #[arg(short, long, default_value = "@")]
        revision: String,
    },
    /// Show status of stacked PRs
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new(
        RealJujutsu,
        RealGit,
        RealGithub::new(GLOBAL_BRANCH_PREFIX.to_string()),
    );

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Create { revision }) => {
            app.cmd_create(&revision, &mut std::io::stdout()).await?
        }
        Some(Commands::Update { revision, message }) => {
            app.cmd_update(&revision, &message, &mut std::io::stdout())
                .await?
        }
        Some(Commands::Restack { revision }) => {
            app.cmd_restack(&revision, &mut std::io::stdout()).await?
        }
        Some(Commands::Status) | None => {
            app.cmd_status(&mut std::io::stdout(), &mut std::io::stderr())
                .await?
        }
    }

    Ok(())
}
