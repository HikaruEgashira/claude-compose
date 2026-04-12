use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub agent_name: String,
    pub agent_color: Option<String>,
    pub message_type: EntryType,
    pub content: String,
    pub tool_name: Option<String>,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum EntryType {
    Assistant,
    User,
    System,
    ToolUse,
    ToolResult,
}

#[derive(Debug, Clone)]
pub struct TeamConfig {
    pub team_name: String,
    pub lead_session_id: String,
    pub cwd: String,
    pub members: Vec<MemberInfo>,
}

#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub name: String,
    pub color: Option<String>,
    pub is_active: bool,
    pub tmux_pane_id: Option<String>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub backend_type: Option<String>,
}

/// Parse a single JSONL line into zero or more LogEntry values.
pub fn parse_line(line: &str, agent_name: &str, agent_color: Option<&str>) -> Vec<LogEntry> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let top_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match top_type {
        "assistant" => parse_assistant(&v, &timestamp, agent_name, agent_color),
        "user" => parse_user(&v, &timestamp, agent_name, agent_color),
        "system" => parse_system(&v, &timestamp, agent_name, agent_color),
        _ => vec![],
    }
}

/// Format a raw ISO 8601 timestamp to HH:MM:SS for display.
pub fn format_timestamp(raw: &str) -> String {
    // "2026-04-12T12:57:14.123Z" → "12:57:14"
    if let Some(t_pos) = raw.find('T') {
        let after_t = &raw[t_pos + 1..];
        if after_t.len() >= 8 {
            return after_t[..8].to_string();
        }
    }
    raw.to_string()
}

fn parse_assistant(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
) -> Vec<LogEntry> {
    let content_blocks = match v.pointer("/message/content") {
        Some(Value::Array(arr)) => arr,
        _ => return vec![],
    };

    let mut entries = Vec::new();
    for block in content_blocks {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match block_type {
            "text" => {
                let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if !text.is_empty() {
                    entries.push(LogEntry {
                        timestamp: timestamp.to_string(),
                        agent_name: agent_name.to_string(),
                        agent_color: agent_color.map(String::from),
                        message_type: EntryType::Assistant,
                        content: text.to_string(),
                        tool_name: None,
                        is_error: false,
                    });
                }
            }
            "tool_use" => {
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown");
                let content = extract_tool_use_summary(name, block);
                entries.push(LogEntry {
                    timestamp: timestamp.to_string(),
                    agent_name: agent_name.to_string(),
                    agent_color: agent_color.map(String::from),
                    message_type: EntryType::ToolUse,
                    content,
                    tool_name: Some(name.to_string()),
                    is_error: false,
                });
            }
            "thinking" => {}
            _ => {}
        }
    }
    entries
}

/// Extract a human-readable summary from tool_use input instead of raw JSON.
fn extract_tool_use_summary(tool_name: &str, block: &Value) -> String {
    let input = block.get("input").cloned().unwrap_or(Value::Null);
    match tool_name {
        "SendMessage" => {
            let to = input.get("to").and_then(|v| v.as_str()).unwrap_or("?");
            let summary = input
                .get("summary")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    input.get("message").and_then(|m| m.as_str())
                })
                .unwrap_or("");
            format!("→ {to}: {summary}")
        }
        "TaskUpdate" => {
            let task_id = input.get("taskId").and_then(|v| v.as_str()).unwrap_or("?");
            let status = input.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status.is_empty() {
                format!("Task #{task_id}")
            } else {
                format!("Task #{task_id} → {status}")
            }
        }
        "TaskCreate" => {
            let subject = input.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            subject.to_string()
        }
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let short = if cmd.len() > 80 { &cmd[..80] } else { cmd };
            short.to_string()
        }
        "Read" | "Write" | "Edit" => {
            let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            path.to_string()
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            format!("/{pattern}/")
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            pattern.to_string()
        }
        _ => serde_json::to_string(&input).unwrap_or_default(),
    }
}

fn parse_user(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
) -> Vec<LogEntry> {
    let content = &v["message"]["content"];

    match content {
        Value::String(s) => {
            if s.is_empty() {
                return vec![];
            }
            vec![LogEntry {
                timestamp: timestamp.to_string(),
                agent_name: agent_name.to_string(),
                agent_color: agent_color.map(String::from),
                message_type: EntryType::User,
                content: s.clone(),
                tool_name: None,
                is_error: false,
            }]
        }
        Value::Array(arr) => {
            let mut entries = Vec::new();
            for block in arr {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if block_type == "tool_result" {
                    let is_error = block
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    let result_content = extract_tool_result_content(block);
                    entries.push(LogEntry {
                        timestamp: timestamp.to_string(),
                        agent_name: agent_name.to_string(),
                        agent_color: agent_color.map(String::from),
                        message_type: EntryType::ToolResult,
                        content: result_content,
                        tool_name: None,
                        is_error,
                    });
                } else if block_type == "text" {
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !text.is_empty() {
                        entries.push(LogEntry {
                            timestamp: timestamp.to_string(),
                            agent_name: agent_name.to_string(),
                            agent_color: agent_color.map(String::from),
                            message_type: EntryType::User,
                            content: text.to_string(),
                            tool_name: None,
                            is_error: false,
                        });
                    }
                }
            }
            entries
        }
        _ => vec![],
    }
}

fn parse_system(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
) -> Vec<LogEntry> {
    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");

    if content.is_empty() && subtype.is_empty() {
        return vec![];
    }

    let display = if !content.is_empty() {
        content.to_string()
    } else {
        format!("[system:{subtype}]")
    };

    vec![LogEntry {
        timestamp: timestamp.to_string(),
        agent_name: agent_name.to_string(),
        agent_color: agent_color.map(String::from),
        message_type: EntryType::System,
        content: display,
        tool_name: None,
        is_error: false,
    }]
}

fn extract_tool_result_content(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    p.get("text").and_then(|t| t.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Team config
// ---------------------------------------------------------------------------

pub fn claude_home() -> anyhow::Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))
}

/// Convert a cwd path to the Claude project key format.
/// "/Users/hikae/ghq/github.com/Foo/bar" → "-Users-hikae-ghq-github-com-Foo-bar"
pub fn cwd_to_project_key(cwd: &str) -> String {
    cwd.replace(['/', '.'], "-")
}

/// List all team names found under ~/.claude/teams/
pub fn find_teams() -> Vec<String> {
    let Ok(claude) = claude_home() else {
        return vec![];
    };
    let teams_dir = claude.join("teams");
    let Ok(entries) = fs::read_dir(&teams_dir) else {
        return vec![];
    };
    entries
        .filter_map(|e| {
            let e = e.ok()?;
            if !e.file_type().ok()?.is_dir() {
                return None;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            // Only include teams that have a config.json
            let config_path = e.path().join("config.json");
            if config_path.is_file() {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Path to a team's config.json.
pub fn team_config_path(team_name: &str) -> anyhow::Result<PathBuf> {
    Ok(claude_home()?
        .join("teams")
        .join(team_name)
        .join("config.json"))
}

/// Load team config from ~/.claude/teams/{team_name}/config.json
pub fn load_team_config(team_name: &str) -> anyhow::Result<TeamConfig> {
    let path = claude_home()?
        .join("teams")
        .join(team_name)
        .join("config.json");
    let data = fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&data)?;

    let lead_session_id = v
        .get("leadSessionId")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    // Get cwd from the lead member or top-level
    let cwd = v
        .get("members")
        .and_then(|m| m.as_array())
        .and_then(|arr| arr.first())
        .and_then(|m| m.get("cwd"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let members: Vec<MemberInfo> = v
        .get("members")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    let pane_id = m
                        .get("tmuxPaneId")
                        .and_then(|p| p.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                    MemberInfo {
                        name: m
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        color: m
                            .get("color")
                            .and_then(|c| c.as_str())
                            .map(String::from),
                        is_active: m
                            .get("isActive")
                            .and_then(|a| a.as_bool())
                            .unwrap_or(false),
                        tmux_pane_id: pane_id,
                        agent_id: m
                            .get("agentId")
                            .and_then(|a| a.as_str())
                            .map(String::from),
                        model: m
                            .get("model")
                            .and_then(|m| m.as_str())
                            .map(String::from),
                        cwd: m
                            .get("cwd")
                            .and_then(|c| c.as_str())
                            .map(String::from),
                        backend_type: m
                            .get("backendType")
                            .and_then(|b| b.as_str())
                            .map(String::from),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(TeamConfig {
        team_name: team_name.to_string(),
        lead_session_id,
        cwd,
        members,
    })
}

/// Read agent name from a subagent's meta.json (description field).
pub fn read_subagent_name(meta_path: &std::path::Path) -> Option<String> {
    let data = fs::read_to_string(meta_path).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    v.get("description")
        .and_then(|d| d.as_str())
        .map(String::from)
}

/// Resolve the project log directory for a team.
pub fn project_log_dir(config: &TeamConfig) -> anyhow::Result<PathBuf> {
    let claude = claude_home()?;
    let project_key = cwd_to_project_key(&config.cwd);
    Ok(claude.join("projects").join(project_key))
}

/// Resolve a member's session ID via tmux pane.
/// Flow: tmuxPaneId -> shell PID -> child PID -> ~/.claude/sessions/{PID}.json -> sessionId
pub fn resolve_member_session_via_tmux(pane_id: &str) -> Option<String> {
    use std::process::Command;

    // Step 1: Get shell PID from tmux pane
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_pid}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let shell_pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if shell_pid.is_empty() {
        return None;
    }

    // Step 2: Find child process (Claude Code) of the shell
    let output = Command::new("pgrep")
        .args(["-P", &shell_pid])
        .output()
        .ok()?;
    let child_pid = if output.status.success() {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()?
            .trim()
            .to_string()
    } else {
        // If no child, try the shell PID itself (Claude might have exec'd)
        shell_pid.clone()
    };

    // Step 3: Read session file (single-line JSON, use BufReader for consistency)
    use std::io::{BufRead, BufReader};
    let claude = claude_home().ok()?;
    let session_file = claude.join("sessions").join(format!("{child_pid}.json"));
    let file = fs::File::open(&session_file).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let v: Value = serde_json::from_str(&first_line).ok()?;
    v.get("sessionId")
        .and_then(|s| s.as_str())
        .map(String::from)
}

/// Discover member session IDs by scanning JSONL files in the project directory.
/// Reads the first 2 lines of each JSONL to find teamName and agentName.
pub fn discover_member_sessions(
    project_dir: &std::path::Path,
    team_name: &str,
    lead_session_id: &str,
) -> Vec<(String, String)> {
    // Returns Vec<(session_id, agent_name)>
    let Ok(entries) = fs::read_dir(project_dir) else {
        return vec![];
    };

    let mut results = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl")
            && let Some((session_id, agent_name)) =
                identify_member_jsonl(&path, team_name, lead_session_id)
        {
            results.push((session_id, agent_name));
        }
    }

    results
}

/// Read the first few lines of a JSONL file to identify if it belongs to a team member.
/// Returns Some((sessionId, agentName)) if it matches.
fn identify_member_jsonl(
    path: &std::path::Path,
    team_name: &str,
    lead_session_id: &str,
) -> Option<(String, String)> {
    use std::io::{BufRead, BufReader};

    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut session_id = None;
    let mut found_team = false;
    let mut agent_name = None;

    for (i, line) in reader.lines().enumerate() {
        if i >= 5 {
            break; // Only check first 5 lines
        }
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if session_id.is_none()
            && let Some(sid) = v.get("sessionId").and_then(|s| s.as_str())
        {
            session_id = Some(sid.to_string());
        }

        if !found_team
            && let Some(tn) = v.get("teamName").and_then(|t| t.as_str())
            && tn == team_name
        {
            found_team = true;
            agent_name = v
                .get("agentName")
                .and_then(|a| a.as_str())
                .map(String::from);
        }

        if session_id.is_some() && found_team {
            break;
        }
    }

    let sid = session_id?;
    if !found_team || sid == lead_session_id {
        return None;
    }
    // agent_name defaults to "unknown" if not found
    Some((sid, agent_name.unwrap_or_else(|| "unknown".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_user_message() {
        let line = r#"{"type":"user","timestamp":"2026-04-12T08:46:49.067Z","message":{"role":"user","content":"hello"}}"#;
        let entries = parse_line(line, "test-agent", Some("blue"));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::User);
        assert_eq!(entries[0].content, "hello");
        assert_eq!(entries[0].agent_color.as_deref(), Some("blue"));
    }

    #[test]
    fn parse_assistant_with_text_and_tool_use() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-10T07:01:55.588Z","message":{"role":"assistant","content":[{"type":"text","text":"Let me check."},{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"ls"}}]}}"#;
        let entries = parse_line(line, "agent-a", None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message_type, EntryType::Assistant);
        assert_eq!(entries[0].content, "Let me check.");
        assert_eq!(entries[1].message_type, EntryType::ToolUse);
        assert_eq!(entries[1].tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn parse_tool_result_in_user_message() {
        let line = r#"{"type":"user","timestamp":"2026-04-12T08:46:53.109Z","message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":"file1.txt\nfile2.txt","is_error":false}]}}"#;
        let entries = parse_line(line, "agent-b", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::ToolResult);
        assert!(!entries[0].is_error);
    }

    #[test]
    fn parse_system_message() {
        let line = r#"{"type":"system","subtype":"bridge_status","content":"connected","timestamp":"2026-04-12T08:46:35.621Z"}"#;
        let entries = parse_line(line, "sys", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::System);
        assert_eq!(entries[0].content, "connected");
    }

    #[test]
    fn skip_metadata_types() {
        let line = r#"{"type":"permission-mode","permissionMode":"default"}"#;
        assert!(parse_line(line, "x", None).is_empty());
    }

    #[test]
    fn skip_thinking_blocks() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"sig"},{"type":"text","text":"answer"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Assistant);
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json", "a", None).is_empty());
        assert!(parse_line("", "a", None).is_empty());
    }

    #[test]
    fn tool_result_error_flag() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"command not found","is_error":true}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_error);
    }

    #[test]
    fn format_timestamp_extracts_time() {
        assert_eq!(format_timestamp("2026-04-12T12:57:14.123Z"), "12:57:14");
        assert_eq!(format_timestamp("short"), "short");
    }

    #[test]
    fn cwd_to_project_key_converts_path() {
        assert_eq!(
            cwd_to_project_key("/Users/hikae/ghq/github.com/Foo/bar"),
            "-Users-hikae-ghq-github-com-Foo-bar"
        );
    }

    #[test]
    fn tool_use_send_message_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"SendMessage","input":{"to":"team-lead","summary":"task done","message":"completed"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "→ team-lead: task done");
    }

    #[test]
    fn find_teams_skips_dirs_without_config() {
        // find_teams should not crash on dirs without config.json (like "default")
        let teams = find_teams();
        assert!(!teams.contains(&"default".to_string()));
    }

    #[test]
    fn identify_member_jsonl_matches_team_member() {
        let dir = std::env::temp_dir().join("cc-test-identify-member");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let jsonl_path = dir.join("abc123.jsonl");
        let content = r#"{"type":"permission-mode","permissionMode":"default","sessionId":"abc123"}
{"teamName":"my-team","agentName":"backend-dev","type":"user","message":{"role":"user","content":"hello"},"timestamp":"2026-04-12T13:00:00Z"}
"#;
        fs::write(&jsonl_path, content).unwrap();

        let result = identify_member_jsonl(&jsonl_path, "my-team", "lead-session-id");
        assert_eq!(result, Some(("abc123".to_string(), "backend-dev".to_string())));

        // Lead session should be excluded
        let result = identify_member_jsonl(&jsonl_path, "my-team", "abc123");
        assert_eq!(result, None);

        // Different team should not match
        let result = identify_member_jsonl(&jsonl_path, "other-team", "lead-session-id");
        assert_eq!(result, None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_member_sessions_finds_team_files() {
        let dir = std::env::temp_dir().join("cc-test-discover-members");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Member JSONL
        let member_content = r#"{"type":"permission-mode","permissionMode":"default","sessionId":"member-sess-1"}
{"teamName":"test-team","agentName":"worker-a","type":"user","message":{"role":"user","content":"hi"},"timestamp":"T"}
"#;
        fs::write(dir.join("member-sess-1.jsonl"), member_content).unwrap();

        // Lead JSONL (should be excluded)
        let lead_content = r#"{"type":"permission-mode","permissionMode":"default","sessionId":"lead-sess"}
{"teamName":"test-team","agentName":"team-lead","type":"user","message":{"role":"user","content":"hi"},"timestamp":"T"}
"#;
        fs::write(dir.join("lead-sess.jsonl"), lead_content).unwrap();

        // Unrelated JSONL (different team)
        let other_content = r#"{"type":"permission-mode","permissionMode":"default","sessionId":"other-sess"}
{"teamName":"other-team","agentName":"x","type":"user","message":{"role":"user","content":"hi"},"timestamp":"T"}
"#;
        fs::write(dir.join("other-sess.jsonl"), other_content).unwrap();

        let results = discover_member_sessions(&dir, "test-team", "lead-sess");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "member-sess-1");
        assert_eq!(results[0].1, "worker-a");

        let _ = fs::remove_dir_all(&dir);
    }
}
