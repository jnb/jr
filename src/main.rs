use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use jr::App;
use jr::Config;
use jr::ops::git::RealGit;
use jr::ops::github::RealGithub;
use jr::ops::jujutsu::RealJujutsu;

#[derive(Parser)]
#[command(name = "jr")]
#[command(about = "Jujutsu Review: Manage Git branches and GitHub PRs in a stacked workflow", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize configuration file in the current repository
    Init,
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
    let cli = Cli::parse();

    // Handle Init command specially - it creates the config
    if matches!(cli.command, Some(Commands::Init)) {
        // For init, we don't need to load config first
        let temp_config = Config::default_for_tests(); // Placeholder, not used
        let temp_github = RealGithub::new(temp_config.github_token.clone())?;
        let app = App::new(temp_config, RealJujutsu, RealGit, temp_github);
        app.cmd_init(&mut std::io::stdout()).await?;
        return Ok(());
    }

    // For all other commands, load config first
    let config = Config::load()?;
    let github = RealGithub::new(config.github_token.clone())?;
    let app = App::new(config, RealJujutsu, RealGit, github);

    match cli.command {
        Some(Commands::Init) => unreachable!(), // Already handled above
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
