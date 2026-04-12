use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::cli::{DownOpts, UpOpts};
use crate::parser::{find_teams, load_team_config, team_config_path, MemberInfo, TeamConfig};

pub fn run_up(opts: UpOpts) -> anyhow::Result<()> {
    let team_name = resolve_team(&opts.team)?;
    let config = load_team_config(&team_name)?;

    if config.lead_session_id.is_empty() {
        anyhow::bail!(
            "Team '{}' has no active lead session. Start the lead first.",
            team_name
        );
    }

    let claude_bin = find_claude_binary()?;
    let tmux_session = ensure_tmux_session(&format!("cc-{team_name}"))?;

    let mut started = 0u32;
    for member in &config.members {
        if member.name == "team-lead" {
            continue;
        }
        // Only start tmux-based members (skip subagents or other backend types)
        if member.backend_type.as_deref().is_some_and(|b| b != "tmux") {
            continue;
        }
        if member.is_active && member.tmux_pane_id.is_some() {
            eprintln!("{}: already active, skipping", member.name);
            continue;
        }

        let pane_id = create_pane(&tmux_session)?;
        let cmd = build_member_command(&claude_bin, &config, member);
        send_keys(&pane_id, &cmd)?;

        update_config_pane(&team_name, &member.name, &pane_id)?;
        eprintln!("{}: started in pane {pane_id}", member.name);
        started += 1;
    }

    if started == 0 {
        eprintln!("No members to start.");
    } else {
        eprintln!("Started {started} member(s) in tmux session '{tmux_session}'.");
    }

    Ok(())
}

pub fn run_down(opts: DownOpts) -> anyhow::Result<()> {
    let team_name = resolve_team(&opts.team)?;
    let config = load_team_config(&team_name)?;

    let mut panes: Vec<(String, String)> = Vec::new(); // (name, pane_id)
    for member in &config.members {
        if member.name == "team-lead" {
            continue;
        }
        if let Some(pane_id) = &member.tmux_pane_id {
            panes.push((member.name.clone(), pane_id.clone()));
        }
    }

    if panes.is_empty() {
        eprintln!("No active members to stop.");
        return Ok(());
    }

    // SIGTERM first
    for (name, pane_id) in &panes {
        if is_pane_alive(pane_id) {
            send_signal(pane_id, "TERM");
            eprintln!("{name}: sent SIGTERM");
        }
    }

    // Wait for graceful shutdown
    thread::sleep(Duration::from_secs(3));

    // SIGKILL survivors
    for (name, pane_id) in &panes {
        if is_pane_alive(pane_id) {
            send_signal(pane_id, "KILL");
            eprintln!("{name}: sent SIGKILL");
        }
    }

    // Update config: mark all inactive, clear pane IDs
    for (_, pane_id) in &panes {
        // Kill the tmux pane itself
        let _ = Command::new("tmux")
            .args(["kill-pane", "-t", pane_id])
            .output();
    }

    update_config_down(&team_name)?;
    eprintln!("Stopped {} member(s).", panes.len());

    Ok(())
}

fn resolve_team(team_opt: &Option<String>) -> anyhow::Result<String> {
    if let Some(name) = team_opt {
        return Ok(name.clone());
    }
    let teams = find_teams();
    match teams.len() {
        0 => anyhow::bail!("No teams found in ~/.claude/teams/. Create a team first."),
        1 => Ok(teams.into_iter().next().unwrap()),
        _ => {
            eprintln!("Multiple teams found. Please specify --team:");
            for t in &teams {
                eprintln!("  {t}");
            }
            anyhow::bail!("Multiple teams found, use --team to specify")
        }
    }
}

fn find_claude_binary() -> anyhow::Result<String> {
    let output = Command::new("which").arg("claude").output()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Ok(path);
        }
    }
    // Fallback: check common install location
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let local_bin = home.join(".local/bin/claude");
    if local_bin.is_file() {
        return Ok(local_bin.to_string_lossy().to_string());
    }
    anyhow::bail!("claude binary not found. Install Claude Code first.")
}

fn ensure_tmux_session(name: &str) -> anyhow::Result<String> {
    // Check if session exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()?;
    if !check.status.success() {
        // Create detached session
        let create = Command::new("tmux")
            .args(["new-session", "-d", "-s", name])
            .output()?;
        if !create.status.success() {
            anyhow::bail!(
                "failed to create tmux session '{}': {}",
                name,
                String::from_utf8_lossy(&create.stderr)
            );
        }
    }
    Ok(name.to_string())
}

fn create_pane(session: &str) -> anyhow::Result<String> {
    let output = Command::new("tmux")
        .args([
            "split-window",
            "-t",
            session,
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to create tmux pane: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn build_member_command(claude_bin: &str, config: &TeamConfig, member: &MemberInfo) -> String {
    let default_agent_id = format!("{}@{}", member.name, config.team_name);
    let agent_id = member.agent_id.as_deref().unwrap_or(&default_agent_id);
    let color = member.color.as_deref().unwrap_or("white");
    let model = member.model.as_deref().unwrap_or("claude-sonnet-4-6");
    let cwd = member.cwd.as_deref().unwrap_or(&config.cwd);

    format!(
        "cd {} && {} --agent-id {} --agent-name {} --team-name {} --agent-color {} --parent-session-id {} --model {}",
        shell_quote(cwd),
        shell_quote(claude_bin),
        shell_quote(agent_id),
        shell_quote(&member.name),
        shell_quote(&config.team_name),
        shell_quote(color),
        shell_quote(&config.lead_session_id),
        shell_quote(model),
    )
}

/// Shell-safe quoting: wrap in single quotes, escaping any embedded single quotes.
/// `it's here` becomes `'it'\''s here'`
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn send_keys(pane_id: &str, command: &str) -> anyhow::Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, command, "Enter"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to send keys to pane {}: {}",
            pane_id,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn is_pane_alive(pane_id: &str) -> bool {
    Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_pid}"])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn send_signal(pane_id: &str, signal: &str) {
    // Get the PID of the process running in the pane
    if let Ok(output) = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{pane_pid}"])
        .output()
    {
        let pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !pid.is_empty() {
            // Signal the child process (Claude Code), not the shell
            if let Ok(children) = Command::new("pgrep").args(["-P", &pid]).output() {
                for child_pid in String::from_utf8_lossy(&children.stdout).lines() {
                    let _ = Command::new("kill")
                        .args([&format!("-{signal}"), child_pid.trim()])
                        .output();
                }
            }
        }
    }
}

/// Update a member's tmuxPaneId and isActive in the team config.
fn update_config_pane(team_name: &str, member_name: &str, pane_id: &str) -> anyhow::Result<()> {
    let path = team_config_path(team_name)?;
    let data = std::fs::read_to_string(&path)?;
    let mut v: serde_json::Value = serde_json::from_str(&data)?;

    if let Some(members) = v.get_mut("members").and_then(|m| m.as_array_mut()) {
        for m in members {
            if m.get("name").and_then(|n| n.as_str()) == Some(member_name) {
                m["tmuxPaneId"] = serde_json::Value::String(pane_id.to_string());
                m["isActive"] = serde_json::Value::Bool(true);
                break;
            }
        }
    }

    std::fs::write(&path, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

/// Mark all non-lead members as inactive in the team config.
fn update_config_down(team_name: &str) -> anyhow::Result<()> {
    let path = team_config_path(team_name)?;
    let data = std::fs::read_to_string(&path)?;
    let mut v: serde_json::Value = serde_json::from_str(&data)?;

    if let Some(members) = v.get_mut("members").and_then(|m| m.as_array_mut()) {
        for m in members {
            let is_lead = m.get("name").and_then(|n| n.as_str()) == Some("team-lead");
            if !is_lead {
                m["isActive"] = serde_json::Value::Bool(false);
            }
        }
    }

    std::fs::write(&path, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> TeamConfig {
        TeamConfig {
            team_name: "test-team".to_string(),
            lead_session_id: "lead-sid-123".to_string(),
            cwd: "/home/user/project".to_string(),
            members: vec![],
        }
    }

    fn make_member(name: &str) -> MemberInfo {
        MemberInfo {
            name: name.to_string(),
            color: Some("blue".to_string()),
            is_active: false,
            tmux_pane_id: None,
            agent_id: Some(format!("{name}@test-team")),
            model: Some("claude-opus-4-6".to_string()),
            cwd: Some("/home/user/project".to_string()),
            backend_type: Some("tmux".to_string()),
        }
    }

    #[test]
    fn build_member_command_contains_required_flags() {
        let config = make_config();
        let member = make_member("worker");
        let cmd = build_member_command("/usr/bin/claude", &config, &member);

        assert!(cmd.contains("--agent-id 'worker@test-team'"));
        assert!(cmd.contains("--agent-name 'worker'"));
        assert!(cmd.contains("--team-name 'test-team'"));
        assert!(cmd.contains("--agent-color 'blue'"));
        assert!(cmd.contains("--parent-session-id 'lead-sid-123'"));
        assert!(cmd.contains("--model 'claude-opus-4-6'"));
        assert!(cmd.starts_with("cd '/home/user/project' && '/usr/bin/claude'"));
    }

    #[test]
    fn build_member_command_uses_defaults() {
        let config = make_config();
        let member = MemberInfo {
            name: "minimal".to_string(),
            color: None,
            is_active: false,
            tmux_pane_id: None,
            agent_id: None,
            model: None,
            cwd: None,
            backend_type: None,
        };
        let cmd = build_member_command("claude", &config, &member);

        assert!(cmd.contains("--agent-id 'minimal@test-team'"));
        assert!(cmd.contains("--agent-color 'white'"));
        assert!(cmd.contains("--model 'claude-sonnet-4-6'"));
        assert!(cmd.contains("cd '/home/user/project'"));
    }

    #[test]
    fn build_member_command_handles_spaces_in_path() {
        let config = make_config();
        let mut member = make_member("worker");
        member.cwd = Some("/home/user/my project".to_string());
        let cmd = build_member_command("/usr/bin/claude", &config, &member);
        assert!(cmd.starts_with("cd '/home/user/my project'"));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("normal"), "'normal'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_handles_special_chars() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("foo;bar"), "'foo;bar'");
        assert_eq!(shell_quote("a`cmd`b"), "'a`cmd`b'");
    }
}
