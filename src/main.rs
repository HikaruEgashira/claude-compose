mod cli;
mod format;
mod parser;
mod watcher;

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_args();

    match cli.command {
        Command::Logs(opts) => {
            watcher::run(opts).await?;
        }
        Command::Ps(opts) => {
            format::print_ps(opts)?;
        }
    }

    Ok(())
}
