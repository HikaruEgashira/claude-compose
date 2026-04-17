use std::collections::HashMap;
use std::fs;
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::cli::{LogsOpts, MessageType};
use crate::format::{format_entry, format_entry_json};
use crate::parser::{
    EntryType, LogEntry, TeamConfig, claude_home, cwd_to_project_key, discover_member_sessions,
    load_team_config, parse_line, project_log_dir, read_subagent_name,
    resolve_member_session_via_tmux,
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
    let mut agent_files = discover_files(&opts)?;
    let max_name_width = tail_entries(&mut agent_files, &opts)?;

    if opts.follow {
        follow_events(&mut agent_files, &opts, max_name_width).await?;
    }

    Ok(())
}

/// Discover agent log files based on --team flag or auto-detection.
/// Applies the agent name filter if specified.
fn discover_files(opts: &LogsOpts) -> anyhow::Result<Vec<AgentFile>> {
    let mut agent_files = if let Some(ref team_name) = opts.team {
        let config = load_team_config(team_name)?;
        let files = discover_team_files(&config)?;
        if files.is_empty() {
            anyhow::bail!(
                "No log files found for team '{team_name}'. \
                 The team's lead session ({}) may have ended.",
                config.lead_session_id
            );
        }
        files
    } else {
        let files = discover_all_sessions()?;
        if files.is_empty() && !opts.follow {
            anyhow::bail!(
                "No active sessions found in ~/.claude/sessions/. \
                 Start a Claude Code session first, or specify --team."
            );
        }
        files
    };

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

    Ok(agent_files)
}

/// Read existing log entries from all agent files, apply tail limit, and print.
/// Returns the computed max_name_width for consistent formatting in follow mode.
fn tail_entries(agent_files: &mut [AgentFile], opts: &LogsOpts) -> anyhow::Result<usize> {
    let max_name_width = agent_files
        .iter()
        .map(|af| af.agent_name.len())
        .max()
        .unwrap_or(10)
        .max(10);

    let mut all_entries: Vec<LogEntry> = Vec::new();
    for af in agent_files.iter_mut() {
        all_entries.extend(read_file_entries(af)?);
    }

    all_entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    let start = all_entries.len().saturating_sub(opts.tail);
    for entry in &all_entries[start..] {
        if !matches_filter(&entry.message_type, &opts.type_filter) {
            continue;
        }
        if should_skip_thinking(&entry.message_type, opts) {
            continue;
        }
        if should_skip_sidechain(entry, opts) {
            continue;
        }
        print_entry(entry, opts, max_name_width);
    }

    Ok(max_name_width)
}

/// Watch for file changes and stream new log entries in real time.
async fn follow_events(
    agent_files: &mut Vec<AgentFile>,
    opts: &LogsOpts,
    max_name_width: usize,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<PathBuf>(256);

    let mut watcher = notify::recommended_watcher({
        let tx = tx.clone();
        move |res: Result<Event, _>| match res {
            Ok(event) => {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in event.paths {
                        let _ = tx.blocking_send(path);
                    }
                }
            }
            Err(e) => {
                eprintln!("[claude-compose] watcher error: {e}");
            }
        }
    })?;
    // Drop the original sender so the channel closes only when the watcher's clone is dropped.
    drop(tx);

    // Watch directories containing our log files
    let mut watched_dirs = std::collections::HashSet::new();
    for af in agent_files.iter() {
        if let Some(dir) = af.path.parent() {
            if watched_dirs.insert(dir.to_path_buf()) {
                watcher.watch(dir, RecursiveMode::NonRecursive)?;
            }
        }
    }

    // Also watch subagent directories for dynamically spawned agents
    for subagents_dir in derive_subagent_dirs(agent_files) {
        if watched_dirs.insert(subagents_dir.clone()) {
            watcher.watch(&subagents_dir, RecursiveMode::NonRecursive)?;
        }
    }

    // Watch ~/.claude/sessions/ to discover new sessions dynamically
    let claude = claude_home()?;
    let sessions_dir = claude.join("sessions");
    if sessions_dir.is_dir() && watched_dirs.insert(sessions_dir.clone()) {
        watcher.watch(&sessions_dir, RecursiveMode::NonRecursive)?;
    }

    let own_ids = find_own_session_ids();
    let mut seen_session_ids: std::collections::HashSet<String> = agent_files
        .iter()
        .filter_map(|af| af.path.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    let mut path_to_idx: HashMap<PathBuf, usize> = agent_files
        .iter()
        .enumerate()
        .map(|(i, af)| (af.path.clone(), i))
        .collect();

    loop {
        let Some(changed_path) = rx.recv().await else {
            anyhow::bail!("file watcher stopped unexpectedly");
        };
        // New session PID file in ~/.claude/sessions/
        if changed_path.parent() == Some(sessions_dir.as_ref())
            && changed_path.extension().is_some_and(|e| e == "json")
        {
            if let Some(new_files) =
                try_register_session(&changed_path, &claude, &own_ids, &mut seen_session_ids)
            {
                for (jsonl, name, _) in new_files {
                    if path_to_idx.contains_key(&jsonl) {
                        continue;
                    }
                    if let Some(dir) = jsonl.parent() {
                        if watched_dirs.insert(dir.to_path_buf()) {
                            let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
                        }
                    }
                    let name = unique_name(&name, agent_files);
                    let color = Some(color_for_name(&name));
                    let idx = agent_files.len();
                    agent_files.push(AgentFile {
                        path: jsonl.clone(),
                        agent_name: name,
                        agent_color: color,
                        offset: 0,
                    });
                    path_to_idx.insert(jsonl, idx);
                }
            }
            continue;
        }

        if let Some(&idx) = path_to_idx.get(&changed_path) {
            let af = &mut agent_files[idx];
            let new_entries = read_new_lines(af)?;
            for entry in new_entries {
                if !matches_filter(&entry.message_type, &opts.type_filter) {
                    continue;
                }
                if should_skip_thinking(&entry.message_type, opts) {
                    continue;
                }
                if should_skip_sidechain(&entry, opts) {
                    continue;
                }
                print_entry(&entry, opts, max_name_width);
            }
        } else if changed_path.extension().is_some_and(|e| e == "jsonl")
            && changed_path.is_file()
            && !path_to_idx.contains_key(&changed_path)
        {
            // New subagent JSONL detected -- register it dynamically
            let (base_name, _) = resolve_subagent_info(&changed_path);
            let name = unique_name(&base_name, agent_files);
            let color = Some(color_for_name(&name));
            let idx = agent_files.len();
            agent_files.push(AgentFile {
                path: changed_path.clone(),
                agent_name: name,
                agent_color: color,
                offset: 0,
            });
            path_to_idx.insert(changed_path, idx);
        }
    }
}

/// Try to register a newly appeared session PID file.
/// Returns a list of (jsonl_path, agent_name, color) for the session and its subagents,
/// or None if the file is not a valid/relevant session.
fn try_register_session(
    pid_file: &std::path::Path,
    claude_dir: &std::path::Path,
    own_ids: &[String],
    seen_session_ids: &mut std::collections::HashSet<String>,
) -> Option<Vec<(PathBuf, String, Option<String>)>> {
    let stem = pid_file.file_stem()?.to_str()?;
    let pid: u32 = stem.parse().ok()?;

    if !is_process_alive(pid) {
        return None;
    }

    let info = read_session_file(pid_file)?;

    if own_ids.contains(&info.session_id) {
        return None;
    }

    if !seen_session_ids.insert(info.session_id.clone()) {
        return None;
    }

    let project_key = cwd_to_project_key(&info.cwd);
    let project_dir = claude_dir.join("projects").join(&project_key);
    let jsonl = project_dir.join(format!("{}.jsonl", info.session_id));
    if !jsonl.is_file() {
        return None;
    }

    let mut results = Vec::new();

    let name = info
        .agent_name
        .unwrap_or_else(|| session_display_name(&info.cwd, &info.session_id));
    let color = Some(color_for_name(&name));
    results.push((jsonl, name, color));

    // Subagent JSONLs
    let subagents_dir = project_dir.join(&info.session_id).join("subagents");
    for af in scan_subagent_dir(&subagents_dir) {
        results.push((af.path, af.agent_name, af.agent_color));
    }

    Some(results)
}

/// Discover all active Claude Code sessions from ~/.claude/sessions/.
/// Each {PID}.json file represents a session; only processes still alive are included.
fn discover_all_sessions() -> anyhow::Result<Vec<AgentFile>> {
    let claude = claude_home()?;
    let sessions_dir = claude.join("sessions");
    if !sessions_dir.is_dir() {
        return Ok(vec![]);
    }

    let mut files = Vec::new();
    let mut seen_session_ids = std::collections::HashSet::new();

    for entry in fs::read_dir(&sessions_dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(pid) = stem.parse::<u32>() else {
            continue;
        };

        if !is_process_alive(pid) {
            continue;
        }

        let Some(info) = read_session_file(&path) else {
            continue;
        };

        if !seen_session_ids.insert(info.session_id.clone()) {
            continue;
        }

        let project_key = cwd_to_project_key(&info.cwd);
        let project_dir = claude.join("projects").join(&project_key);
        if !project_dir.is_dir() {
            continue;
        }

        let jsonl = project_dir.join(format!("{}.jsonl", info.session_id));
        if !jsonl.is_file() {
            continue;
        }

        let name = info
            .agent_name
            .unwrap_or_else(|| session_display_name(&info.cwd, &info.session_id));
        let color = Some(color_for_name(&name));

        files.push(AgentFile {
            path: jsonl,
            agent_name: name,
            agent_color: color,
            offset: 0,
        });

        // Subagent JSONLs within the session directory
        let subagents_dir = project_dir.join(&info.session_id).join("subagents");
        files.extend(scan_subagent_dir(&subagents_dir));
    }

    // Deduplicate names (e.g. two sessions in the same project dir)
    deduplicate_names(&mut files);

    // Exclude own session to prevent feedback loop
    exclude_own_sessions(&mut files);

    Ok(files)
}

struct SessionInfo {
    session_id: String,
    cwd: String,
    agent_name: Option<String>,
}

/// Read session metadata from a PID session file using BufReader.
fn read_session_file(path: &std::path::Path) -> Option<SessionInfo> {
    use std::io::BufReader;

    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let v: serde_json::Value = serde_json::from_reader(reader).ok()?;
    let session_id = v.get("sessionId")?.as_str()?.to_string();
    let cwd = v.get("cwd")?.as_str()?.to_string();
    let agent_name = v
        .get("agentName")
        .and_then(|a| a.as_str())
        .map(String::from);
    Some(SessionInfo {
        session_id,
        cwd,
        agent_name,
    })
}

/// Check if a process is still alive using kill -0.
fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Resolve a subagent's display name and color from its JSONL path.
/// Reads the companion `.meta.json` if it exists; falls back to stripping
/// the `agent-` prefix from the file stem.
fn resolve_subagent_info(jsonl_path: &std::path::Path) -> (String, Option<String>) {
    let meta_path = jsonl_path.with_extension("meta.json");
    let name = read_subagent_name(&meta_path).unwrap_or_else(|| {
        jsonl_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .strip_prefix("agent-")
            .unwrap_or("unknown")
            .to_string()
    });
    let color = Some(color_for_name(&name));
    (name, color)
}

/// Scan a subagents directory and return AgentFile entries for each .jsonl found.
fn scan_subagent_dir(subagents_dir: &std::path::Path) -> Vec<AgentFile> {
    let mut files = Vec::new();
    if !subagents_dir.is_dir() {
        return files;
    }
    let Ok(entries) = fs::read_dir(subagents_dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            let (name, color) = resolve_subagent_info(&path);
            files.push(AgentFile {
                path,
                agent_name: name,
                agent_color: color,
                offset: 0,
            });
        }
    }
    files
}

/// Derive subagent directories from discovered AgentFile paths.
/// For a JSONL at `projects/{key}/{session_id}.jsonl`, the subagents dir
/// is `projects/{key}/{session_id}/subagents/`.
fn derive_subagent_dirs(files: &[AgentFile]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for af in files {
        // Skip files already inside a subagents directory
        if af
            .path
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == "subagents")
        {
            continue;
        }
        if let (Some(stem), Some(parent)) = (af.path.file_stem(), af.path.parent()) {
            let subagents = parent.join(stem).join("subagents");
            if subagents.is_dir() {
                dirs.push(subagents);
            }
        }
    }
    dirs
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
    files.extend(scan_subagent_dir(&subagents_dir));

    // 3. Team member sessions (tmux-based members with independent JSONL files)
    let known_sessions = collect_known_sessions(&files);
    let member_sessions = resolve_member_sessions(config, &project_dir);

    for (member_session_id, member_name, member_color) in &member_sessions {
        if known_sessions.contains(member_session_id.as_str()) {
            continue;
        }
        let member_jsonl = project_dir.join(format!("{member_session_id}.jsonl"));
        if member_jsonl.is_file() {
            files.push(AgentFile {
                path: member_jsonl,
                agent_name: member_name.clone(),
                agent_color: member_color.clone(),
                offset: 0,
            });
        }
    }

    // 4. Exclude own session to prevent feedback loop in follow mode.
    //    When claude-compose runs inside a Claude Code session, its stdout
    //    is captured as tool_result in the session's JSONL. Watching that
    //    file creates an infinite read-print-write cycle.
    exclude_own_sessions(&mut files);

    Ok(files)
}

/// Read entries from a JSONL file using BufReader (no full-file slurp).
/// Sets `af.offset` to end-of-file for subsequent follow-mode reads.
fn read_file_entries(af: &mut AgentFile) -> anyhow::Result<Vec<LogEntry>> {
    use std::io::{BufRead, BufReader};

    let file = fs::File::open(&af.path)?;
    let file_len = file.metadata()?.len();
    let reader = BufReader::new(file);

    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed = parse_line(&line, &af.agent_name, af.agent_color.as_deref());
        entries.extend(parsed);
    }

    af.offset = file_len;
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
        Some(pos) => pos + 1,      // include the newline
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

/// Collect session IDs already tracked in the file list to avoid duplicates.
fn collect_known_sessions(files: &[AgentFile]) -> std::collections::HashSet<String> {
    files
        .iter()
        .filter_map(|af| af.path.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

/// Resolve member session IDs using two strategies:
/// 1. tmux pane -> PID -> session file (for active members)
/// 2. JSONL scan fallback (for all members including terminated ones)
///
/// Returns Vec<(session_id, member_name, color)>.
fn resolve_member_sessions(
    config: &TeamConfig,
    project_dir: &std::path::Path,
) -> Vec<(String, String, Option<String>)> {
    let mut results: Vec<(String, String, Option<String>)> = Vec::new();
    let mut resolved_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Strategy 1: tmux resolution for members with pane IDs
    for member in &config.members {
        if member.name == "team-lead" {
            continue;
        }
        if let Some(pane_id) = &member.tmux_pane_id
            && let Some(sid) = resolve_member_session_via_tmux(pane_id)
            && sid != config.lead_session_id
        {
            resolved_names.insert(member.name.clone());
            results.push((sid, member.name.clone(), member.color.clone()));
        }
    }

    // Strategy 2: JSONL scan fallback for members not resolved via tmux
    let scanned = discover_member_sessions(project_dir, &config.team_name, &config.lead_session_id);
    for (sid, agent_name) in scanned {
        if resolved_names.contains(&agent_name) {
            continue;
        }
        // Look up color from config
        let color = config
            .members
            .iter()
            .find(|m| m.name == agent_name)
            .and_then(|m| m.color.clone());
        resolved_names.insert(agent_name.clone());
        results.push((sid, agent_name, color));
    }

    results
}

/// Derive a human-readable session name from the cwd path.
/// "/Users/hikae/ghq/github.com/Foo/bar" → "bar"
/// Falls back to "session-{short_id}" when cwd is empty or root.
fn session_display_name(cwd: &str, session_id: &str) -> String {
    if !cwd.is_empty() {
        let trimmed = cwd.trim_end_matches('/');
        if let Some(pos) = trimmed.rfind('/') {
            let basename = &trimmed[pos + 1..];
            if !basename.is_empty() {
                return basename.to_string();
            }
        } else if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let short_id = &session_id[..session_id.len().min(8)];
    format!("session-{short_id}")
}

/// Deduplicate agent names by appending `-2`, `-3`, … to collisions.
/// The first occurrence keeps the original name.
fn deduplicate_names(files: &mut [AgentFile]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for af in files.iter_mut() {
        let count = counts.entry(af.agent_name.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            af.agent_name = format!("{}-{}", af.agent_name, count);
        }
    }
}

/// Generate a unique name that doesn't collide with existing agent names.
fn unique_name(base: &str, existing: &[AgentFile]) -> String {
    if !existing.iter().any(|af| af.agent_name == base) {
        return base.to_string();
    }
    for i in 2.. {
        let candidate = format!("{base}-{i}");
        if !existing.iter().any(|af| af.agent_name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

const DEFAULT_COLORS: &[&str] = &["blue", "green", "yellow", "cyan", "magenta", "red"];

/// Deterministic color assignment based on agent name.
/// Uses FNV-1a for better distribution so agents get varied colors.
fn color_for_name(name: &str) -> String {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let hash = name.bytes().fold(FNV_OFFSET, |acc, b| {
        (acc ^ u32::from(b)).wrapping_mul(FNV_PRIME)
    });
    DEFAULT_COLORS[(hash as usize) % DEFAULT_COLORS.len()].to_string()
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
            | (MessageType::Thinking, EntryType::Thinking)
            | (MessageType::Summary, EntryType::Summary)
            | (MessageType::Result, EntryType::Result)
            | (MessageType::Snapshot, EntryType::Snapshot)
    )
}

/// Decide whether to skip a Thinking entry based on `--show-thinking` and
/// the active type filter. Thinking entries are hidden by default unless:
///   - the user explicitly passed `--show-thinking`, OR
///   - the user explicitly filtered to `--type thinking`.
fn should_skip_thinking(entry_type: &EntryType, opts: &LogsOpts) -> bool {
    if *entry_type != EntryType::Thinking {
        return false;
    }
    if opts.show_thinking {
        return false;
    }
    if matches!(opts.type_filter, Some(MessageType::Thinking)) {
        return false;
    }
    true
}

/// Decide whether to skip a sidechain (subagent/Task-tool) entry.
/// Sidechain entries are shown by default; they are only suppressed when
/// the user explicitly passed `--hide-sidechain`.
fn should_skip_sidechain(entry: &LogEntry, opts: &LogsOpts) -> bool {
    opts.hide_sidechain && entry.is_sidechain
}

fn print_entry(entry: &LogEntry, opts: &LogsOpts, max_name_width: usize) {
    if opts.json {
        println!("{}", format_entry_json(entry, opts.verbose));
    } else {
        println!(
            "{}",
            format_entry(entry, opts.verbose, opts.no_color, max_name_width)
        );
    }
}

/// Remove any AgentFiles whose session ID matches the current process's
/// ancestor chain to prevent feedback loops.
fn exclude_own_sessions(files: &mut Vec<AgentFile>) {
    let own_ids = find_own_session_ids();
    if !own_ids.is_empty() {
        files.retain(|af| {
            let stem = af
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            !own_ids.contains(&stem)
        });
    }
}

/// Detect session IDs belonging to the current process's ancestor chain.
///
/// Claude Code stores session metadata at ~/.claude/sessions/{PID}.json.
/// By walking up the process tree from our PID, we can find which Claude Code
/// session (if any) is our ancestor, and exclude its JSONL from the watcher
/// to prevent the feedback loop.
fn find_own_session_ids() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let sessions_dir = home.join(".claude").join("sessions");
    if !sessions_dir.is_dir() {
        return vec![];
    }

    let mut ids = Vec::new();
    let mut pid = std::process::id();

    for _ in 0..32 {
        let session_file = sessions_dir.join(format!("{pid}.json"));
        if let Ok(data) = fs::read_to_string(&session_file)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&data)
            && let Some(sid) = v.get("sessionId").and_then(|s| s.as_str())
        {
            ids.push(sid.to_string());
        }

        match parent_pid(pid) {
            Some(ppid) if ppid > 1 && ppid != pid => pid = ppid,
            _ => break,
        }
    }

    ids
}

/// Get the parent PID of a given process via `ps`.
fn parent_pid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_logs_opts() -> LogsOpts {
        LogsOpts {
            follow: false,
            tail: 50,
            type_filter: None,
            json: false,
            no_color: true,
            team: None,
            verbose: false,
            show_thinking: false,
            hide_sidechain: false,
            agents: vec![],
        }
    }

    #[test]
    fn matches_filter_none_passes_all() {
        assert!(matches_filter(&EntryType::Thinking, &None));
        assert!(matches_filter(&EntryType::Summary, &None));
        assert!(matches_filter(&EntryType::Result, &None));
        assert!(matches_filter(&EntryType::Snapshot, &None));
    }

    #[test]
    fn matches_filter_maps_new_variants() {
        assert!(matches_filter(
            &EntryType::Thinking,
            &Some(MessageType::Thinking)
        ));
        assert!(matches_filter(
            &EntryType::Summary,
            &Some(MessageType::Summary)
        ));
        assert!(matches_filter(
            &EntryType::Result,
            &Some(MessageType::Result)
        ));
        assert!(matches_filter(
            &EntryType::Snapshot,
            &Some(MessageType::Snapshot)
        ));
        assert!(!matches_filter(
            &EntryType::Assistant,
            &Some(MessageType::Thinking)
        ));
    }

    #[test]
    fn thinking_hidden_by_default() {
        let opts = make_logs_opts();
        assert!(should_skip_thinking(&EntryType::Thinking, &opts));
        assert!(!should_skip_thinking(&EntryType::Assistant, &opts));
    }

    #[test]
    fn thinking_shown_with_show_thinking_flag() {
        let mut opts = make_logs_opts();
        opts.show_thinking = true;
        assert!(!should_skip_thinking(&EntryType::Thinking, &opts));
    }

    #[test]
    fn thinking_shown_with_explicit_type_filter() {
        let mut opts = make_logs_opts();
        opts.type_filter = Some(MessageType::Thinking);
        assert!(!should_skip_thinking(&EntryType::Thinking, &opts));
    }

    fn make_sidechain_entry(is_sidechain: bool) -> LogEntry {
        LogEntry {
            timestamp: "T".to_string(),
            agent_name: "agent".to_string(),
            agent_color: None,
            message_type: EntryType::Assistant,
            content: "hi".to_string(),
            tool_name: None,
            is_error: false,
            is_sidechain,
        }
    }

    #[test]
    fn sidechain_shown_by_default() {
        let opts = make_logs_opts();
        let entry = make_sidechain_entry(true);
        assert!(!should_skip_sidechain(&entry, &opts));
    }

    #[test]
    fn sidechain_hidden_with_hide_sidechain_flag() {
        let mut opts = make_logs_opts();
        opts.hide_sidechain = true;
        let sidechain = make_sidechain_entry(true);
        let regular = make_sidechain_entry(false);
        assert!(should_skip_sidechain(&sidechain, &opts));
        assert!(!should_skip_sidechain(&regular, &opts));
    }

    #[test]
    fn parent_pid_returns_valid_for_self() {
        let my_pid = std::process::id();
        let ppid = parent_pid(my_pid);
        assert!(ppid.is_some(), "should resolve parent of current process");
        assert!(ppid.unwrap() > 0);
    }

    #[test]
    fn parent_pid_returns_none_for_nonexistent() {
        let ppid = parent_pid(4_000_000_000);
        assert!(ppid.is_none());
    }

    #[test]
    fn find_own_session_ids_does_not_panic() {
        // Should return empty or valid IDs without panicking,
        // regardless of whether ~/.claude/sessions/ exists.
        let ids = find_own_session_ids();
        for id in &ids {
            assert!(!id.is_empty());
        }
    }

    #[test]
    fn self_exclusion_filters_matching_session() {
        let files = vec![
            AgentFile {
                path: PathBuf::from("/tmp/projects/abc-123.jsonl"),
                agent_name: "team-lead".to_string(),
                agent_color: None,
                offset: 0,
            },
            AgentFile {
                path: PathBuf::from("/tmp/projects/def-456.jsonl"),
                agent_name: "impl-loop".to_string(),
                agent_color: Some("green".to_string()),
                offset: 0,
            },
        ];

        let own_ids = vec!["def-456".to_string()];

        let filtered: Vec<_> = files
            .into_iter()
            .filter(|af| {
                let stem = af
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                !own_ids.contains(&stem)
            })
            .collect();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].agent_name, "team-lead");
    }

    #[test]
    fn self_exclusion_no_match_retains_all() {
        let files = vec![
            AgentFile {
                path: PathBuf::from("/tmp/projects/abc-123.jsonl"),
                agent_name: "team-lead".to_string(),
                agent_color: None,
                offset: 0,
            },
            AgentFile {
                path: PathBuf::from("/tmp/projects/def-456.jsonl"),
                agent_name: "impl-loop".to_string(),
                agent_color: None,
                offset: 0,
            },
        ];

        let own_ids = vec!["xyz-789".to_string()];

        let filtered: Vec<_> = files
            .into_iter()
            .filter(|af| {
                let stem = af
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                !own_ids.contains(&stem)
            })
            .collect();

        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn color_for_name_is_deterministic() {
        let c1 = color_for_name("my-agent");
        let c2 = color_for_name("my-agent");
        assert_eq!(c1, c2);
    }

    #[test]
    fn color_for_name_differs_for_different_names() {
        // Not guaranteed for all pairs, but these specific names should differ
        let c1 = color_for_name("Explore");
        let c2 = color_for_name("general-purpose");
        assert_ne!(c1, c2);
    }

    #[test]
    fn color_for_name_returns_valid_color() {
        let color = color_for_name("test-agent");
        assert!(DEFAULT_COLORS.contains(&color.as_str()));
    }

    #[test]
    fn self_exclusion_empty_ids_retains_all() {
        let files = vec![AgentFile {
            path: PathBuf::from("/tmp/projects/abc-123.jsonl"),
            agent_name: "team-lead".to_string(),
            agent_color: None,
            offset: 0,
        }];

        let own_ids: Vec<String> = vec![];

        let filtered: Vec<_> = files
            .into_iter()
            .filter(|af| {
                let stem = af
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                !own_ids.contains(&stem)
            })
            .collect();

        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn session_display_name_uses_basename() {
        assert_eq!(
            session_display_name("/Users/hikae/ghq/github.com/Foo/bar", "abc12345-xyz"),
            "bar"
        );
    }

    #[test]
    fn session_display_name_trailing_slash() {
        assert_eq!(
            session_display_name("/Users/hikae/project/", "abc12345"),
            "project"
        );
    }

    #[test]
    fn session_display_name_fallback_on_empty_cwd() {
        assert_eq!(
            session_display_name("", "abc12345-long-id"),
            "session-abc12345"
        );
    }

    #[test]
    fn session_display_name_root_path() {
        assert_eq!(
            session_display_name("/", "abc12345-long-id"),
            "session-abc12345"
        );
    }

    fn make_agent_file(name: &str) -> AgentFile {
        AgentFile {
            path: PathBuf::from(format!("/tmp/{name}.jsonl")),
            agent_name: name.to_string(),
            agent_color: None,
            offset: 0,
        }
    }

    #[test]
    fn deduplicate_names_adds_suffix() {
        let mut files = vec![
            make_agent_file("my-project"),
            make_agent_file("my-project"),
            make_agent_file("my-project"),
            make_agent_file("other"),
        ];
        deduplicate_names(&mut files);
        assert_eq!(files[0].agent_name, "my-project");
        assert_eq!(files[1].agent_name, "my-project-2");
        assert_eq!(files[2].agent_name, "my-project-3");
        assert_eq!(files[3].agent_name, "other");
    }

    #[test]
    fn deduplicate_names_no_duplicates() {
        let mut files = vec![make_agent_file("a"), make_agent_file("b")];
        deduplicate_names(&mut files);
        assert_eq!(files[0].agent_name, "a");
        assert_eq!(files[1].agent_name, "b");
    }

    #[test]
    fn unique_name_no_collision() {
        let existing = vec![make_agent_file("alpha")];
        assert_eq!(unique_name("beta", &existing), "beta");
    }

    #[test]
    fn unique_name_with_collision() {
        let existing = vec![make_agent_file("proj"), make_agent_file("proj-2")];
        assert_eq!(unique_name("proj", &existing), "proj-3");
    }
}
