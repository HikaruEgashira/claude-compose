use std::io::IsTerminal;

use crossterm::style::{Color, Stylize};

use crate::cli::PsOpts;
use crate::format::resolve_color;
use crate::parser::{find_teams, load_team_config};

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

    if opts.json {
        return print_ps_json(&teams, explicit_team);
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
    use crate::parser::MemberInfo;

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
}
