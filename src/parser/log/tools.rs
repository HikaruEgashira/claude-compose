use serde_json::Value;

use super::types::truncate_chars;

/// Build a concise, human-readable summary of a `tool_use` block for
/// display. The shape of `input` depends on which tool was invoked, so the
/// dispatch below hand-extracts the most interesting bit per tool family.
///
/// Tools we don't specifically recognise fall back to compact JSON so the
/// viewer still shows *something* (rather than silently dropping the call).
pub(crate) fn extract_tool_use_summary(tool_name: &str, block: &Value) -> String {
    let input = block.get("input").cloned().unwrap_or(Value::Null);

    // Tools that share a `command` string input.
    if matches!(
        tool_name,
        "Bash" | "PowerShell" | "Monitor" | "SlashCommand"
    ) {
        return command_tool(&input, tool_name);
    }

    match tool_name {
        // --- messaging / Task coordination -----------------------------
        "SendMessage" => send_message(&input),
        "TaskUpdate" => task_update(&input),
        "TaskCreate" => str_field(&input, "subject").unwrap_or_default(),
        "TaskGet" | "TaskStop" => task_lookup(&input),
        "TaskList" => "tasks".to_string(),

        // --- scheduled tasks (Cron family) -----------------------------
        "CronCreate" => cron_create(&input),
        "CronDelete" => str_field(&input, "cron_id")
            .map(|id| format!("#{id}"))
            .unwrap_or_else(|| "cron".to_string()),
        "CronList" => "crons".to_string(),

        // --- filesystem -----------------------------------------------
        "Read" | "Write" | "Edit" => str_field(&input, "file_path").unwrap_or_default(),
        "MultiEdit" => multi_edit(&input),
        "NotebookEdit" => notebook_edit(&input),
        "Glob" => str_field(&input, "pattern").unwrap_or_default(),
        "Grep" => format!("/{}/", str_field(&input, "pattern").unwrap_or_default()),

        // --- background Bash controls ---------------------------------
        "BashOutput" => bash_output(&input),
        "KillBash" | "KillShell" => kill_shell(&input),

        // --- agents / planning ----------------------------------------
        "Task" => task_tool(&input),
        "ExitPlanMode" => format!(
            "plan: {}",
            truncate_chars(str_field(&input, "plan").as_deref().unwrap_or(""), 60)
        ),
        "EnterPlanMode" => "plan mode".to_string(),
        "Skill" => skill_tool(&input),

        // --- web / search ---------------------------------------------
        // `web_search` is the server-hosted variant that appears inside
        // `server_tool_use` blocks; treat it the same as `WebSearch`.
        "WebSearch" | "web_search" => str_field(&input, "query").unwrap_or_default(),
        "WebFetch" | "web_fetch" => str_field(&input, "url").unwrap_or_default(),

        // --- MCP resource surface -------------------------------------
        "ListMcpResourcesTool" => match str_field(&input, "server") {
            Some(server) => format!("[mcp:{server}] list resources"),
            None => "mcp resources".to_string(),
        },
        "ReadMcpResourceTool" => mcp_read_resource(&input),

        // --- infra / misc ---------------------------------------------
        "TodoWrite" => todo_write(&input),
        "ToolSearch" => str_field(&input, "query").unwrap_or_default(),
        "PushNotification" => {
            truncate_chars(str_field(&input, "message").as_deref().unwrap_or(""), 80)
        }
        "AskUserQuestion" => ask_user_question(&input),

        // --- namespaced MCP servers -----------------------------------
        name if name.starts_with("mcp__") => mcp_namespaced(name, &input),

        // --- unknown --------------------------------------------------
        _ => serde_json::to_string(&input).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------
// Per-tool helpers
// ---------------------------------------------------------------------

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn command_tool(input: &Value, tool_name: &str) -> String {
    let cmd = str_field(input, "command").unwrap_or_default();
    match tool_name {
        "SlashCommand" => {
            if cmd.starts_with('/') {
                truncate_chars(&cmd, 80)
            } else {
                format!("/{}", truncate_chars(&cmd, 79))
            }
        }
        _ => truncate_chars(&cmd, 80),
    }
}

fn send_message(input: &Value) -> String {
    let to = input.get("to").and_then(|v| v.as_str()).unwrap_or("?");
    let summary = input
        .get("summary")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("message").and_then(|m| m.as_str()))
        .unwrap_or("");
    format!("→ {to}: {summary}")
}

fn task_update(input: &Value) -> String {
    let task_id = input.get("taskId").and_then(|v| v.as_str()).unwrap_or("?");
    let status = input.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status.is_empty() {
        format!("Task #{task_id}")
    } else {
        format!("Task #{task_id} → {status}")
    }
}

fn task_lookup(input: &Value) -> String {
    match str_field(input, "task_id").or_else(|| str_field(input, "taskId")) {
        Some(id) => format!("#{id}"),
        None => String::new(),
    }
}

fn cron_create(input: &Value) -> String {
    let freq = str_field(input, "frequency");
    let one_shot = input
        .get("one_shot")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    let prefix = match (freq, one_shot) {
        (Some(f), _) => format!("[{f}] "),
        (None, true) => "[one-shot] ".to_string(),
        _ => String::new(),
    };
    format!("{prefix}{}", truncate_chars(prompt, 60))
}

fn multi_edit(input: &Value) -> String {
    let path = str_field(input, "file_path").unwrap_or_default();
    let n = input
        .get("edits")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!("{path} ({n} edits)")
}

fn notebook_edit(input: &Value) -> String {
    let path = str_field(input, "notebook_path").unwrap_or_default();
    match input.get("cell_id").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => format!("{path} [{id}]"),
        _ => path,
    }
}

fn bash_output(input: &Value) -> String {
    let id = str_field(input, "bash_id")
        .or_else(|| str_field(input, "shell_id"))
        .unwrap_or_else(|| "?".to_string());
    match str_field(input, "filter") {
        Some(f) => format!("#{id} /{f}/"),
        None => format!("#{id}"),
    }
}

fn kill_shell(input: &Value) -> String {
    str_field(input, "shell_id")
        .or_else(|| str_field(input, "bash_id"))
        .map(|id| format!("#{id}"))
        .unwrap_or_default()
}

fn task_tool(input: &Value) -> String {
    let subagent_type = input
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let description = input
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let head = if !subagent_type.is_empty() && !description.is_empty() {
        format!("{subagent_type}: {description}")
    } else if !subagent_type.is_empty() {
        let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        format!("{subagent_type}: {}", truncate_chars(prompt, 60))
    } else {
        let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        truncate_chars(prompt, 60)
    };

    // Flag worktree isolation and background execution so the log viewer
    // surfaces parallel work explicitly.
    let mut flags = Vec::new();
    if input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        flags.push("bg");
    }
    if input.get("isolation").and_then(|v| v.as_str()) == Some("worktree") {
        flags.push("worktree");
    }

    if flags.is_empty() {
        head
    } else {
        format!("{head} [{}]", flags.join(","))
    }
}

fn skill_tool(input: &Value) -> String {
    let skill = input.get("skill").and_then(|v| v.as_str()).unwrap_or("");
    let args = input.get("args").and_then(|v| v.as_str()).unwrap_or("");

    // Plugin-namespaced skills use `plugin:skill` form — surface the
    // plugin so the viewer can tell first-party from plugin skills.
    let label = match skill.split_once(':') {
        Some((plugin, name)) if !plugin.is_empty() && !name.is_empty() => {
            format!("[{plugin}] {name}")
        }
        _ => skill.to_string(),
    };

    if args.is_empty() {
        label
    } else {
        format!("{label} {args}")
    }
}

fn mcp_read_resource(input: &Value) -> String {
    let server = str_field(input, "server");
    let uri = str_field(input, "uri").unwrap_or_default();
    match server {
        Some(s) => format!("[mcp:{s}] {uri}"),
        None => uri,
    }
}

fn todo_write(input: &Value) -> String {
    let todos = match input.get("todos").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return "todos cleared".to_string(),
    };
    if todos.is_empty() {
        return "todos cleared".to_string();
    }

    // Newer TodoWrite schema carries `activeForm` per item; older schemas
    // use `status: "in_progress"`. Treat either as "in progress".
    let in_progress = todos
        .iter()
        .filter(|t| {
            let status = t.get("status").and_then(|s| s.as_str()).unwrap_or("");
            status == "in_progress" || t.get("activeForm").is_some_and(|v| !v.is_null())
        })
        .count();

    format!("{} todos ({} in progress)", todos.len(), in_progress)
}

fn ask_user_question(input: &Value) -> String {
    let question = input
        .get("questions")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|q| q.get("question"))
        .and_then(|q| q.as_str())
        .unwrap_or("");
    truncate_chars(question, 60)
}

fn mcp_namespaced(name: &str, input: &Value) -> String {
    let parts: Vec<&str> = name.splitn(3, "__").collect();
    let server = parts.get(1).copied().unwrap_or("");
    let tool = parts.get(2).copied().unwrap_or("");
    let compact = truncate_chars(&serde_json::to_string(input).unwrap_or_default(), 80);
    format!("[{server}] {tool}: {compact}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn block(name: &str, input: Value) -> Value {
        json!({ "type": "tool_use", "id": "t1", "name": name, "input": input })
    }

    fn summary(name: &str, input: Value) -> String {
        extract_tool_use_summary(name, &block(name, input))
    }

    #[test]
    fn bash_output_with_id_and_filter() {
        let s = summary("BashOutput", json!({ "bash_id": "42", "filter": "error" }));
        assert_eq!(s, "#42 /error/");
    }

    #[test]
    fn bash_output_falls_back_to_shell_id() {
        let s = summary("BashOutput", json!({ "shell_id": "abc" }));
        assert_eq!(s, "#abc");
    }

    #[test]
    fn kill_shell_reports_id() {
        assert_eq!(summary("KillShell", json!({ "shell_id": "xyz" })), "#xyz");
        assert_eq!(summary("KillBash", json!({ "bash_id": "7" })), "#7");
    }

    #[test]
    fn monitor_summarises_command() {
        let s = summary("Monitor", json!({ "command": "tail -f log.txt" }));
        assert_eq!(s, "tail -f log.txt");
    }

    #[test]
    fn tool_search_surfaces_query() {
        let s = summary("ToolSearch", json!({ "query": "github pull" }));
        assert_eq!(s, "github pull");
    }

    #[test]
    fn push_notification_summarises_message() {
        let s = summary("PushNotification", json!({ "message": "build green" }));
        assert_eq!(s, "build green");
    }

    #[test]
    fn slash_command_tool_prefixes_slash() {
        let s = summary("SlashCommand", json!({ "command": "/compact" }));
        assert_eq!(s, "/compact");
        let s = summary("SlashCommand", json!({ "command": "compact" }));
        assert_eq!(s, "/compact");
    }

    #[test]
    fn powershell_like_bash() {
        let s = summary("PowerShell", json!({ "command": "Get-Process" }));
        assert_eq!(s, "Get-Process");
    }

    #[test]
    fn enter_plan_mode_short_label() {
        assert_eq!(summary("EnterPlanMode", json!({})), "plan mode");
    }

    #[test]
    fn list_mcp_resources_with_server() {
        let s = summary("ListMcpResourcesTool", json!({ "server": "github" }));
        assert_eq!(s, "[mcp:github] list resources");
    }

    #[test]
    fn list_mcp_resources_without_server() {
        assert_eq!(summary("ListMcpResourcesTool", json!({})), "mcp resources");
    }

    #[test]
    fn read_mcp_resource_formats_server_and_uri() {
        let s = summary(
            "ReadMcpResourceTool",
            json!({ "server": "github", "uri": "resource://pr/1" }),
        );
        assert_eq!(s, "[mcp:github] resource://pr/1");
    }

    #[test]
    fn task_get_and_list_and_stop() {
        assert_eq!(summary("TaskGet", json!({ "task_id": "5" })), "#5");
        assert_eq!(summary("TaskStop", json!({ "task_id": "5" })), "#5");
        assert_eq!(summary("TaskList", json!({})), "tasks");
    }

    #[test]
    fn cron_summaries() {
        let s = summary(
            "CronCreate",
            json!({ "frequency": "5m", "prompt": "check PR" }),
        );
        assert_eq!(s, "[5m] check PR");
        let s = summary(
            "CronCreate",
            json!({ "one_shot": true, "prompt": "in 10 min" }),
        );
        assert_eq!(s, "[one-shot] in 10 min");
        assert_eq!(summary("CronDelete", json!({ "cron_id": "c1" })), "#c1");
        assert_eq!(summary("CronList", json!({})), "crons");
    }

    #[test]
    fn task_tool_surfaces_background_and_worktree() {
        let s = summary(
            "Task",
            json!({
                "subagent_type": "Explore",
                "description": "map the repo",
                "run_in_background": true,
                "isolation": "worktree",
            }),
        );
        assert_eq!(s, "Explore: map the repo [bg,worktree]");
    }

    #[test]
    fn task_tool_without_flags_is_unchanged() {
        let s = summary(
            "Task",
            json!({ "subagent_type": "Explore", "description": "look" }),
        );
        assert_eq!(s, "Explore: look");
    }

    #[test]
    fn skill_plugin_namespace_split() {
        let s = summary(
            "Skill",
            json!({ "skill": "my-plugin:format", "args": "all" }),
        );
        assert_eq!(s, "[my-plugin] format all");
    }

    #[test]
    fn skill_without_namespace_passes_through() {
        let s = summary("Skill", json!({ "skill": "simplify" }));
        assert_eq!(s, "simplify");
    }

    #[test]
    fn todo_write_counts_active_form() {
        let s = summary(
            "TodoWrite",
            json!({
                "todos": [
                    { "activeForm": "Writing tests" },
                    { "status": "in_progress" },
                    { "status": "pending" },
                ]
            }),
        );
        assert_eq!(s, "3 todos (2 in progress)");
    }

    #[test]
    fn todo_write_empty_array_is_cleared() {
        assert_eq!(
            summary("TodoWrite", json!({ "todos": [] })),
            "todos cleared"
        );
    }

    #[test]
    fn unknown_tool_falls_back_to_json() {
        let s = summary("UnrecognisedTool", json!({ "a": 1 }));
        assert!(s.contains("\"a\":1"));
    }

    #[test]
    fn mcp_namespaced_includes_server_and_tool() {
        let s = summary("mcp__github__get_me", json!({ "x": 1 }));
        assert!(s.starts_with("[github] get_me: "));
    }
}
