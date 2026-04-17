use serde::Serialize;
use serde_json::Value;

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

impl LogEntry {
    fn new(
        timestamp: &str,
        agent_name: &str,
        agent_color: Option<&str>,
        message_type: EntryType,
        content: String,
    ) -> Self {
        Self {
            timestamp: timestamp.to_string(),
            agent_name: agent_name.to_string(),
            agent_color: agent_color.map(String::from),
            message_type,
            content,
            tool_name: None,
            is_error: false,
        }
    }

    fn with_tool(mut self, name: &str) -> Self {
        self.tool_name = Some(name.to_string());
        self
    }

    fn with_error(mut self, is_error: bool) -> Self {
        self.is_error = is_error;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum EntryType {
    Assistant,
    User,
    System,
    ToolUse,
    ToolResult,
    Thinking,
    Summary,
    Result,
    Snapshot,
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
        "summary" => parse_summary(&v, &timestamp, agent_name, agent_color),
        "result" => parse_result(&v, &timestamp, agent_name, agent_color),
        "file-history-snapshot" => parse_snapshot(&v, &timestamp, agent_name, agent_color),
        _ => vec![],
    }
}

/// Truncate a string to at most `max` characters (not bytes).
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
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
                    entries.push(LogEntry::new(
                        timestamp,
                        agent_name,
                        agent_color,
                        EntryType::Assistant,
                        text.to_string(),
                    ));
                }
            }
            "tool_use" => {
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown");
                let content = extract_tool_use_summary(name, block);
                entries.push(
                    LogEntry::new(
                        timestamp,
                        agent_name,
                        agent_color,
                        EntryType::ToolUse,
                        content,
                    )
                    .with_tool(name),
                );
            }
            "thinking" => {
                let thinking = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                if !thinking.is_empty() {
                    entries.push(LogEntry::new(
                        timestamp,
                        agent_name,
                        agent_color,
                        EntryType::Thinking,
                        thinking.to_string(),
                    ));
                }
            }
            "redacted_thinking" => {
                entries.push(LogEntry::new(
                    timestamp,
                    agent_name,
                    agent_color,
                    EntryType::Thinking,
                    "[redacted thinking]".to_string(),
                ));
            }
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
                .or_else(|| input.get("message").and_then(|m| m.as_str()))
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
            truncate_chars(cmd, 80)
        }
        "Read" | "Write" | "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
        "Task" => {
            let subagent_type = input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let description = input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !subagent_type.is_empty() && !description.is_empty() {
                format!("{subagent_type}: {description}")
            } else if !subagent_type.is_empty() {
                let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                format!("{subagent_type}: {}", truncate_chars(prompt, 60))
            } else {
                let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                truncate_chars(prompt, 60)
            }
        }
        "TodoWrite" => {
            let todos = input.get("todos").and_then(|v| v.as_array());
            match todos {
                Some(arr) if arr.is_empty() => "todos cleared".to_string(),
                Some(arr) => {
                    let n = arr.len();
                    let in_progress = arr
                        .iter()
                        .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
                        .count();
                    format!("{n} todos ({in_progress} in progress)")
                }
                None => "todos cleared".to_string(),
            }
        }
        "WebSearch" => {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            query.to_string()
        }
        "WebFetch" => {
            let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
            url.to_string()
        }
        "NotebookEdit" => {
            let path = input
                .get("notebook_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cell_id = input.get("cell_id").and_then(|v| v.as_str());
            match cell_id {
                Some(id) if !id.is_empty() => format!("{path} [{id}]"),
                _ => path.to_string(),
            }
        }
        "MultiEdit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let n = input
                .get("edits")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{path} ({n} edits)")
        }
        "ExitPlanMode" => {
            let plan = input.get("plan").and_then(|v| v.as_str()).unwrap_or("");
            format!("plan: {}", truncate_chars(plan, 60))
        }
        "Skill" => {
            let skill = input.get("skill").and_then(|v| v.as_str()).unwrap_or("");
            let args = input.get("args").and_then(|v| v.as_str()).unwrap_or("");
            if args.is_empty() {
                skill.to_string()
            } else {
                format!("{skill} {args}")
            }
        }
        "AskUserQuestion" => {
            let question = input
                .get("questions")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|q| q.get("question"))
                .and_then(|q| q.as_str())
                .unwrap_or("");
            truncate_chars(question, 60)
        }
        name if name.starts_with("mcp__") => {
            let parts: Vec<&str> = name.splitn(3, "__").collect();
            let server = parts.get(1).copied().unwrap_or("");
            let tool = parts.get(2).copied().unwrap_or("");
            let compact = truncate_chars(&serde_json::to_string(&input).unwrap_or_default(), 80);
            format!("[{server}] {tool}: {compact}")
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
    let content = match v.pointer("/message/content") {
        Some(c) => c,
        None => return vec![],
    };

    match content {
        Value::String(s) => {
            if s.is_empty() {
                return vec![];
            }
            vec![LogEntry::new(
                timestamp,
                agent_name,
                agent_color,
                EntryType::User,
                s.clone(),
            )]
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
                    entries.push(
                        LogEntry::new(
                            timestamp,
                            agent_name,
                            agent_color,
                            EntryType::ToolResult,
                            result_content,
                        )
                        .with_error(is_error),
                    );
                } else if block_type == "text" {
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !text.is_empty() {
                        entries.push(LogEntry::new(
                            timestamp,
                            agent_name,
                            agent_color,
                            EntryType::User,
                            text.to_string(),
                        ));
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

    vec![LogEntry::new(
        timestamp,
        agent_name,
        agent_color,
        EntryType::System,
        display,
    )]
}

fn parse_summary(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
) -> Vec<LogEntry> {
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
    if summary.is_empty() {
        return vec![];
    }
    vec![LogEntry::new(
        timestamp,
        agent_name,
        agent_color,
        EntryType::Summary,
        summary.to_string(),
    )]
}

fn parse_result(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
) -> Vec<LogEntry> {
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
    let num_turns = v.get("num_turns").and_then(|n| n.as_u64());
    let total_cost = v.get("total_cost_usd").and_then(|c| c.as_f64());
    let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);

    let content = if !subtype.is_empty() || num_turns.is_some() || total_cost.is_some() {
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = num_turns {
            parts.push(format!("turns={n}"));
        }
        if let Some(c) = total_cost {
            parts.push(format!("cost=${c:.2}"));
        }
        if parts.is_empty() {
            format!("session result: {subtype}")
        } else {
            format!("session result: {subtype} ({})", parts.join(", "))
        }
    } else {
        v.get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string()
    };

    if content.is_empty() {
        return vec![];
    }

    vec![
        LogEntry::new(
            timestamp,
            agent_name,
            agent_color,
            EntryType::Result,
            content,
        )
        .with_error(is_error),
    ]
}

fn parse_snapshot(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
) -> Vec<LogEntry> {
    let is_update = v
        .get("isSnapshotUpdate")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let content = if is_update {
        "[git snapshot] (update)".to_string()
    } else {
        "[git snapshot]".to_string()
    };
    vec![LogEntry::new(
        timestamp,
        agent_name,
        agent_color,
        EntryType::Snapshot,
        content,
    )]
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
    fn parse_thinking_block() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"sig"},{"type":"text","text":"answer"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message_type, EntryType::Thinking);
        assert_eq!(entries[0].content, "hmm");
        assert_eq!(entries[1].message_type, EntryType::Assistant);
        assert_eq!(entries[1].content, "answer");
    }

    #[test]
    fn parse_redacted_thinking_block() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"redacted_thinking","data":"xyz"},{"type":"text","text":"answer"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message_type, EntryType::Thinking);
        assert_eq!(entries[0].content, "[redacted thinking]");
        assert_eq!(entries[1].message_type, EntryType::Assistant);
    }

    #[test]
    fn parse_summary_message() {
        let line = r#"{"type":"summary","summary":"Conversation compacted","leafUuid":"abc","timestamp":"2026-04-12T08:46:49.067Z"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Summary);
        assert_eq!(entries[0].content, "Conversation compacted");
    }

    #[test]
    fn parse_summary_missing_timestamp() {
        let line = r#"{"type":"summary","summary":"S","leafUuid":"abc"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, "");
    }

    #[test]
    fn parse_result_message() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.1234,"duration_ms":1200,"num_turns":5,"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Result);
        assert!(entries[0].content.contains("session result: success"));
        assert!(entries[0].content.contains("turns=5"));
        assert!(entries[0].content.contains("cost=$0.12"));
    }

    #[test]
    fn parse_result_fallback_to_text() {
        let line = r#"{"type":"result","result":"done","timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Result);
        assert_eq!(entries[0].content, "done");
    }

    #[test]
    fn parse_snapshot_message() {
        let line =
            r#"{"type":"file-history-snapshot","messageId":"m","snapshot":{},"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Snapshot);
        assert_eq!(entries[0].content, "[git snapshot]");
    }

    #[test]
    fn parse_snapshot_update() {
        let line = r#"{"type":"file-history-snapshot","messageId":"m","isSnapshotUpdate":true,"snapshot":{},"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Snapshot);
        assert!(entries[0].content.contains("update"));
    }

    #[test]
    fn tool_use_task_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Task","input":{"subagent_type":"explore","description":"Look at parser","prompt":"ignored"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "explore: Look at parser");
    }

    #[test]
    fn tool_use_todo_write_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"status":"in_progress"},{"status":"pending"},{"status":"completed"}]}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "3 todos (1 in progress)");
    }

    #[test]
    fn tool_use_todo_write_empty() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[]}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "todos cleared");
    }

    #[test]
    fn tool_use_web_search_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"WebSearch","input":{"query":"rust async traits"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "rust async traits");
    }

    #[test]
    fn tool_use_mcp_namespace() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"mcp__github__get_me","input":{"x":1}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.starts_with("[github] get_me: "));
    }

    #[test]
    fn tool_use_multi_edit_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"MultiEdit","input":{"file_path":"/a/b.rs","edits":[{},{}]}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "/a/b.rs (2 edits)");
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
    fn tool_use_send_message_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"SendMessage","input":{"to":"team-lead","summary":"task done","message":"completed"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "→ team-lead: task done");
    }
}
