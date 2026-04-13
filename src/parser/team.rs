use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct TeamConfig {
    pub team_name: String,
    pub lead_session_id: String,
    pub cwd: String,
    pub members: Vec<MemberInfo>,
}

#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub name: String,
    pub color: Option<String>,
    pub is_active: bool,
    pub tmux_pane_id: Option<String>,
}

pub fn claude_home() -> anyhow::Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))
}

/// Convert a cwd path to the Claude project key format.
/// "/Users/hikae/ghq/github.com/Foo/bar" → "-Users-hikae-ghq-github-com-Foo-bar"
pub fn cwd_to_project_key(cwd: &str) -> String {
    cwd.replace(['/', '.'], "-")
}

/// List all team names found under ~/.claude/teams/
pub fn find_teams() -> Vec<String> {
    let Ok(claude) = claude_home() else {
        return vec![];
    };
    let teams_dir = claude.join("teams");
    let Ok(entries) = fs::read_dir(&teams_dir) else {
        return vec![];
    };
    entries
        .filter_map(|e| {
            let e = e.ok()?;
            if !e.file_type().ok()?.is_dir() {
                return None;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            // Only include teams that have a config.json
            let config_path = e.path().join("config.json");
            if config_path.is_file() {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Load team config from ~/.claude/teams/{team_name}/config.json
pub fn load_team_config(team_name: &str) -> anyhow::Result<TeamConfig> {
    let path = claude_home()?
        .join("teams")
        .join(team_name)
        .join("config.json");
    let data = fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&data)?;

    let lead_session_id = v
        .get("leadSessionId")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    // Get cwd from the lead member or top-level
    let cwd = v
        .get("members")
        .and_then(|m| m.as_array())
        .and_then(|arr| arr.first())
        .and_then(|m| m.get("cwd"))
        .and_then(|c| c.as_str())
        .or_else(|| v.get("cwd").and_then(|c| c.as_str()))
        .unwrap_or("")
        .to_string();

    let members: Vec<MemberInfo> = v
        .get("members")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    let pane_id = m
                        .get("tmuxPaneId")
                        .and_then(|p| p.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                    MemberInfo {
                        name: m
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        color: m.get("color").and_then(|c| c.as_str()).map(String::from),
                        is_active: m.get("isActive").and_then(|a| a.as_bool()).unwrap_or(false),
                        tmux_pane_id: pane_id,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(TeamConfig {
        team_name: team_name.to_string(),
        lead_session_id,
        cwd,
        members,
    })
}

/// Read agent name from a subagent's meta.json.
/// Prefers `description`, falls back to `agentType`.
pub fn read_subagent_name(meta_path: &std::path::Path) -> Option<String> {
    let data = fs::read_to_string(meta_path).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    v.get("description")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| v.get("agentType").and_then(|t| t.as_str()))
        .map(String::from)
}

/// Resolve the project log directory for a team.
pub fn project_log_dir(config: &TeamConfig) -> anyhow::Result<PathBuf> {
    let claude = claude_home()?;
    let project_key = cwd_to_project_key(&config.cwd);
    Ok(claude.join("projects").join(project_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_to_project_key_converts_path() {
        assert_eq!(
            cwd_to_project_key("/Users/hikae/ghq/github.com/Foo/bar"),
            "-Users-hikae-ghq-github-com-Foo-bar"
        );
    }

    #[test]
    fn find_teams_skips_dirs_without_config() {
        // find_teams should not crash on dirs without config.json (like "default")
        let teams = find_teams();
        assert!(!teams.contains(&"default".to_string()));
    }
}
