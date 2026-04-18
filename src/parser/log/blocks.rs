use serde_json::Value;

use super::meta::RecordMeta;
use super::tags::detect_first_tag;
use super::tools::extract_tool_use_summary;
use super::types::{EntryType, LogEntry, truncate_chars};

pub(crate) fn parse_assistant(
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
                    let tag = detect_first_tag(text).map(|(k, _)| k);
                    entries.push(
                        LogEntry::new(
                            timestamp,
                            agent_name,
                            agent_color,
                            EntryType::Assistant,
                            text.to_string(),
                        )
                        .with_meta(meta)
                        .with_tag(tag),
                    );
                }
            }
            // `tool_use` and the server-hosted `server_tool_use` /
            // `mcp_tool_use` / `computer_use` variants all carry the same
            // shape: a tool name plus an input object. We normalise them to
            // a single ToolUse entry so the renderer doesn't have to know
            // about the hosting distinction. `computer_use` blocks from
            // Claude's desktop-control tool don't always carry a `name`
            // field, so fall back to `"Computer"` in that case.
            "tool_use" | "server_tool_use" | "mcp_tool_use" | "computer_use" => {
                let fallback = if block_type == "computer_use" {
                    "Computer"
                } else {
                    "unknown"
                };
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or(fallback);
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
            // Citation annotations emitted alongside assistant text for
            // grounded responses (web_search / file-based citations).
            // Render as a single User-facing line so a reader can see the
            // source without tying it to a specific text block index.
            "citation" | "citations_delta" => {
                let content = extract_citation(block);
                if !content.is_empty() {
                    entries.push(
                        LogEntry::new(
                            timestamp,
                            agent_name,
                            agent_color,
                            EntryType::Assistant,
                            content,
                        )
                        .with_meta(meta),
                    );
                }
            }
            // Server-hosted WebSearch / MCP results arrive as a result block
            // inline in the assistant message. Render as ToolResult so
            // downstream filters (type=tool_result) still catch them.
            "web_search_tool_result" | "mcp_tool_result" => {
                let is_error = block
                    .get("is_error")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                let content = extract_server_tool_result(block);
                entries.push(
                    LogEntry::new(
                        timestamp,
                        agent_name,
                        agent_color,
                        EntryType::ToolResult,
                        content,
                    )
                    .with_error(is_error)
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

pub(crate) fn parse_user(
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
            let tag = detect_first_tag(s).map(|(k, _)| k);
            vec![
                LogEntry::new(
                    timestamp,
                    agent_name,
                    agent_color,
                    EntryType::User,
                    s.clone(),
                )
                .with_meta(meta)
                .with_tag(tag),
            ]
        }
        Value::Array(arr) => {
            let mut entries = Vec::new();
            for block in arr {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "tool_result" | "mcp_tool_result" => {
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
                    }
                    "text" => {
                        let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            let tag = detect_first_tag(text).map(|(k, _)| k);
                            entries.push(
                                LogEntry::new(
                                    timestamp,
                                    agent_name,
                                    agent_color,
                                    EntryType::User,
                                    text.to_string(),
                                )
                                .with_meta(meta)
                                .with_tag(tag),
                            );
                        }
                    }
                    "image" => {
                        let media = block
                            .pointer("/source/media_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("image");
                        entries.push(
                            LogEntry::new(
                                timestamp,
                                agent_name,
                                agent_color,
                                EntryType::User,
                                format!("[image: {media}]"),
                            )
                            .with_meta(meta),
                        );
                    }
                    "document" => {
                        // Prefer filename, then media_type, else a generic label.
                        let filename = block
                            .get("filename")
                            .or_else(|| block.pointer("/source/filename"))
                            .and_then(|v| v.as_str());
                        let label = filename
                            .or_else(|| {
                                block.pointer("/source/media_type").and_then(|v| v.as_str())
                            })
                            .unwrap_or("document");
                        entries.push(
                            LogEntry::new(
                                timestamp,
                                agent_name,
                                agent_color,
                                EntryType::User,
                                format!("[document: {label}]"),
                            )
                            .with_meta(meta),
                        );
                    }
                    _ => {}
                }
            }
            entries
        }
        _ => vec![],
    }
}

pub(crate) fn parse_system(
    v: &Value,
    timestamp: &str,
    agent_name: &str,
    agent_color: Option<&str>,
    meta: &RecordMeta,
) -> Vec<LogEntry> {
    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");

    // `compact_boundary` marks the point at which auto-compaction fired.
    // Promote it to its own EntryType so renderers can draw a separator
    // (and so --type filters can target it independently).
    if subtype == "compact_boundary" {
        let trigger = v
            .pointer("/compactMetadata/trigger")
            .and_then(|t| t.as_str());
        // Newer Claude Code builds shortened the token field to `preTokens`
        // while older logs carry `preCompactTokens`. Accept either so the
        // separator stays informative across versions.
        let pre_tokens = v
            .pointer("/compactMetadata/preCompactTokens")
            .or_else(|| v.pointer("/compactMetadata/preTokens"))
            .and_then(|n| n.as_u64());
        let display = match (trigger, pre_tokens) {
            (Some(t), Some(n)) => format!("compact boundary: trigger={t}, pre_tokens={n}"),
            (Some(t), None) => format!("compact boundary: trigger={t}"),
            _ => "compact boundary".to_string(),
        };
        return vec![
            LogEntry::new(
                timestamp,
                agent_name,
                agent_color,
                EntryType::CompactBoundary,
                display,
            )
            .with_meta(meta),
        ];
    }

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

pub(crate) fn parse_summary(
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

pub(crate) fn parse_result(
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
            // Claude API costs can be sub-cent; format with enough precision to
            // preserve small values, then strip trailing zeros so common values
            // still render compactly (e.g. `$0.12`, not `$0.120000`).
            let raw = format!("{c:.6}");
            let trimmed = raw.trim_end_matches('0').trim_end_matches('.');
            parts.push(format!("cost=${trimmed}"));
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

pub(crate) fn parse_snapshot(
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

/// Pull a human-readable label from a `citation` / `citations_delta` block.
/// Prefers `cited_text` (quoted span) + a source URL/title; falls back to any
/// URL or title alone so we always surface something for the reader.
fn extract_citation(block: &Value) -> String {
    let cited = block
        .get("cited_text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Citation blocks nest their origin under `source` (URL citations) or
    // carry `title` / `url` at the top level (older shape). Support both.
    let source = block.get("source").unwrap_or(block);
    let url = source.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let title = source.get("title").and_then(|v| v.as_str()).unwrap_or("");

    let origin = match (title, url) {
        ("", "") => String::new(),
        ("", u) => u.to_string(),
        (t, "") => t.to_string(),
        (t, u) => format!("{t} ({u})"),
    };

    let cited = truncate_chars(cited, 120);
    match (cited.as_str(), origin.as_str()) {
        ("", "") => String::new(),
        ("", o) => format!("[citation] {o}"),
        (c, "") => format!("[citation] \u{201c}{c}\u{201d}"),
        (c, o) => format!("[citation] \u{201c}{c}\u{201d} — {o}"),
    }
}

/// Pull a human-readable summary from a server-hosted tool result block.
///
/// WebSearch results arrive as `content[{type: "web_search_result", ...}]`;
/// MCP results often reuse the standard `content[{type: "text", ...}]`
/// shape. Fall back to compact JSON when we don't recognise the payload so
/// the viewer still shows something.
fn extract_server_tool_result(block: &Value) -> String {
    if let Some(content) = block.get("content") {
        match content {
            Value::String(s) => return s.clone(),
            Value::Array(parts) => {
                let mut lines = Vec::new();
                for p in parts {
                    match p.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                lines.push(t.to_string());
                            }
                        }
                        Some("web_search_result") => {
                            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            let url = p.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            lines.push(format!("{title} — {url}"));
                        }
                        _ => {}
                    }
                }
                if !lines.is_empty() {
                    return lines.join("\n");
                }
            }
            _ => {}
        }
    }
    serde_json::to_string(block).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::parse_line;
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_system_compact_boundary_is_distinct_entry() {
        let line = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compactMetadata": {
                "trigger": "auto",
                "preCompactTokens": 50000
            },
            "timestamp": "T"
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::CompactBoundary);
        assert!(entries[0].content.contains("trigger=auto"));
        assert!(entries[0].content.contains("pre_tokens=50000"));
    }

    #[test]
    fn parse_system_compact_boundary_without_metadata() {
        let line = json!({ "type": "system", "subtype": "compact_boundary", "timestamp": "T" })
            .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::CompactBoundary);
        assert_eq!(entries[0].content, "compact boundary");
    }

    #[test]
    fn assistant_server_tool_use_surfaces_as_toolcall() {
        let line = json!({
            "type": "assistant",
            "timestamp": "T",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "server_tool_use", "id": "s1", "name": "web_search", "input": { "query": "rust tokio" } }
                ]
            }
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::ToolUse);
        assert_eq!(entries[0].tool_name.as_deref(), Some("web_search"));
        assert_eq!(entries[0].content, "rust tokio");
    }

    #[test]
    fn assistant_web_search_result_surfaces_as_toolresult() {
        let line = json!({
            "type": "assistant",
            "timestamp": "T",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "web_search_tool_result",
                    "tool_use_id": "s1",
                    "content": [
                        { "type": "web_search_result", "title": "Tokio", "url": "https://tokio.rs" }
                    ]
                }]
            }
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::ToolResult);
        assert!(entries[0].content.contains("Tokio"));
        assert!(entries[0].content.contains("https://tokio.rs"));
    }

    #[test]
    fn compact_boundary_accepts_pre_tokens_alias() {
        // Newer Claude Code payloads shorten the field name from
        // `preCompactTokens` to `preTokens`. Both must produce the same
        // human-readable separator so the boundary stays informative.
        let line = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compactMetadata": { "trigger": "manual", "preTokens": 12345 },
            "timestamp": "T"
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("trigger=manual"));
        assert!(entries[0].content.contains("pre_tokens=12345"));
    }

    #[test]
    fn assistant_computer_use_block_surfaces_as_toolcall() {
        // Claude's desktop-control tool emits `computer_use` blocks. The
        // block may omit `name`, in which case we default to "Computer".
        let line = json!({
            "type": "assistant",
            "timestamp": "T",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "computer_use", "input": { "action": "screenshot" } }
                ]
            }
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::ToolUse);
        assert_eq!(entries[0].tool_name.as_deref(), Some("Computer"));
    }

    #[test]
    fn assistant_citation_block_renders_origin() {
        let line = json!({
            "type": "assistant",
            "timestamp": "T",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "citation",
                    "cited_text": "Tokio is an async runtime",
                    "source": { "title": "Tokio", "url": "https://tokio.rs" }
                }]
            }
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.starts_with("[citation]"));
        assert!(entries[0].content.contains("Tokio"));
        assert!(entries[0].content.contains("https://tokio.rs"));
    }

    #[test]
    fn compact_summary_flag_propagates_to_entries() {
        let line = json!({
            "type": "user",
            "isCompactSummary": true,
            "logicalParentUuid": "lp-7",
            "timestamp": "T",
            "message": { "role": "user", "content": "[summarised prior turns]" }
        })
        .to_string();
        let entries = parse_line(&line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_compact_summary);
        assert_eq!(entries[0].logical_parent_uuid.as_deref(), Some("lp-7"));
    }

    #[test]
    fn assistant_mcp_tool_use_and_result_are_handled() {
        let use_line = json!({
            "type": "assistant",
            "timestamp": "T",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "mcp_tool_use", "id": "m1", "name": "mcp__github__get_me", "input": {} }
                ]
            }
        })
        .to_string();
        let uses = parse_line(&use_line, "a", None);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].message_type, EntryType::ToolUse);

        let result_line = json!({
            "type": "assistant",
            "timestamp": "T",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "mcp_tool_result", "tool_use_id": "m1", "content": "ok" }
                ]
            }
        })
        .to_string();
        let results = parse_line(&result_line, "a", None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message_type, EntryType::ToolResult);
    }
}
