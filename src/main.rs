mod cli;
mod format;
mod parser;
mod ps;
mod tmux;
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
            ps::run(opts)?;
        }
        Command::Up(opts) => {
            tmux::run_up(opts)?;
        }
        Command::Down(opts) => {
            tmux::run_down(opts)?;
        }
    }

    Ok(())
}
