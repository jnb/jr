use anyhow::Result;
use clap::Parser;
use jr::git::RealGit;
use jr::github::RealGithub;
use jr::jujutsu::RealJujutsu;
use jr::{App, Cli, Commands, GLOBAL_BRANCH_PREFIX};

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
            app.cmd_update(&revision, message.as_deref(), &mut std::io::stdout())
                .await?
        }
        Some(Commands::Status) | None => {
            app.cmd_status(&mut std::io::stdout(), &mut std::io::stderr())
                .await?
        }
    }

    Ok(())
}
