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
    /// True when the underlying JSONL entry had `"isSidechain": true`
    /// (e.g. subagent/Task-tool output emitted inline in the parent log).
    /// Defaults to false for regular conversation entries.
    #[serde(default)]
    pub is_sidechain: bool,
    /// Per-record unique identifier from Claude Code JSONL. Used for
    /// deduplication when stream-json input duplicates entries.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub uuid: Option<String>,
    /// Parent UUID for threading across records.
    #[serde(
        rename = "parentUuid",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub parent_uuid: Option<String>,
    /// Model that produced the assistant response (only populated for
    /// assistant records).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Stop reason emitted by the model (`end_turn`, `tool_use`, etc.).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stop_reason: Option<String>,
    /// Usage statistics (input/output/cache tokens) for assistant messages.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<Usage>,
}

/// Token-usage metadata carried on assistant records.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(
        rename = "cache_creation_input_tokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(
        rename = "cache_read_input_tokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_read_input_tokens: Option<u64>,
}

/// Metadata shared by every LogEntry emitted from a single JSONL record.
#[derive(Debug, Clone, Default)]
struct RecordMeta {
    is_sidechain: bool,
    uuid: Option<String>,
    parent_uuid: Option<String>,
    model: Option<String>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
}

impl LogEntry {
    /// Construct a new LogEntry. `is_sidechain` defaults to false; use
    /// [`LogEntry::with_sidechain`] to flip it after construction.
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
            is_sidechain: false,
            uuid: None,
            parent_uuid: None,
            model: None,
            stop_reason: None,
            usage: None,
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

    fn with_meta(mut self, meta: &RecordMeta) -> Self {
        self.is_sidechain = meta.is_sidechain;
        self.uuid = meta.uuid.clone();
        self.parent_uuid = meta.parent_uuid.clone();
        self.model = meta.model.clone();
        self.stop_reason = meta.stop_reason.clone();
        self.usage = meta.usage.clone();
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

    let meta = extract_record_meta(&v);

    match top_type {
        "assistant" => parse_assistant(&v, &timestamp, agent_name, agent_color, &meta),
        "user" => parse_user(&v, &timestamp, agent_name, agent_color, &meta),
        "system" => parse_system(&v, &timestamp, agent_name, agent_color, &meta),
        "summary" => parse_summary(&v, &timestamp, agent_name, agent_color, &meta),
        "result" => parse_result(&v, &timestamp, agent_name, agent_color, &meta),
        "file-history-snapshot" => parse_snapshot(&v, &timestamp, agent_name, agent_color, &meta),
        _ => vec![],
    }
}

/// Extract the per-record metadata (uuid, parent, model, stop_reason, usage,
/// is_sidechain) once up front so every LogEntry emitted from this line can
/// share the same metadata values.
fn extract_record_meta(v: &Value) -> RecordMeta {
    // Claude Code marks subagent (Task-tool) entries inline in the parent
    // JSONL with a top-level `isSidechain: true` flag. We propagate this to
    // every LogEntry produced so downstream filters / renderers can treat
    // sidechain traffic distinctly.
    let is_sidechain = v
        .get("isSidechain")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

    let uuid = v.get("uuid").and_then(|s| s.as_str()).map(String::from);
    let parent_uuid = v
        .get("parentUuid")
        .and_then(|s| s.as_str())
        .map(String::from);
    let model = v
        .pointer("/message/model")
        .and_then(|s| s.as_str())
        .map(String::from);
    let stop_reason = v
        .pointer("/message/stop_reason")
        .and_then(|s| s.as_str())
        .map(String::from);
    let usage = v.pointer("/message/usage").and_then(extract_usage);

    RecordMeta {
        is_sidechain,
        uuid,
        parent_uuid,
        model,
        stop_reason,
        usage,
    }
}

/// Parse a `/message/usage` object into a `Usage` struct. Returns None if
/// every field is missing so we don't emit an empty usage blob.
fn extract_usage(v: &Value) -> Option<Usage> {
    let obj = v.as_object()?;
    let input_tokens = obj.get("input_tokens").and_then(|n| n.as_u64());
    let output_tokens = obj.get("output_tokens").and_then(|n| n.as_u64());
    let cache_creation_input_tokens = obj
        .get("cache_creation_input_tokens")
        .and_then(|n| n.as_u64());
    let cache_read_input_tokens = obj.get("cache_read_input_tokens").and_then(|n| n.as_u64());
    if input_tokens.is_none()
        && output_tokens.is_none()
        && cache_creation_input_tokens.is_none()
        && cache_read_input_tokens.is_none()
    {
        return None;
    }
    Some(Usage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens,
        cache_read_input_tokens,
    })
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
    meta: &RecordMeta,
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
                    entries.push(
                        LogEntry::new(
                            timestamp,
                            agent_name,
                            agent_color,
                            EntryType::Assistant,
                            text.to_string(),
                        )
                        .with_meta(meta),
                    );
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
                    .with_tool(name)
                    .with_meta(meta),
                );
            }
            "thinking" => {
                let thinking = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                if !thinking.is_empty() {
                    entries.push(
                        LogEntry::new(
                            timestamp,
                            agent_name,
                            agent_color,
                            EntryType::Thinking,
                            thinking.to_string(),
                        )
                        .with_meta(meta),
                    );
                }
            }
            "redacted_thinking" => {
                entries.push(
                    LogEntry::new(
                        timestamp,
                        agent_name,
                        agent_color,
                        EntryType::Thinking,
                        "[redacted thinking]".to_string(),
                    )
                    .with_meta(meta),
                );
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
    meta: &RecordMeta,
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
            vec![
                LogEntry::new(
                    timestamp,
                    agent_name,
                    agent_color,
                    EntryType::User,
                    s.clone(),
                )
                .with_meta(meta),
            ]
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
                        .with_error(is_error)
                        .with_meta(meta),
                    );
                } else if block_type == "text" {
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !text.is_empty() {
                        entries.push(
                            LogEntry::new(
                                timestamp,
                                agent_name,
                                agent_color,
                                EntryType::User,
                                text.to_string(),
                            )
                            .with_meta(meta),
                        );
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
    meta: &RecordMeta,
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

    vec![
        LogEntry::new(
            timestamp,
            agent_name,
            agent_color,
            EntryType::System,
            display,
        )
        .with_meta(meta),
    ]
}

fn parse_summary(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
    meta: &RecordMeta,
) -> Vec<LogEntry> {
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
    if summary.is_empty() {
        return vec![];
    }
    vec![
        LogEntry::new(
            timestamp,
            agent_name,
            agent_color,
            EntryType::Summary,
            summary.to_string(),
        )
        .with_meta(meta),
    ]
}

fn parse_result(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
    meta: &RecordMeta,
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
        .with_error(is_error)
        .with_meta(meta),
    ]
}

fn parse_snapshot(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
    meta: &RecordMeta,
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
    vec![
        LogEntry::new(
            timestamp,
            agent_name,
            agent_color,
            EntryType::Snapshot,
            content,
        )
        .with_meta(meta),
    ]
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

    #[test]
    fn parse_line_extracts_is_sidechain_true() {
        let line = r#"{"type":"user","isSidechain":true,"timestamp":"T","message":{"role":"user","content":"hello from subagent"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_sidechain);
    }

    #[test]
    fn parse_line_defaults_is_sidechain_false() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":"hello"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_sidechain);
    }

    #[test]
    fn is_sidechain_propagates_to_all_blocks() {
        let line = r#"{"type":"assistant","isSidechain":true,"timestamp":"T","message":{"role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_sidechain);
        assert!(entries[1].is_sidechain);
        assert_eq!(entries[0].message_type, EntryType::Assistant);
        assert_eq!(entries[1].message_type, EntryType::ToolUse);
    }

    #[test]
    fn is_sidechain_serialized_in_json() {
        let line = r#"{"type":"user","isSidechain":true,"timestamp":"T","message":{"role":"user","content":"x"}}"#;
        let entries = parse_line(line, "a", None);
        let json = serde_json::to_string(&entries[0]).unwrap();
        assert!(json.contains("\"is_sidechain\":true"));
    }

    #[test]
    fn parse_line_extracts_uuid_and_parent() {
        let line = r#"{"type":"user","uuid":"child-1","parentUuid":"parent-0","timestamp":"T","message":{"role":"user","content":"hi"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uuid.as_deref(), Some("child-1"));
        assert_eq!(entries[0].parent_uuid.as_deref(), Some("parent-0"));
    }

    #[test]
    fn parse_line_extracts_model_and_stop_reason() {
        let line = r#"{"type":"assistant","uuid":"a1","timestamp":"T","message":{"role":"assistant","model":"claude-sonnet-4-6","stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(entries[0].stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn parse_line_extracts_usage_with_cache_tokens() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","model":"claude","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":11,"output_tokens":22,"cache_creation_input_tokens":33,"cache_read_input_tokens":44}}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        let usage = entries[0].usage.as_ref().expect("usage expected");
        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.output_tokens, Some(22));
        assert_eq!(usage.cache_creation_input_tokens, Some(33));
        assert_eq!(usage.cache_read_input_tokens, Some(44));
    }

    #[test]
    fn parse_line_uuid_absent_is_none() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":"hi"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].uuid.is_none());
        assert!(entries[0].parent_uuid.is_none());
        assert!(entries[0].model.is_none());
        assert!(entries[0].stop_reason.is_none());
        assert!(entries[0].usage.is_none());
    }

    #[test]
    fn metadata_shared_across_all_blocks_from_same_line() {
        let line = r#"{"type":"assistant","uuid":"same-uuid","parentUuid":"parent","timestamp":"T","message":{"role":"assistant","model":"claude-sonnet-4-6","stop_reason":"tool_use","content":[{"type":"text","text":"thinking out loud"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}},{"type":"thinking","thinking":"hmm","signature":"s"}],"usage":{"input_tokens":1,"output_tokens":2}}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 3);
        for entry in &entries {
            assert_eq!(entry.uuid.as_deref(), Some("same-uuid"));
            assert_eq!(entry.parent_uuid.as_deref(), Some("parent"));
            assert_eq!(entry.model.as_deref(), Some("claude-sonnet-4-6"));
            assert_eq!(entry.stop_reason.as_deref(), Some("tool_use"));
            let usage = entry.usage.as_ref().expect("usage shared");
            assert_eq!(usage.input_tokens, Some(1));
            assert_eq!(usage.output_tokens, Some(2));
        }
    }

    #[test]
    fn parent_uuid_serialized_as_camel_case() {
        let line = r#"{"type":"user","uuid":"u1","parentUuid":"p0","timestamp":"T","message":{"role":"user","content":"x"}}"#;
        let entries = parse_line(line, "a", None);
        let json = serde_json::to_string(&entries[0]).unwrap();
        assert!(
            json.contains("\"parentUuid\":\"p0\""),
            "parentUuid should serialise in camelCase: {json}"
        );
    }

    #[test]
    fn usage_cache_tokens_serialize_with_original_names() {
        let u = Usage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_creation_input_tokens: Some(30),
            cache_read_input_tokens: Some(40),
        };
        let json = serde_json::to_string(&u).unwrap();
        assert!(json.contains("\"cache_creation_input_tokens\":30"));
        assert!(json.contains("\"cache_read_input_tokens\":40"));
    }
}
