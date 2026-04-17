use std::collections::HashMap;
use std::io::IsTerminal;

use crossterm::style::{Color, Stylize};

use crate::cli::PsOpts;
use crate::discovery::{IdeAttachment, TodoSummary, load_all_todos, load_ide_attachments};
use crate::format::resolve_color;
use crate::parser::{TeamConfig, find_teams, load_team_config};

/// Print agent status table (claude-compose ps).
pub fn run(opts: PsOpts) -> anyhow::Result<()> {
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

    // Best-effort discovery (both return empty map on failure).
    let todos = load_all_todos();
    let ides = load_ide_attachments();

    if opts.json {
        return print_ps_json(&teams, explicit_team, &todos, &ides);
    }

    let no_color = !std::io::stdout().is_terminal() || std::env::var("NO_COLOR").is_ok();

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
            println!("{}", format!("Team: {team_name}").with(Color::Cyan).bold());
        } else {
            println!("Team: {team_name}");
        }

        // Team-level IDE line (if an IDE is attached to this team's cwd).
        if let Some(att) = lookup_ide_for_cwd(&ides, &config.cwd) {
            let line = format!("IDE: {} ({})", att.ide_name, att.workspace);
            if no_color {
                println!("{line}");
            } else {
                println!("{}", line.with(Color::Magenta));
            }
        }

        println!("{:<20} {:<10} {:<12}", "NAME", "STATUS", "TODOS");
        println!("{}", "-".repeat(46));

        for member in &config.members {
            let status = if member.is_active { "active" } else { "idle" };
            let todos_cell = render_todos_cell(lookup_todos_for_member(&todos, &member.name));

            if no_color {
                println!("{:<20} {:<10} {:<12}", member.name, status, todos_cell);
            } else {
                let color = resolve_color(member.color.as_deref());
                let styled_name = format!("{:<20}", member.name).with(color).bold();
                let styled_status = if member.is_active {
                    status.with(Color::Green).to_string()
                } else {
                    status.with(Color::DarkGrey).to_string()
                };
                println!("{styled_name} {styled_status:<10} {todos_cell:<12}");
            }
        }
        println!();
    }

    Ok(())
}

fn print_ps_json(
    teams: &[String],
    explicit_team: bool,
    todos: &HashMap<String, TodoSummary>,
    ides: &HashMap<String, IdeAttachment>,
) -> anyhow::Result<()> {
    let mut result: Vec<serde_json::Value> = Vec::new();

    for team_name in teams {
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

        let team_value = build_team_json(&config, todos, ides);
        result.push(team_value);
    }

    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

/// Build a single team's JSON value, including `todos` on members (when present)
/// and `ide` on the team (when attached).
fn build_team_json(
    config: &TeamConfig,
    todos: &HashMap<String, TodoSummary>,
    ides: &HashMap<String, IdeAttachment>,
) -> serde_json::Value {
    let members: Vec<serde_json::Value> = config
        .members
        .iter()
        .map(|m| build_member_json(&m.name, m.is_active, todos))
        .collect();

    let mut obj = serde_json::Map::new();
    obj.insert(
        "team".to_string(),
        serde_json::Value::String(config.team_name.clone()),
    );
    obj.insert("members".to_string(), serde_json::Value::Array(members));

    if let Some(att) = lookup_ide_for_cwd(ides, &config.cwd) {
        obj.insert(
            "ide".to_string(),
            serde_json::json!({
                "name": att.ide_name,
                "workspace": att.workspace,
            }),
        );
    }

    serde_json::Value::Object(obj)
}

fn build_member_json(
    name: &str,
    is_active: bool,
    todos: &HashMap<String, TodoSummary>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "agent_name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    obj.insert("active".to_string(), serde_json::Value::Bool(is_active));

    if let Some(summary) = lookup_todos_for_member(todos, name)
        && summary.total() > 0
    {
        obj.insert(
            "todos".to_string(),
            serde_json::json!({
                "pending": summary.pending,
                "in_progress": summary.in_progress,
                "completed": summary.completed,
            }),
        );
    }

    serde_json::Value::Object(obj)
}

/// Look up a todo summary for a member by best-effort name match.
/// TeamConfig does not carry per-member session IDs, so we scan the todos map
/// for any key containing the member name as a substring. Returns the first
/// non-empty match (deterministic under HashMap iteration by iterating once).
fn lookup_todos_for_member<'a>(
    todos: &'a HashMap<String, TodoSummary>,
    member_name: &str,
) -> Option<&'a TodoSummary> {
    if member_name.is_empty() {
        return None;
    }
    // Prefer exact-key match first (covers the case where session_id equals name).
    if let Some(s) = todos.get(member_name) {
        return Some(s);
    }
    // Fallback: any key containing the member name.
    todos
        .iter()
        .find(|(k, v)| k.contains(member_name) && v.total() > 0)
        .map(|(_, v)| v)
}

/// Look up the IDE attached to a given cwd, normalising paths so that
/// canonicalised and non-canonicalised forms both match.
fn lookup_ide_for_cwd<'a>(
    ides: &'a HashMap<String, IdeAttachment>,
    cwd: &str,
) -> Option<&'a IdeAttachment> {
    if cwd.is_empty() {
        return None;
    }
    if let Some(att) = ides.get(cwd) {
        return Some(att);
    }
    if let Ok(canon) = std::fs::canonicalize(cwd) {
        let canon_s = canon.to_string_lossy().into_owned();
        if let Some(att) = ides.get(&canon_s) {
            return Some(att);
        }
    }
    None
}

fn render_todos_cell(summary: Option<&TodoSummary>) -> String {
    match summary {
        Some(s) if s.total() > 0 => {
            format!("{}p/{}r/{}d", s.pending, s.in_progress, s.completed)
        }
        _ => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::MemberInfo;

    fn sample_member(name: &str, active: bool) -> MemberInfo {
        MemberInfo {
            name: name.to_string(),
            color: None,
            is_active: active,
            tmux_pane_id: None,
        }
    }

    fn sample_config(name: &str, cwd: &str, members: Vec<MemberInfo>) -> TeamConfig {
        TeamConfig {
            team_name: name.to_string(),
            lead_session_id: String::new(),
            cwd: cwd.to_string(),
            members,
        }
    }

    #[test]
    fn test_ps_json_explicit_nonexistent_team_errors() {
        let opts = PsOpts {
            team: Some("nonexistent-team-xyz".to_string()),
            json: true,
        };
        let result = run(opts);
        assert!(result.is_err());
    }

    #[test]
    fn test_ps_json_schema() {
        // Verify JSON schema matches spec:
        // [{"team":"name","members":[{"agent_name":"x","active":true}]}]
        let members = vec![
            MemberInfo {
                name: "agent-a".to_string(),
                color: None,
                is_active: true,
                tmux_pane_id: None,
            },
            MemberInfo {
                name: "agent-b".to_string(),
                color: None,
                is_active: false,
                tmux_pane_id: None,
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
        let member_keys: Vec<&str> = members_arr[0]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(member_keys.len(), 2);
        assert!(member_keys.contains(&"active"));
        assert!(member_keys.contains(&"agent_name"));
    }

    #[test]
    fn ps_json_includes_todos_when_present() {
        let config = sample_config(
            "t",
            "/tmp/does-not-exist-cc-ps",
            vec![sample_member("worker-a", true)],
        );

        let mut todos: HashMap<String, TodoSummary> = HashMap::new();
        todos.insert(
            "session-for-worker-a-xyz".to_string(),
            TodoSummary {
                pending: 3,
                in_progress: 1,
                completed: 5,
            },
        );
        let ides: HashMap<String, IdeAttachment> = HashMap::new();

        let v = build_team_json(&config, &todos, &ides);

        assert_eq!(v["team"], "t");
        let members = v["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        let m = &members[0];
        assert_eq!(m["agent_name"], "worker-a");
        assert_eq!(m["active"], true);
        let todos_v = &m["todos"];
        assert_eq!(todos_v["pending"], 3);
        assert_eq!(todos_v["in_progress"], 1);
        assert_eq!(todos_v["completed"], 5);

        // No IDE attached.
        assert!(v.get("ide").is_none());
    }

    #[test]
    fn ps_json_omits_todos_when_empty() {
        let config = sample_config(
            "t",
            "/tmp/does-not-exist-cc-ps",
            vec![sample_member("worker-a", false)],
        );

        // Empty summary — should be omitted.
        let mut todos: HashMap<String, TodoSummary> = HashMap::new();
        todos.insert("worker-a".to_string(), TodoSummary::default());
        let ides: HashMap<String, IdeAttachment> = HashMap::new();

        let v = build_team_json(&config, &todos, &ides);

        let m = &v["members"].as_array().unwrap()[0];
        assert!(
            m.get("todos").is_none(),
            "expected no `todos` key on member with zero totals, got {m:?}"
        );
    }

    #[test]
    fn ps_json_omits_todos_when_no_match() {
        let config = sample_config("t", "/tmp/x", vec![sample_member("worker-a", false)]);

        // No matching keys at all.
        let todos: HashMap<String, TodoSummary> = HashMap::new();
        let ides: HashMap<String, IdeAttachment> = HashMap::new();

        let v = build_team_json(&config, &todos, &ides);
        let m = &v["members"].as_array().unwrap()[0];
        assert!(m.get("todos").is_none());
    }

    #[test]
    fn ps_json_includes_ide_when_cwd_matches() {
        let cwd = "/tmp/some-ws";
        let config = sample_config("t", cwd, vec![sample_member("a", true)]);

        let todos: HashMap<String, TodoSummary> = HashMap::new();
        let mut ides: HashMap<String, IdeAttachment> = HashMap::new();
        ides.insert(
            cwd.to_string(),
            IdeAttachment {
                ide_name: "VS Code".to_string(),
                workspace: cwd.to_string(),
            },
        );

        let v = build_team_json(&config, &todos, &ides);
        let ide = v.get("ide").expect("expected ide key");
        assert_eq!(ide["name"], "VS Code");
        assert_eq!(ide["workspace"], cwd);
    }

    #[test]
    fn render_todos_cell_formats_nonempty() {
        let s = TodoSummary {
            pending: 2,
            in_progress: 1,
            completed: 4,
        };
        assert_eq!(render_todos_cell(Some(&s)), "2p/1r/4d");
        assert_eq!(render_todos_cell(None), "-");
        assert_eq!(render_todos_cell(Some(&TodoSummary::default())), "-");
    }

    #[test]
    fn lookup_todos_prefers_exact_key_match() {
        let mut todos: HashMap<String, TodoSummary> = HashMap::new();
        todos.insert(
            "worker-a".to_string(),
            TodoSummary {
                pending: 1,
                in_progress: 0,
                completed: 0,
            },
        );
        todos.insert(
            "other-worker-a-session".to_string(),
            TodoSummary {
                pending: 9,
                in_progress: 9,
                completed: 9,
            },
        );
        let got = lookup_todos_for_member(&todos, "worker-a").unwrap();
        assert_eq!(got.pending, 1);
    }

    #[test]
    fn lookup_todos_substring_fallback() {
        let mut todos: HashMap<String, TodoSummary> = HashMap::new();
        todos.insert(
            "abc-worker-a-xyz".to_string(),
            TodoSummary {
                pending: 2,
                in_progress: 0,
                completed: 1,
            },
        );
        let got = lookup_todos_for_member(&todos, "worker-a").unwrap();
        assert_eq!(got.pending, 2);
    }

    #[test]
    fn lookup_todos_empty_name_returns_none() {
        let mut todos: HashMap<String, TodoSummary> = HashMap::new();
        todos.insert(
            "anything".to_string(),
            TodoSummary {
                pending: 1,
                in_progress: 0,
                completed: 0,
            },
        );
        assert!(lookup_todos_for_member(&todos, "").is_none());
    }
}
