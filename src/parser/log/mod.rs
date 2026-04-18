//! JSONL transcript parser for Claude Code session files.
//!
//! Split into submodules so each file stays tightly scoped:
//! - [`types`]    — the public `LogEntry`/`EntryType`/`TagKind`/`Usage` shapes
//! - [`tags`]     — classification of injected `<…>` tags
//! - [`meta`]     — per-record metadata extraction
//! - [`tools`]    — per-tool human-readable summaries for `tool_use` blocks
//! - [`blocks`]   — per-type block parsers (assistant/user/system/…)

mod blocks;
mod meta;
mod tags;
mod tools;
mod types;

pub use types::{EntryType, LogEntry, TagKind, Usage, format_timestamp};

#[cfg(test)]
pub(crate) use tags::{classify_tag, detect_first_tag};

use serde_json::Value;

/// Parse a single JSONL line into zero or more LogEntry values.
///
/// One line can produce multiple entries — e.g. an assistant turn with both
/// a text block and a tool_use block yields two entries sharing the same
/// uuid/meta. Malformed JSON silently yields an empty vec so a corrupt
/// line doesn't stop the stream.
pub fn parse_line(line: &str, agent_name: &str, agent_color: Option<&str>) -> Vec<LogEntry> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let top_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let meta = meta::extract_record_meta(&v);

    match top_type {
        "assistant" => blocks::parse_assistant(&v, &timestamp, agent_name, agent_color, &meta),
        "user" => blocks::parse_user(&v, &timestamp, agent_name, agent_color, &meta),
        "system" => blocks::parse_system(&v, &timestamp, agent_name, agent_color, &meta),
        "summary" => blocks::parse_summary(&v, &timestamp, agent_name, agent_color, &meta),
        "result" => blocks::parse_result(&v, &timestamp, agent_name, agent_color, &meta),
        "file-history-snapshot" => {
            blocks::parse_snapshot(&v, &timestamp, agent_name, agent_color, &meta)
        }
        _ => vec![],
    }
}

// ---------------------------------------------------------------------
// End-to-end tests. Per-piece tests live alongside their owning module
// (see `types::tests`, `tags::tests`, `meta::tests`, `tools::tests`,
// `blocks::tests`). This module tests the `parse_line` dispatcher and
// meta-propagation across the block parsers.
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_user_message() {
        let line = r#"{"type":"user","timestamp":"2026-04-12T08:46:49.067Z","message":{"role":"user","content":"hello"}}"#;
        let entries = parse_line(line, "test-agent", Some("blue"));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::User);
        assert_eq!(entries[0].content, "hello");
        assert_eq!(entries[0].agent_color.as_deref(), Some("blue"));
    }

    #[test]
    fn parse_assistant_with_text_and_tool_use() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-10T07:01:55.588Z","message":{"role":"assistant","content":[{"type":"text","text":"Let me check."},{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"ls"}}]}}"#;
        let entries = parse_line(line, "agent-a", None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message_type, EntryType::Assistant);
        assert_eq!(entries[0].content, "Let me check.");
        assert_eq!(entries[1].message_type, EntryType::ToolUse);
        assert_eq!(entries[1].tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn parse_tool_result_in_user_message() {
        let line = r#"{"type":"user","timestamp":"2026-04-12T08:46:53.109Z","message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":"file1.txt\nfile2.txt","is_error":false}]}}"#;
        let entries = parse_line(line, "agent-b", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::ToolResult);
        assert!(!entries[0].is_error);
    }

    #[test]
    fn parse_system_message() {
        let line = r#"{"type":"system","subtype":"bridge_status","content":"connected","timestamp":"2026-04-12T08:46:35.621Z"}"#;
        let entries = parse_line(line, "sys", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::System);
        assert_eq!(entries[0].content, "connected");
    }

    #[test]
    fn skip_metadata_types() {
        let line = r#"{"type":"permission-mode","permissionMode":"default"}"#;
        assert!(parse_line(line, "x", None).is_empty());
    }

    #[test]
    fn parse_thinking_block() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"sig"},{"type":"text","text":"answer"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message_type, EntryType::Thinking);
        assert_eq!(entries[0].content, "hmm");
        assert_eq!(entries[1].message_type, EntryType::Assistant);
        assert_eq!(entries[1].content, "answer");
    }

    #[test]
    fn parse_redacted_thinking_block() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"redacted_thinking","data":"xyz"},{"type":"text","text":"answer"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message_type, EntryType::Thinking);
        assert_eq!(entries[0].content, "[redacted thinking]");
        assert_eq!(entries[1].message_type, EntryType::Assistant);
    }

    #[test]
    fn parse_summary_message() {
        let line = r#"{"type":"summary","summary":"Conversation compacted","leafUuid":"abc","timestamp":"2026-04-12T08:46:49.067Z"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Summary);
        assert_eq!(entries[0].content, "Conversation compacted");
    }

    #[test]
    fn parse_summary_missing_timestamp() {
        let line = r#"{"type":"summary","summary":"S","leafUuid":"abc"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, "");
    }

    #[test]
    fn parse_result_message() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.1234,"duration_ms":1200,"num_turns":5,"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Result);
        assert!(entries[0].content.contains("session result: success"));
        assert!(entries[0].content.contains("turns=5"));
        assert!(entries[0].content.contains("cost=$0.1234"));
    }

    #[test]
    fn parse_result_preserves_small_cost() {
        let line = r#"{"type":"result","subtype":"success","total_cost_usd":0.004,"num_turns":1,"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("cost=$0.004"));
    }

    #[test]
    fn parse_result_trims_trailing_zeros() {
        let line = r#"{"type":"result","subtype":"success","total_cost_usd":2.5,"num_turns":1,"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("cost=$2.5"));
        assert!(!entries[0].content.contains("cost=$2.500000"));
    }

    #[test]
    fn parse_result_fallback_to_text() {
        let line = r#"{"type":"result","result":"done","timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Result);
        assert_eq!(entries[0].content, "done");
    }

    #[test]
    fn parse_snapshot_message() {
        let line =
            r#"{"type":"file-history-snapshot","messageId":"m","snapshot":{},"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Snapshot);
        assert_eq!(entries[0].content, "[git snapshot]");
    }

    #[test]
    fn parse_snapshot_update() {
        let line = r#"{"type":"file-history-snapshot","messageId":"m","isSnapshotUpdate":true,"snapshot":{},"timestamp":"T"}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::Snapshot);
        assert!(entries[0].content.contains("update"));
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json", "a", None).is_empty());
        assert!(parse_line("", "a", None).is_empty());
    }

    #[test]
    fn tool_result_error_flag() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"command not found","is_error":true}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_error);
    }

    #[test]
    fn tool_use_send_message_summary() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"SendMessage","input":{"to":"team-lead","summary":"task done","message":"completed"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "→ team-lead: task done");
    }

    #[test]
    fn parse_line_extracts_is_sidechain_true() {
        let line = r#"{"type":"user","isSidechain":true,"timestamp":"T","message":{"role":"user","content":"hello from subagent"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_sidechain);
    }

    #[test]
    fn parse_line_defaults_is_sidechain_false() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":"hello"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_sidechain);
    }

    #[test]
    fn is_sidechain_propagates_to_all_blocks() {
        let line = r#"{"type":"assistant","isSidechain":true,"timestamp":"T","message":{"role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_sidechain);
        assert!(entries[1].is_sidechain);
        assert_eq!(entries[0].message_type, EntryType::Assistant);
        assert_eq!(entries[1].message_type, EntryType::ToolUse);
    }

    #[test]
    fn is_sidechain_serialized_in_json() {
        let line = r#"{"type":"user","isSidechain":true,"timestamp":"T","message":{"role":"user","content":"x"}}"#;
        let entries = parse_line(line, "a", None);
        let json = serde_json::to_string(&entries[0]).unwrap();
        assert!(json.contains("\"is_sidechain\":true"));
    }

    #[test]
    fn parse_line_extracts_uuid_and_parent() {
        let line = r#"{"type":"user","uuid":"child-1","parentUuid":"parent-0","timestamp":"T","message":{"role":"user","content":"hi"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uuid.as_deref(), Some("child-1"));
        assert_eq!(entries[0].parent_uuid.as_deref(), Some("parent-0"));
    }

    #[test]
    fn parse_line_extracts_model_and_stop_reason() {
        let line = r#"{"type":"assistant","uuid":"a1","timestamp":"T","message":{"role":"assistant","model":"claude-sonnet-4-6","stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(entries[0].stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn parse_line_extracts_usage_with_cache_tokens() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","model":"claude","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":11,"output_tokens":22,"cache_creation_input_tokens":33,"cache_read_input_tokens":44}}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        let usage = entries[0].usage.as_ref().expect("usage expected");
        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.output_tokens, Some(22));
        assert_eq!(usage.cache_creation_input_tokens, Some(33));
        assert_eq!(usage.cache_read_input_tokens, Some(44));
    }

    #[test]
    fn parse_line_uuid_absent_is_none() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":"hi"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].uuid.is_none());
        assert!(entries[0].parent_uuid.is_none());
        assert!(entries[0].model.is_none());
        assert!(entries[0].stop_reason.is_none());
        assert!(entries[0].usage.is_none());
    }

    #[test]
    fn metadata_shared_across_all_blocks_from_same_line() {
        let line = r#"{"type":"assistant","uuid":"same-uuid","parentUuid":"parent","timestamp":"T","message":{"role":"assistant","model":"claude-sonnet-4-6","stop_reason":"tool_use","content":[{"type":"text","text":"thinking out loud"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}},{"type":"thinking","thinking":"hmm","signature":"s"}],"usage":{"input_tokens":1,"output_tokens":2}}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 3);
        for entry in &entries {
            assert_eq!(entry.uuid.as_deref(), Some("same-uuid"));
            assert_eq!(entry.parent_uuid.as_deref(), Some("parent"));
            assert_eq!(entry.model.as_deref(), Some("claude-sonnet-4-6"));
            assert_eq!(entry.stop_reason.as_deref(), Some("tool_use"));
            let usage = entry.usage.as_ref().expect("usage shared");
            assert_eq!(usage.input_tokens, Some(1));
            assert_eq!(usage.output_tokens, Some(2));
        }
    }

    #[test]
    fn parent_uuid_serialized_as_camel_case() {
        let line = r#"{"type":"user","uuid":"u1","parentUuid":"p0","timestamp":"T","message":{"role":"user","content":"x"}}"#;
        let entries = parse_line(line, "a", None);
        let json = serde_json::to_string(&entries[0]).unwrap();
        assert!(
            json.contains("\"parentUuid\":\"p0\""),
            "parentUuid should serialise in camelCase: {json}"
        );
    }

    #[test]
    fn user_text_block_with_system_reminder_tagged() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>stay focused</system-reminder>"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::User);
        assert_eq!(entries[0].tag, Some(TagKind::SystemReminder));
        assert!(entries[0].content.contains("<system-reminder>"));
    }

    #[test]
    fn user_image_block_emits_placeholder() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::User);
        assert_eq!(entries[0].content, "[image: image/png]");
    }

    #[test]
    fn user_document_block_emits_placeholder() {
        let line = r#"{"type":"user","timestamp":"T","message":{"role":"user","content":[{"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"..."},"filename":"paper.pdf"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_type, EntryType::User);
        assert_eq!(entries[0].content, "[document: paper.pdf]");
    }

    #[test]
    fn assistant_text_with_slash_command_tagged() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"text","text":"<command-name>/compact</command-name>"}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tag, Some(TagKind::SlashCommand));
    }

    #[test]
    fn parse_line_extracts_new_meta_fields() {
        // Exercise the A-bucket additions end-to-end: cwd, gitBranch,
        // sessionId, version, userType, isMeta, isApiErrorMessage,
        // requestId should all round-trip through parse_line.
        let line = r#"{"type":"user","uuid":"u1","sessionId":"s0","cwd":"/tmp/proj","gitBranch":"main","version":"2.0.47","userType":"agent","isMeta":true,"isApiErrorMessage":true,"requestId":"req-42","timestamp":"T","message":{"role":"user","content":"hi"}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.session_id.as_deref(), Some("s0"));
        assert_eq!(e.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(e.git_branch.as_deref(), Some("main"));
        assert_eq!(e.version.as_deref(), Some("2.0.47"));
        assert_eq!(e.user_type.as_deref(), Some("agent"));
        assert!(e.is_meta);
        assert!(e.is_api_error);
        assert_eq!(e.request_id.as_deref(), Some("req-42"));
    }

    #[test]
    fn tool_use_task_surfaces_worktree_and_background_flags() {
        let line = r#"{"type":"assistant","timestamp":"T","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Task","input":{"subagent_type":"explore","description":"scan","run_in_background":true,"isolation":"worktree"}}]}}"#;
        let entries = parse_line(line, "a", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "explore: scan [bg,worktree]");
    }
}
