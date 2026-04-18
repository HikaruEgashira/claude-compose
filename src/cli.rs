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

    /// Show assistant thinking blocks (hidden by default)
    #[arg(long)]
    pub show_thinking: bool,

    /// Hide sidechain (subagent/Task-tool) entries — shown by default
    #[arg(long)]
    pub hide_sidechain: bool,

    /// Append per-record metadata (model, usage tokens) to each non-JSON line
    #[arg(long)]
    pub show_metadata: bool,

    /// Only show entries with timestamp >= this ISO-8601 value (lexicographic match).
    /// Prefix accepted, e.g. `--since 2026-04-12` or `--since 2026-04-12T09:00`.
    #[arg(long)]
    pub since: Option<String>,

    /// Only show entries with timestamp < this ISO-8601 value (lexicographic match).
    #[arg(long)]
    pub until: Option<String>,

    /// Only show entries whose `sessionId` matches (exact). Lets you focus on
    /// a single subagent when sidechain records are mixed into the stream.
    #[arg(long)]
    pub session: Option<String>,

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

#[derive(Clone, ValueEnum, PartialEq)]
pub enum MessageType {
    Assistant,
    User,
    System,
    ToolUse,
    ToolResult,
    Thinking,
    Summary,
    Result,
    Snapshot,
    /// Match `system` records with `subtype: "compact_boundary"` — the
    /// marker Claude Code emits when it auto-compacts the transcript.
    CompactBoundary,
    /// Match User/Assistant entries whose content carries a recognised
    /// slash-command tag (e.g. `<command-name>`).
    SlashCommand,
    /// Match User/Assistant entries whose content carries a `*-hook` tag
    /// (covers hooks v1 and v2 variants).
    Hook,
    /// Match User/Assistant entries whose content carries a
    /// `<system-reminder>` tag.
    Reminder,
    /// Match entries wrapped in `<github-webhook-activity>` (PR/CI events
    /// injected by the GitHub integration).
    GithubActivity,
    /// Match environment-level injections (`<available-skills>`,
    /// `<user-memory>`, `<current-branch>`, etc.).
    Env,
}
