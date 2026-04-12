use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "claude-compose",
    version,
    about = "Real-time log viewer for Claude Code Agent Teams"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Stream logs from Claude Code Agent Team sessions
    #[command(alias = "log")]
    Logs(LogsOpts),
    /// Show agent status (like docker ps)
    Ps(PsOpts),
    /// Start team members in tmux panes (like docker compose up)
    Up(UpOpts),
    /// Stop team members (like docker compose down)
    Down(DownOpts),
}

#[derive(clap::Args)]
pub struct LogsOpts {
    /// Follow log output (like tail -f)
    #[arg(short, long)]
    pub follow: bool,

    /// Number of lines to show from the end
    #[arg(long, default_value = "50")]
    pub tail: usize,

    /// Filter by message type
    #[arg(long = "type", value_enum)]
    pub type_filter: Option<MessageType>,

    /// Output as JSON (pipe-friendly)
    #[arg(long)]
    pub json: bool,

    /// Disable colored output
    #[arg(long)]
    pub no_color: bool,

    /// Team name (auto-detect if omitted)
    #[arg(long)]
    pub team: Option<String>,

    /// Show full tool_result output
    #[arg(long)]
    pub verbose: bool,

    /// Filter by agent names
    pub agents: Vec<String>,
}

#[derive(clap::Args)]
pub struct PsOpts {
    /// Team name (auto-detect if omitted)
    #[arg(long)]
    pub team: Option<String>,

    /// Output as JSON (pipe-friendly)
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct UpOpts {
    /// Project directory (default: current directory)
    #[arg(default_value = ".")]
    pub path: String,
}

#[derive(clap::Args)]
pub struct DownOpts {
    /// Project directory (default: current directory)
    #[arg(default_value = ".")]
    pub path: String,
}

#[derive(Clone, ValueEnum)]
pub enum MessageType {
    Assistant,
    User,
    System,
    ToolUse,
    ToolResult,
}
