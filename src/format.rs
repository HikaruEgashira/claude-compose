use std::io::IsTerminal;

use crossterm::style::{Color, Stylize};

use crate::parser::{EntryType, LogEntry, format_timestamp};

/// Format a LogEntry for terminal output.
pub fn format_entry(
    entry: &LogEntry,
    verbose: bool,
    no_color: bool,
    max_name_width: usize,
) -> String {
    let no_color =
        no_color || !std::io::stdout().is_terminal() || std::env::var("NO_COLOR").is_ok();

    let ts = format_timestamp(&entry.timestamp);
    let timestamp = format!("[{ts}]");
    let name = format!("{:<width$}", entry.agent_name, width = max_name_width);

    let content = format_content(entry, verbose, no_color);

    // Use a branch-style separator for sidechain (subagent/Task-tool) entries
    // so they are visually indented from the main conversation stream.
    let (sep_color, sep_no_color) = if entry.is_sidechain {
        ("╰─", ">-")
    } else {
        ("│", "|")
    };

    if no_color {
        return format!("{timestamp} {name}{sep_no_color} {content}");
    }

    let color = resolve_color(entry.agent_color.as_deref());

    let styled_name = name.with(color).bold();
    let styled_content = if entry.is_error {
        content.with(Color::Red).to_string()
    } else {
        render_inline_bold(&content)
    };

    format!("{timestamp} {styled_name}{sep_color} {styled_content}")
}

fn format_content(entry: &LogEntry, verbose: bool, no_color: bool) -> String {
    match entry.message_type {
        EntryType::ToolUse => {
            let tool = entry.tool_name.as_deref().unwrap_or("unknown");
            let icon = tool_icon(tool, &entry.content, no_color);
            if matches!(tool, "SendMessage" | "TaskCreate")
                || (tool == "TaskUpdate" && entry.content.contains("completed"))
            {
                format!("{icon} {}", entry.content)
            } else {
                format!("{icon} {tool}: {}", entry.content)
            }
        }
        EntryType::ToolResult => {
            if verbose {
                entry.content.clone()
            } else {
                truncate_lines(&entry.content, 3)
            }
        }
        EntryType::Thinking => {
            if no_color {
                format!("[thinking] {}", entry.content)
            } else {
                // Dim styled thinking prefix with a thought-bubble emoji.
                let body = format!("\u{1f4ad} {}", entry.content);
                body.as_str().dim().to_string()
            }
        }
        EntryType::Summary => {
            let prefix = if no_color {
                "[summary] "
            } else {
                "\u{2261} summary: "
            };
            format!("{prefix}{}", entry.content)
        }
        EntryType::Result => {
            let prefix = if no_color { "[result] " } else { "\u{23f9} " };
            if entry.is_error {
                let icon = if no_color { "[err]" } else { "\u{f071}" };
                format!("{prefix}{icon} {}", entry.content)
            } else {
                format!("{prefix}{}", entry.content)
            }
        }
        EntryType::Snapshot => {
            let prefix = if no_color { "[snapshot] " } else { "\u{25f7} " };
            format!("{prefix}{}", entry.content)
        }
        _ => {
            if entry.is_error {
                let icon = if no_color { "[err]" } else { "\u{f071}" };
                format!("{icon} {}", entry.content)
            } else {
                entry.content.clone()
            }
        }
    }
}

/// Convert markdown `**bold**` to ANSI bold sequences.
fn render_inline_bold(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("**") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("**") {
            let bold_text = &after_open[..end];
            result.push_str(&bold_text.bold().to_string());
            rest = &after_open[end + 2..];
        } else {
            // No closing **, emit the rest as-is
            result.push_str(&rest[start..]);
            return result;
        }
    }
    result.push_str(rest);
    result
}

fn tool_icon(tool: &str, content: &str, no_color: bool) -> &'static str {
    if no_color {
        match tool {
            "SendMessage" => "[msg]",
            "TaskUpdate" if content.contains("completed") => "[ok]",
            "TaskCreate" => "[new]",
            "Task" => "[task]",
            "TodoWrite" => "[todo]",
            "WebSearch" | "WebFetch" => "[web]",
            "ExitPlanMode" => "[plan]",
            "Skill" => "[skill]",
            t if t.starts_with("mcp__") => "[mcp]",
            _ => "[tool]",
        }
    } else {
        match tool {
            "SendMessage" => "\u{f1d8}", // paper-plane
            "TaskUpdate" if content.contains("completed") => "\u{f00c}", // check
            "TaskCreate" => "\u{f0ca}",  // list
            "Task" => "\u{f03a}",        // list-alt
            "TodoWrite" => "\u{f14a}",   // check-square
            "WebSearch" | "WebFetch" => "\u{f0ac}", // globe
            "ExitPlanMode" => "\u{f024}", // flag
            "Skill" => "\u{f005}",       // star / sparkle
            t if t.starts_with("mcp__") => "\u{f1e6}", // plug
            _ => "\u{f0ad}",             // wrench
        }
    }
}

fn truncate_lines(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max_lines {
        return s.to_string();
    }
    let shown: Vec<&str> = lines[..max_lines].to_vec();
    let remaining = lines.len() - max_lines;
    format!("{}\n  ... ({remaining} more lines)", shown.join("\n"))
}

/// Truncate assistant text to the first `max_chars` characters.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}...")
}

/// Build a compact copy of a LogEntry for non-verbose JSON output.
/// - tool_result: first 3 lines + omitted count
/// - assistant text: first 200 chars + "..."
/// - tool_use: unchanged (already summarised)
fn compact_entry(entry: &LogEntry) -> LogEntry {
    let content = match entry.message_type {
        EntryType::ToolResult => truncate_lines(&entry.content, 3),
        EntryType::Assistant | EntryType::Thinking => truncate_chars(&entry.content, 200),
        _ => entry.content.clone(),
    };
    LogEntry {
        content,
        ..entry.clone()
    }
}

/// Format a LogEntry as a single JSON line.
/// When `verbose` is false, content is truncated to save tokens.
pub fn format_entry_json(entry: &LogEntry, verbose: bool) -> String {
    if verbose {
        serde_json::to_string(entry).unwrap_or_default()
    } else {
        serde_json::to_string(&compact_entry(entry)).unwrap_or_default()
    }
}

pub(crate) fn resolve_color(color: Option<&str>) -> Color {
    match color {
        Some(c) => match c.to_lowercase().as_str() {
            "black" => Color::Black,
            "red" => Color::Red,
            "green" => Color::Green,
            "yellow" => Color::Yellow,
            "blue" => Color::Blue,
            "magenta" | "purple" => Color::Magenta,
            "cyan" => Color::Cyan,
            "white" => Color::White,
            "orange" => Color::Rgb {
                r: 255,
                g: 165,
                b: 0,
            },
            hex if hex.starts_with('#') && hex.len() == 7 => {
                let r = u8::from_str_radix(&hex[1..3], 16).unwrap_or(255);
                let g = u8::from_str_radix(&hex[3..5], 16).unwrap_or(255);
                let b = u8::from_str_radix(&hex[5..7], 16).unwrap_or(255);
                Color::Rgb { r, g, b }
            }
            _ => Color::White,
        },
        None => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(message_type: EntryType, content: &str) -> LogEntry {
        LogEntry {
            timestamp: "2026-04-12T12:57:14.123Z".to_string(),
            agent_name: "advocate".to_string(),
            agent_color: Some("blue".to_string()),
            message_type,
            content: content.to_string(),
            tool_name: None,
            is_error: false,
            is_sidechain: false,
        }
    }

    #[test]
    fn test_format_entry_basic() {
        let entry = make_entry(EntryType::Assistant, "UX要件を整理中...");
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("advocate"));
        assert!(output.contains("UX要件を整理中..."));
        assert!(output.contains("[12:57:14]"));
    }

    #[test]
    fn test_format_tool_use() {
        let mut entry = make_entry(EntryType::ToolUse, "\"claude code log viewer github\"");
        entry.tool_name = Some("WebSearch".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("[web] WebSearch:"));
    }

    #[test]
    fn test_format_tool_use_unknown_uses_generic_icon() {
        let mut entry = make_entry(EntryType::ToolUse, "something");
        entry.tool_name = Some("SomeUnknownTool".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("[tool] SomeUnknownTool:"));
    }

    #[test]
    fn test_format_tool_use_mcp() {
        let mut entry = make_entry(EntryType::ToolUse, "[github] get_me: {}");
        entry.tool_name = Some("mcp__github__get_me".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("[mcp]"));
    }

    #[test]
    fn test_format_send_message() {
        let mut entry = make_entry(EntryType::ToolUse, "-> team-lead: done");
        entry.tool_name = Some("SendMessage".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("[msg]"));
    }

    #[test]
    fn test_format_task_completed() {
        let mut entry = make_entry(EntryType::ToolUse, "Task #1 completed");
        entry.tool_name = Some("TaskUpdate".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("[ok]"));
    }

    #[test]
    fn test_format_error() {
        let mut entry = make_entry(EntryType::System, "API rate limit exceeded");
        entry.is_error = true;
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains("[err]"));
    }

    #[test]
    fn test_truncate_lines() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_lines(text, 3);
        assert!(result.contains("line1"));
        assert!(result.contains("line3"));
        assert!(result.contains("2 more lines"));
        assert!(!result.contains("line4"));
    }

    #[test]
    fn test_resolve_color() {
        assert_eq!(resolve_color(Some("blue")), Color::Blue);
        assert_eq!(resolve_color(Some("RED")), Color::Red);
        assert_eq!(resolve_color(Some("purple")), Color::Magenta);
        assert_eq!(
            resolve_color(Some("#ff8800")),
            Color::Rgb {
                r: 255,
                g: 136,
                b: 0
            }
        );
        assert_eq!(resolve_color(None), Color::White);
    }

    #[test]
    fn test_format_entry_json_verbose() {
        let entry = make_entry(EntryType::Assistant, "hello");
        let json = format_entry_json(&entry, true);
        assert!(json.contains("\"agent_name\":\"advocate\""));
        assert!(json.contains("\"content\":\"hello\""));
    }

    #[test]
    fn test_format_entry_json_compact_tool_result() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6";
        let entry = make_entry(EntryType::ToolResult, content);
        let json = format_entry_json(&entry, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let c = parsed["content"].as_str().unwrap();
        assert!(c.contains("line1"));
        assert!(c.contains("line3"));
        assert!(c.contains("3 more lines"));
        assert!(!c.contains("line4"));
    }

    #[test]
    fn test_format_entry_json_compact_assistant() {
        let long_text = "a".repeat(300);
        let entry = make_entry(EntryType::Assistant, &long_text);
        let json = format_entry_json(&entry, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let c = parsed["content"].as_str().unwrap();
        assert!(c.ends_with("..."));
        // 200 chars + "..."
        assert_eq!(c.len(), 203);
    }

    #[test]
    fn test_format_entry_json_compact_short_assistant_unchanged() {
        let entry = make_entry(EntryType::Assistant, "short text");
        let json_compact = format_entry_json(&entry, false);
        let json_verbose = format_entry_json(&entry, true);
        assert_eq!(json_compact, json_verbose);
    }

    #[test]
    fn test_format_entry_json_compact_tool_use_unchanged() {
        let mut entry = make_entry(EntryType::ToolUse, "ls -la");
        entry.tool_name = Some("Bash".to_string());
        let json_compact = format_entry_json(&entry, false);
        let json_verbose = format_entry_json(&entry, true);
        assert_eq!(json_compact, json_verbose);
    }

    #[test]
    fn test_truncate_chars_multibyte() {
        let text = "あいうえお".repeat(100); // 500 chars
        let result = truncate_chars(&text, 200);
        assert_eq!(result.chars().count(), 203); // 200 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_timestamp_shortened() {
        let entry = make_entry(EntryType::Assistant, "test");
        let output = format_entry(&entry, false, true, 10);
        // Should show HH:MM:SS not full ISO 8601
        assert!(output.contains("[12:57:14]"));
        assert!(!output.contains("2026-04-12"));
    }

    #[test]
    fn test_render_inline_bold() {
        let result = render_inline_bold("hello **world** foo");
        assert!(result.contains("world"));
        // Should contain ANSI bold sequences
        assert!(result.contains("\x1b["));
        assert!(!result.contains("**"));
    }

    #[test]
    fn test_render_inline_bold_no_markers() {
        assert_eq!(render_inline_bold("plain text"), "plain text");
    }

    #[test]
    fn test_render_inline_bold_unclosed() {
        let result = render_inline_bold("open **bold but no close");
        assert_eq!(result, "open **bold but no close");
    }

    #[test]
    fn test_format_thinking_entry_no_color() {
        let entry = make_entry(EntryType::Thinking, "pondering a tricky bug");
        let output = format_entry(&entry, false, true, 10);
        assert!(output.contains("[thinking]"));
        assert!(output.contains("pondering a tricky bug"));
    }

    #[test]
    fn test_format_summary_entry_no_color() {
        let entry = make_entry(EntryType::Summary, "compacted history");
        let output = format_entry(&entry, false, true, 10);
        assert!(output.contains("[summary]"));
        assert!(output.contains("compacted history"));
    }

    #[test]
    fn test_format_result_entry_no_color() {
        let entry = make_entry(EntryType::Result, "session result: success (turns=3)");
        let output = format_entry(&entry, false, true, 10);
        assert!(output.contains("[result]"));
        assert!(output.contains("turns=3"));
    }

    #[test]
    fn test_format_snapshot_entry_no_color() {
        let entry = make_entry(EntryType::Snapshot, "[git snapshot]");
        let output = format_entry(&entry, false, true, 10);
        assert!(output.contains("[snapshot]"));
    }

    #[test]
    fn test_format_entry_sidechain_separator() {
        let mut entry = make_entry(EntryType::Assistant, "subagent text");
        entry.is_sidechain = true;
        let output = format_entry(&entry, false, true, 10);
        assert!(
            output.contains(">-"),
            "expected sidechain ASCII separator '>-' in {output:?}"
        );
        assert!(
            !output.contains('|'),
            "regular '|' separator must not appear for sidechain entries: {output:?}"
        );
        assert!(output.contains("subagent text"));
    }

    #[test]
    fn test_format_entry_regular_separator() {
        let entry = make_entry(EntryType::Assistant, "main text");
        let output = format_entry(&entry, false, true, 10);
        assert!(
            output.contains('|'),
            "regular entry should use '|' separator: {output:?}"
        );
        assert!(
            !output.contains(">-"),
            "regular entry must not use sidechain separator: {output:?}"
        );
    }
}
