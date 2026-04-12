use std::collections::HashMap;
use std::fs;
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::cli::{LogsOpts, MessageType};
use crate::format::{format_entry, format_entry_json};
use crate::parser::{
    find_teams, load_team_config, parse_line, project_log_dir, read_subagent_name, EntryType,
    LogEntry, TeamConfig,
};

pub struct AgentFile {
    pub path: PathBuf,
    pub agent_name: String,
    pub agent_color: Option<String>,
    /// Byte offset of the last fully-read newline. Only complete lines
    /// (terminated by '\n') are consumed; a partial trailing line is left
    /// for the next read to pick up once the writer flushes the newline.
    pub offset: u64,
}

pub async fn run(opts: LogsOpts) -> anyhow::Result<()> {
    let team_name = resolve_team(&opts.team)?;
    let config = load_team_config(&team_name)?;

    // Discover JSONL files scoped to this team's lead session
    let mut agent_files = discover_team_files(&config)?;

    if agent_files.is_empty() {
        anyhow::bail!(
            "No log files found for team '{team_name}'. \
             The team's lead session ({}) may have ended.",
            config.lead_session_id
        );
    }

    // Apply agent name filter
    if !opts.agents.is_empty() {
        agent_files.retain(|af| {
            opts.agents
                .iter()
                .any(|a| af.agent_name.to_lowercase().contains(&a.to_lowercase()))
        });
        if agent_files.is_empty() {
            anyhow::bail!("No log files matching agent filter: {:?}", opts.agents);
        }
    }

    let max_name_width = agent_files
        .iter()
        .map(|af| af.agent_name.len())
        .max()
        .unwrap_or(10)
        .max(10);

    // Read existing content (tail)
    let mut all_entries: Vec<LogEntry> = Vec::new();
    for af in &mut agent_files {
        let entries = read_file_entries(af)?;
        all_entries.extend(entries);
    }

    // Sort by timestamp
    all_entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    // Apply tail limit
    let start = all_entries.len().saturating_sub(opts.tail);
    let tail_entries = &all_entries[start..];

    for entry in tail_entries {
        if !matches_filter(&entry.message_type, &opts.type_filter) {
            continue;
        }
        print_entry(entry, &opts, max_name_width);
    }

    if !opts.follow {
        return Ok(());
    }

    // Follow mode: watch for file changes using notify
    let (tx, mut rx) = mpsc::channel::<PathBuf>(256);

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                for path in event.paths {
                    let _ = tx.blocking_send(path);
                }
            }
        }
    })?;

    // Watch directories containing our log files
    let mut watched_dirs = std::collections::HashSet::new();
    for af in &agent_files {
        if let Some(dir) = af.path.parent() {
            if watched_dirs.insert(dir.to_path_buf()) {
                watcher.watch(dir, RecursiveMode::NonRecursive)?;
            }
        }
    }

    // Build path -> index lookup
    let path_to_idx: HashMap<PathBuf, usize> = agent_files
        .iter()
        .enumerate()
        .map(|(i, af)| (af.path.clone(), i))
        .collect();

    loop {
        match rx.recv().await {
            Some(changed_path) => {
                if let Some(&idx) = path_to_idx.get(&changed_path) {
                    let af = &mut agent_files[idx];
                    let new_entries = read_new_lines(af)?;
                    for entry in new_entries {
                        if !matches_filter(&entry.message_type, &opts.type_filter) {
                            continue;
                        }
                        print_entry(&entry, &opts, max_name_width);
                    }
                }
            }
            None => break,
        }
    }

    Ok(())
}

fn resolve_team(team_opt: &Option<String>) -> anyhow::Result<String> {
    if let Some(name) = team_opt {
        return Ok(name.clone());
    }

    let teams = find_teams();
    match teams.len() {
        0 => anyhow::bail!("No teams found in ~/.claude/teams/. Create a team first."),
        1 => Ok(teams.into_iter().next().unwrap()),
        _ => {
            eprintln!("Multiple teams found. Please specify --team:");
            for t in &teams {
                eprintln!("  {t}");
            }
            anyhow::bail!("Multiple teams found, use --team to specify")
        }
    }
}

/// Discover JSONL files scoped to a specific team's lead session.
/// Uses leadSessionId + cwd to find the exact session directory.
fn discover_team_files(config: &TeamConfig) -> anyhow::Result<Vec<AgentFile>> {
    let project_dir = project_log_dir(config)?;
    let session_id = &config.lead_session_id;

    if session_id.is_empty() {
        anyhow::bail!("Team '{}' has no leadSessionId", config.team_name);
    }

    let mut files = Vec::new();

    // 1. Lead session JSONL
    let lead_jsonl = project_dir.join(format!("{session_id}.jsonl"));
    if lead_jsonl.is_file() {
        let lead_name = config
            .members
            .iter()
            .find(|m| m.name == "team-lead")
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "team-lead".to_string());
        let lead_color = config
            .members
            .iter()
            .find(|m| m.name == "team-lead")
            .and_then(|m| m.color.clone());
        files.push(AgentFile {
            path: lead_jsonl,
            agent_name: lead_name,
            agent_color: lead_color,
            offset: 0,
        });
    }

    // 2. Subagent JSONLs within the lead session directory
    let subagents_dir = project_dir.join(session_id).join("subagents");
    if subagents_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&subagents_dir) {
            // Color rotation for subagents without explicit color
            let colors = ["blue", "green", "yellow", "cyan", "magenta", "red"];
            let mut color_idx = 0;

            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    let meta_path = path.with_extension("meta.json");
                    let name = read_subagent_name(&meta_path)
                        .unwrap_or_else(|| {
                            path.file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .strip_prefix("agent-")
                                .unwrap_or("unknown")
                                .to_string()
                        });

                    let color = Some(colors[color_idx % colors.len()].to_string());
                    color_idx += 1;

                    files.push(AgentFile {
                        path,
                        agent_name: name,
                        agent_color: color,
                        offset: 0,
                    });
                }
            }
        }
    }

    Ok(files)
}

fn read_file_entries(af: &mut AgentFile) -> anyhow::Result<Vec<LogEntry>> {
    let content = fs::read_to_string(&af.path)?;
    let mut entries = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = parse_line(line, &af.agent_name, af.agent_color.as_deref());
        entries.extend(parsed);
    }

    // Set offset to the end of the file for follow mode
    af.offset = content.len() as u64;

    Ok(entries)
}

/// Read new complete lines since the last offset.
/// Only advances the offset past fully terminated lines ('\n')
/// to avoid consuming a partial write.
fn read_new_lines(af: &mut AgentFile) -> anyhow::Result<Vec<LogEntry>> {
    let mut file = fs::File::open(&af.path)?;
    let file_len = file.metadata()?.len();

    if file_len <= af.offset {
        return Ok(vec![]);
    }

    file.seek(SeekFrom::Start(af.offset))?;

    let mut raw = Vec::new();
    io::Read::read_to_end(&mut file, &mut raw)?;

    // Find the last newline — everything after it is an incomplete line
    let consumed = match raw.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1, // include the newline
        None => return Ok(vec![]), // no complete line yet
    };

    af.offset += consumed as u64;

    let text = String::from_utf8_lossy(&raw[..consumed]);
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = parse_line(line, &af.agent_name, af.agent_color.as_deref());
        entries.extend(parsed);
    }

    Ok(entries)
}

fn matches_filter(entry_type: &EntryType, filter: &Option<MessageType>) -> bool {
    let Some(f) = filter else { return true };
    matches!(
        (f, entry_type),
        (MessageType::Assistant, EntryType::Assistant)
            | (MessageType::User, EntryType::User)
            | (MessageType::System, EntryType::System)
            | (MessageType::ToolUse, EntryType::ToolUse)
            | (MessageType::ToolResult, EntryType::ToolResult)
    )
}

fn print_entry(entry: &LogEntry, opts: &LogsOpts, max_name_width: usize) {
    if opts.json {
        println!("{}", format_entry_json(entry));
    } else {
        println!(
            "{}",
            format_entry(entry, opts.verbose, opts.no_color, max_name_width)
        );
    }
}
