mod log;
mod session;
mod team;

pub use log::{EntryType, LogEntry, format_timestamp, parse_line};
pub use session::{discover_member_sessions, resolve_member_session_via_tmux};
pub use team::{
    TeamConfig, claude_home, cwd_to_project_key, find_teams, load_team_config, project_log_dir,
    read_subagent_name,
};

#[cfg(test)]
pub use team::MemberInfo;
