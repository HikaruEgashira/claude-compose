use std::io::IsTerminal;

use crossterm::style::{Color, Stylize};

use crate::cli::PsOpts;
use crate::parser::{find_teams, format_timestamp, load_team_config, EntryType, LogEntry};

/// Format a LogEntry for terminal output.
pub fn format_entry(
    entry: &LogEntry,
    verbose: bool,
    no_color: bool,
    max_name_width: usize,
) -> String {
    let no_color = no_color || !std::io::stdout().is_terminal();

    let ts = format_timestamp(&entry.timestamp);
    let timestamp = format!("[{ts}]");
    let name = format!("{:<width$}", entry.agent_name, width = max_name_width);

    let content = format_content(entry, verbose);

    if no_color {
        return format!("{timestamp} {name}│ {content}");
    }

    let color = resolve_color(entry.agent_color.as_deref());

    let styled_name = name.with(color).bold();
    let styled_content = if entry.is_error {
        content.with(Color::Red).to_string()
    } else {
        content
    };

    format!("{timestamp} {styled_name}│ {styled_content}")
}

fn format_content(entry: &LogEntry, verbose: bool) -> String {
    match entry.message_type {
        EntryType::ToolUse => {
            let tool = entry.tool_name.as_deref().unwrap_or("unknown");
            if tool == "SendMessage" {
                format!(" {}", entry.content)
            } else if tool == "TaskUpdate" && entry.content.contains("completed") {
                format!(" {}", entry.content)
            } else if tool == "TaskCreate" {
                format!(" {}", entry.content)
            } else {
                format!(" {tool}: {}", entry.content)
            }
        }
        EntryType::ToolResult => {
            if verbose {
                entry.content.clone()
            } else {
                truncate_lines(&entry.content, 3)
            }
        }
        _ => {
            if entry.is_error {
                format!(" {}", entry.content)
            } else {
                entry.content.clone()
            }
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

/// Format a LogEntry as a single JSON line.
pub fn format_entry_json(entry: &LogEntry) -> String {
    serde_json::to_string(entry).unwrap_or_default()
}

fn resolve_color(color: Option<&str>) -> Color {
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

/// Print agent status table (claude-compose ps).
pub fn print_ps(opts: PsOpts) -> anyhow::Result<()> {
    let explicit_team = opts.team.is_some();
    let teams = if let Some(ref name) = opts.team {
        vec![name.clone()]
    } else {
        find_teams()
    };

    if teams.is_empty() {
        if opts.json {
            println!("[]");
        } else {
            println!("No active teams found.");
        }
        return Ok(());
    }

    if opts.json {
        return print_ps_json(&teams, explicit_team);
    }

    let no_color = !std::io::stdout().is_terminal();

    for team_name in &teams {
        let config = match load_team_config(team_name) {
            Ok(c) => c,
            Err(e) => {
                if explicit_team {
                    anyhow::bail!("team '{team_name}' not found: {e}");
                }
                eprintln!("Warning: skipping team '{team_name}': {e}");
                continue;
            }
        };

        if !no_color {
            println!(
                "{}",
                format!("Team: {team_name}").with(Color::Cyan).bold()
            );
        } else {
            println!("Team: {team_name}");
        }

        println!("{:<20} {:<10}", "NAME", "STATUS");
        println!("{}", "-".repeat(32));

        for member in &config.members {
            let status = if member.is_active { "active" } else { "idle" };

            if no_color {
                println!("{:<20} {:<10}", member.name, status);
            } else {
                let color = resolve_color(member.color.as_deref());
                let styled_name = format!("{:<20}", member.name).with(color).bold();
                let styled_status = if member.is_active {
                    status.with(Color::Green).to_string()
                } else {
                    status.with(Color::DarkGrey).to_string()
                };
                println!("{styled_name} {styled_status:<10}");
            }
        }
        println!();
    }

    Ok(())
}

fn print_ps_json(teams: &[String], explicit_team: bool) -> anyhow::Result<()> {
    let mut result: Vec<serde_json::Value> = Vec::new();

    for team_name in teams {
        let config = match load_team_config(team_name) {
            Ok(c) => c,
            Err(e) => {
                if explicit_team {
                    anyhow::bail!("team '{team_name}' not found: {e}");
                }
                continue;
            }
        };

        let members: Vec<serde_json::Value> = config
            .members
            .iter()
            .map(|m| {
                serde_json::json!({
                    "agent_name": m.name,
                    "active": m.is_active,
                })
            })
            .collect();

        result.push(serde_json::json!({
            "team": team_name,
            "members": members,
        }));
    }

    println!("{}", serde_json::to_string(&result)?);
    Ok(())
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
        assert!(output.contains(" WebSearch:"));
    }

    #[test]
    fn test_format_send_message() {
        let mut entry = make_entry(EntryType::ToolUse, "→ team-lead: ペインポイントの整理完了");
        entry.tool_name = Some("SendMessage".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains(""));
    }

    #[test]
    fn test_format_task_completed() {
        let mut entry = make_entry(EntryType::ToolUse, "Task #1 completed");
        entry.tool_name = Some("TaskUpdate".to_string());
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains(""));
    }

    #[test]
    fn test_format_error() {
        let mut entry = make_entry(EntryType::System, "API rate limit exceeded");
        entry.is_error = true;
        let output = format_entry(&entry, false, true, 20);
        assert!(output.contains(""));
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
    fn test_format_entry_json() {
        let entry = make_entry(EntryType::Assistant, "hello");
        let json = format_entry_json(&entry);
        assert!(json.contains("\"agent_name\":\"advocate\""));
        assert!(json.contains("\"content\":\"hello\""));
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
    fn test_ps_json_explicit_nonexistent_team_errors() {
        let opts = PsOpts {
            team: Some("nonexistent-team-xyz".to_string()),
            json: true,
        };
        let result = print_ps(opts);
        assert!(result.is_err());
    }

    #[test]
    fn test_ps_json_schema() {
        // Verify JSON schema matches spec:
        // [{"team":"name","members":[{"agent_name":"x","active":true}]}]
        use crate::parser::MemberInfo;
        let members = vec![
            MemberInfo {
                name: "agent-a".to_string(),
                color: None,
                is_active: true,
            },
            MemberInfo {
                name: "agent-b".to_string(),
                color: None,
                is_active: false,
            },
        ];

        let json_members: Vec<serde_json::Value> = members
            .iter()
            .map(|m| {
                serde_json::json!({
                    "agent_name": m.name,
                    "active": m.is_active,
                })
            })
            .collect();

        let result = vec![serde_json::json!({
            "team": "test-team",
            "members": json_members,
        })];

        let output = serde_json::to_string(&result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["team"], "test-team");

        let members_arr = arr[0]["members"].as_array().unwrap();
        assert_eq!(members_arr.len(), 2);
        assert_eq!(members_arr[0]["agent_name"], "agent-a");
        assert_eq!(members_arr[0]["active"], true);
        assert_eq!(members_arr[1]["agent_name"], "agent-b");
        assert_eq!(members_arr[1]["active"], false);

        // Must not contain unexpected keys
        let member_keys: Vec<&str> = members_arr[0].as_object().unwrap().keys().map(|k| k.as_str()).collect();
        assert_eq!(member_keys.len(), 2);
        assert!(member_keys.contains(&"active"));
        assert!(member_keys.contains(&"agent_name"));
    }
}
