use serde_json::Value;
use std::fs;

use super::team::claude_home;

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
/// Reads the first 5 lines of each JSONL to find teamName and agentName.
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
        assert_eq!(
            result,
            Some(("abc123".to_string(), "backend-dev".to_string()))
        );

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
