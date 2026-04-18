use serde::Serialize;

/// A single renderable row produced from a Claude Code JSONL line.
///
/// One source record can emit multiple entries (e.g. an assistant turn with
/// both a text block and a tool_use block). All entries produced from the
/// same record share the same `uuid`/`parent_uuid`/meta fields.
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
    /// Stop reason emitted by the model (`end_turn`, `tool_use`, `pause_turn`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stop_reason: Option<String>,
    /// Usage statistics (input/output/cache tokens) for assistant messages.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<Usage>,
    /// Kind of injected XML-like tag found in the content (slash command,
    /// hook output, system reminder, github-webhook-activity, env tag).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag: Option<TagKind>,
    /// Session id that produced this record — distinct from the file stem
    /// because sidechain entries may originate in a parent/child session.
    #[serde(
        rename = "sessionId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub session_id: Option<String>,
    /// Working directory at the time the record was written. May change
    /// mid-session (`cd` in Bash) so is carried per-entry.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    /// Git branch active when the record was written.
    #[serde(
        rename = "gitBranch",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub git_branch: Option<String>,
    /// Claude Code format/version string (e.g. `"2.0.47"`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<String>,
    /// `"user"` vs `"agent"` — lets downstream distinguish Task-originated
    /// prompts from human ones.
    #[serde(
        rename = "userType",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub user_type: Option<String>,
    /// Meta records (harness-inserted, not conversation) — e.g. the initial
    /// environment block.
    #[serde(rename = "isMeta", default, skip_serializing_if = "is_false")]
    pub is_meta: bool,
    /// API-layer error (rate limit, overloaded, etc.). Distinct from a
    /// tool-level `is_error` flag.
    #[serde(
        rename = "isApiErrorMessage",
        default,
        skip_serializing_if = "is_false"
    )]
    pub is_api_error: bool,
    /// Correlates the record with the API request that produced it.
    #[serde(
        rename = "requestId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub request_id: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Classification of XML-like tags injected by Claude Code into
/// user/assistant text content (not written by the user themselves).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TagKind {
    /// `<command-name>` / `<command-message>` / `<command-args>` — slash
    /// command invocation metadata.
    SlashCommand,
    /// Any `<*-hook>` variant (user-prompt-submit, pre/post-tool-use,
    /// stop, subagent-stop, session-start/end, pre/post-compact,
    /// notification, permission-denied, cwd-changed, file-changed,
    /// task-created).
    Hook,
    /// `<system-reminder>` — auto-injected system reminders.
    SystemReminder,
    /// `<ide-selection>` / `<ide-diagnostic>` — IDE context.
    Ide,
    /// `<local-command-stdout>` and `<bash-input>` / `<bash-stdout>` /
    /// `<bash-stderr>` — inline shell context.
    Bash,
    /// `<github-webhook-activity>` — PR/CI event wrapper injected by the
    /// GitHub integration.
    GitHubActivity,
    /// Environment-level injections: `<available-skills>`, `<user-memory>`,
    /// `<current-branch>`, `<current-working-directory>`, etc.
    Env,
}

/// Token-usage metadata carried on assistant records.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
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
    /// `"system"` record with `subtype: "compact_boundary"` — marks the
    /// point at which Claude auto-compacted the transcript. Rendered as a
    /// distinct separator rather than a generic system line.
    CompactBoundary,
}

impl LogEntry {
    /// Construct a bare LogEntry with only the mandatory fields populated.
    /// All optional metadata defaults to `None`/`false`.
    pub(crate) fn new(
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
            tag: None,
            session_id: None,
            cwd: None,
            git_branch: None,
            version: None,
            user_type: None,
            is_meta: false,
            is_api_error: false,
            request_id: None,
        }
    }

    pub(crate) fn with_tool(mut self, name: &str) -> Self {
        self.tool_name = Some(name.to_string());
        self
    }

    pub(crate) fn with_tag(mut self, tag: Option<TagKind>) -> Self {
        self.tag = tag;
        self
    }

    pub(crate) fn with_error(mut self, is_error: bool) -> Self {
        self.is_error = is_error;
        self
    }

    pub(crate) fn with_meta(mut self, meta: &super::meta::RecordMeta) -> Self {
        self.is_sidechain = meta.is_sidechain;
        self.uuid = meta.uuid.clone();
        self.parent_uuid = meta.parent_uuid.clone();
        self.model = meta.model.clone();
        self.stop_reason = meta.stop_reason.clone();
        self.usage = meta.usage.clone();
        self.session_id = meta.session_id.clone();
        self.cwd = meta.cwd.clone();
        self.git_branch = meta.git_branch.clone();
        self.version = meta.version.clone();
        self.user_type = meta.user_type.clone();
        self.is_meta = meta.is_meta;
        self.is_api_error = meta.is_api_error;
        self.request_id = meta.request_id.clone();
        self
    }
}

impl Default for LogEntry {
    fn default() -> Self {
        Self::new("", "", None, EntryType::System, String::new())
    }
}

/// Truncate a string to at most `max` characters (not bytes).
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_timestamp_extracts_time() {
        assert_eq!(format_timestamp("2026-04-12T12:57:14.123Z"), "12:57:14");
        assert_eq!(format_timestamp("short"), "short");
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

    #[test]
    fn default_logentry_has_sane_blanks() {
        let e = LogEntry::default();
        assert_eq!(e.message_type, EntryType::System);
        assert!(e.uuid.is_none());
        assert!(!e.is_sidechain);
        assert!(!e.is_meta);
        assert!(!e.is_api_error);
    }

    #[test]
    fn new_meta_fields_omitted_when_none() {
        // serde skip_serializing_if keeps JSON compact when the new meta
        // fields are unset — otherwise every line would grow by several
        // noise keys.
        let e = LogEntry::default();
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("\"sessionId\""));
        assert!(!json.contains("\"cwd\""));
        assert!(!json.contains("\"gitBranch\""));
        assert!(!json.contains("\"isMeta\""));
        assert!(!json.contains("\"isApiErrorMessage\""));
    }

    #[test]
    fn new_meta_fields_use_camel_case() {
        let e = LogEntry {
            session_id: Some("s".into()),
            cwd: Some("/tmp".into()),
            git_branch: Some("main".into()),
            user_type: Some("agent".into()),
            is_meta: true,
            is_api_error: true,
            request_id: Some("r".into()),
            ..LogEntry::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"sessionId\":\"s\""));
        assert!(json.contains("\"gitBranch\":\"main\""));
        assert!(json.contains("\"userType\":\"agent\""));
        assert!(json.contains("\"isMeta\":true"));
        assert!(json.contains("\"isApiErrorMessage\":true"));
        assert!(json.contains("\"requestId\":\"r\""));
    }

    #[test]
    fn truncate_chars_respects_char_boundary() {
        let s = "あいうえお";
        assert_eq!(truncate_chars(s, 2), "あい");
        assert_eq!(truncate_chars(s, 100), "あいうえお");
    }
}
