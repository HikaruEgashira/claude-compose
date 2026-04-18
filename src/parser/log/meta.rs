use serde_json::Value;

use super::types::Usage;

/// Metadata shared by every LogEntry emitted from a single JSONL record.
///
/// Fields mirror Claude Code's JSONL schema (camelCase) so the reader can
/// stay a single `pointer(...)` deep. Optional fields default to `None`;
/// flag fields default to `false`.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecordMeta {
    pub is_sidechain: bool,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: Option<Usage>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    pub user_type: Option<String>,
    pub is_meta: bool,
    pub is_api_error: bool,
    pub request_id: Option<String>,
}

/// Read every piece of per-record metadata from a JSONL value so each
/// downstream LogEntry carries identical bookkeeping.
///
/// Missing fields stay `None`/`false` — we never fabricate values, so
/// downstream filters can tell "unknown" apart from "explicitly false".
pub(crate) fn extract_record_meta(v: &Value) -> RecordMeta {
    let is_sidechain = v
        .get("isSidechain")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let is_meta = v.get("isMeta").and_then(|b| b.as_bool()).unwrap_or(false);
    let is_api_error = v
        .get("isApiErrorMessage")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

    let uuid = str_field(v, "uuid");
    let parent_uuid = str_field(v, "parentUuid");
    let session_id = str_field(v, "sessionId");
    let cwd = str_field(v, "cwd");
    let git_branch = str_field(v, "gitBranch");
    let version = str_field(v, "version");
    let user_type = str_field(v, "userType");
    let request_id = str_field(v, "requestId");

    let model = v
        .pointer("/message/model")
        .and_then(|s| s.as_str())
        .map(String::from);
    let stop_reason = v
        .pointer("/message/stop_reason")
        .and_then(|s| s.as_str())
        .map(String::from);
    let usage = v.pointer("/message/usage").and_then(extract_usage);

    RecordMeta {
        is_sidechain,
        uuid,
        parent_uuid,
        model,
        stop_reason,
        usage,
        session_id,
        cwd,
        git_branch,
        version,
        user_type,
        is_meta,
        is_api_error,
        request_id,
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|s| s.as_str()).map(String::from)
}

/// Parse a `/message/usage` object into a `Usage` struct. Returns None if
/// every field is missing so we don't emit an empty usage blob.
pub(crate) fn extract_usage(v: &Value) -> Option<Usage> {
    let obj = v.as_object()?;
    let input_tokens = obj.get("input_tokens").and_then(|n| n.as_u64());
    let output_tokens = obj.get("output_tokens").and_then(|n| n.as_u64());
    let cache_creation_input_tokens = obj
        .get("cache_creation_input_tokens")
        .and_then(|n| n.as_u64());
    let cache_read_input_tokens = obj.get("cache_read_input_tokens").and_then(|n| n.as_u64());
    if input_tokens.is_none()
        && output_tokens.is_none()
        && cache_creation_input_tokens.is_none()
        && cache_read_input_tokens.is_none()
    {
        return None;
    }
    Some(Usage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens,
        cache_read_input_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_usage_returns_none_for_empty() {
        assert!(extract_usage(&json!({})).is_none());
    }

    #[test]
    fn extract_usage_preserves_cache_fields() {
        let u = extract_usage(&json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_creation_input_tokens": 3,
            "cache_read_input_tokens": 4,
        }))
        .unwrap();
        assert_eq!(u.input_tokens, Some(1));
        assert_eq!(u.cache_creation_input_tokens, Some(3));
        assert_eq!(u.cache_read_input_tokens, Some(4));
    }

    #[test]
    fn extract_record_meta_reads_all_fields() {
        let v = json!({
            "isSidechain": true,
            "isMeta": true,
            "isApiErrorMessage": true,
            "uuid": "u1",
            "parentUuid": "p0",
            "sessionId": "s0",
            "cwd": "/tmp",
            "gitBranch": "main",
            "version": "2.0.47",
            "userType": "agent",
            "requestId": "req-1",
            "message": {
                "model": "claude-sonnet",
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 10, "output_tokens": 20 }
            }
        });
        let m = extract_record_meta(&v);
        assert!(m.is_sidechain);
        assert!(m.is_meta);
        assert!(m.is_api_error);
        assert_eq!(m.uuid.as_deref(), Some("u1"));
        assert_eq!(m.parent_uuid.as_deref(), Some("p0"));
        assert_eq!(m.session_id.as_deref(), Some("s0"));
        assert_eq!(m.cwd.as_deref(), Some("/tmp"));
        assert_eq!(m.git_branch.as_deref(), Some("main"));
        assert_eq!(m.version.as_deref(), Some("2.0.47"));
        assert_eq!(m.user_type.as_deref(), Some("agent"));
        assert_eq!(m.request_id.as_deref(), Some("req-1"));
        assert_eq!(m.model.as_deref(), Some("claude-sonnet"));
        assert_eq!(m.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(m.usage.as_ref().unwrap().input_tokens, Some(10));
    }

    #[test]
    fn extract_record_meta_defaults_when_absent() {
        let m = extract_record_meta(&json!({}));
        assert!(!m.is_sidechain);
        assert!(!m.is_meta);
        assert!(!m.is_api_error);
        assert!(m.uuid.is_none());
        assert!(m.session_id.is_none());
        assert!(m.cwd.is_none());
    }
}
