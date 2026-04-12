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
    fn tool_use_send_message_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"SendMessage","input":{"to":"team-lead","summary":"task done","message":"completed"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "→ team-lead: task done");
    }
}
