mod log;
mod session;
mod team;

pub use log::{EntryType, LogEntry, TagKind, Usage, format_timestamp, parse_line};
// `classify_tag` / `detect_first_tag` are public in `log.rs` for testability,
// but not re-exported here: nothing outside the parser needs to classify tag
// strings directly — downstream code reads `LogEntry.tag` instead.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use log::{classify_tag, detect_first_tag};
pub use session::{discover_member_sessions, resolve_member_session_via_tmux};
pub use team::{
    TeamConfig, claude_home, cwd_to_project_key, find_teams, load_team_config, project_log_dir,
    read_subagent_name,
};

#[cfg(test)]
pub use team::MemberInfo;
