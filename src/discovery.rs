use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::parser::{claude_home, cwd_to_project_key};

/// Per-session summary of TodoWrite state, bucketed by status.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TodoSummary {
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
}

impl TodoSummary {
    pub fn total(&self) -> usize {
        self.pending + self.in_progress + self.completed
    }
}

/// IDE attachment observed via a `~/.claude/ide/*.lock` file.
#[derive(Debug, Clone, Serialize, Default)]
pub struct IdeAttachment {
    pub ide_name: String,
    pub workspace: String,
}

/// Per-project artefact counts sourced from
/// `~/.claude/projects/<project_key>/{memory,file-history}/`.
///
/// Both trees are optional (older Claude Code installs don't create them)
/// so either count may be zero without implying absence of activity.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ProjectArtifacts {
    pub memory_files: usize,
    pub file_history_snapshots: usize,
}

impl ProjectArtifacts {
    pub fn is_empty(&self) -> bool {
        self.memory_files == 0 && self.file_history_snapshots == 0
    }
}

/// Summary of the harness-level configuration read from
/// `~/.claude/settings.json` (or a project-local settings file).
///
/// We capture just the shape needed by downstream tooling (`ps`): which
/// hook events are configured, and a count of permission rules. The raw
/// JSON is intentionally *not* surfaced — downstream code should read the
/// file directly if it needs the full shape.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SettingsSummary {
    /// Hook event names (e.g. `PreToolUse`, `PostToolUse`, `SessionStart`).
    /// Sorted deterministically so two runs on the same file produce the
    /// same ordering.
    pub hooks: Vec<String>,
    /// Number of entries under `permissions.allow`/`deny`/`ask` combined.
    pub permission_rules: usize,
}

impl SettingsSummary {
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty() && self.permission_rules == 0
    }
}

/// Derive the "session stem" from a todos filename.
/// Filenames may be `{session_id}.json` or `{session_id}-agent-{other_id}.json`.
/// We return the portion before the first `-agent-`, stripping the `.json` suffix
/// if present.
fn todos_session_stem(file_stem: &str) -> String {
    if let Some((before, _)) = file_stem.split_once("-agent-") {
        before.to_string()
    } else {
        file_stem.to_string()
    }
}

/// Read `~/.claude/todos/` and build a map from session_id → TodoSummary.
/// Filenames may be `{session_id}.json` or `{session_id}-agent-{other_id}.json`;
/// keys of the returned map are the leading UUID stem (before the first `-agent-`).
/// Returns an empty map on any IO error (best-effort discovery).
pub fn load_all_todos() -> HashMap<String, TodoSummary> {
    let Ok(claude) = claude_home() else {
        return HashMap::new();
    };
    load_all_todos_from(&claude.join("todos"))
}

/// Same as [`load_all_todos`] but reads from an explicit base directory.
/// Useful for tests that inject a temporary directory.
pub fn load_all_todos_from(base: &Path) -> HashMap<String, TodoSummary> {
    let mut out: HashMap<String, TodoSummary> = HashMap::new();

    let Ok(entries) = fs::read_dir(base) else {
        return out;
    };

    // Sort by filename so "first file wins" collisions resolve deterministically
    // regardless of filesystem enumeration order.
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let key = todos_session_stem(stem);

        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let Some(arr) = v.as_array() else {
            continue;
        };

        let mut summary = TodoSummary::default();
        for item in arr {
            let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
            match status {
                "pending" => summary.pending += 1,
                "in_progress" => summary.in_progress += 1,
                "completed" => summary.completed += 1,
                _ => {}
            }
        }

        // First file wins on collision so behaviour is deterministic.
        out.entry(key).or_insert(summary);
    }

    out
}

/// Read `~/.claude/ide/*.lock` and return a map from workspace path → IdeAttachment.
/// Returns an empty map on any IO error (best-effort discovery).
pub fn load_ide_attachments() -> HashMap<String, IdeAttachment> {
    let Ok(claude) = claude_home() else {
        return HashMap::new();
    };
    load_ide_attachments_from(&claude.join("ide"))
}

/// Same as [`load_ide_attachments`] but reads from an explicit base directory.
pub fn load_ide_attachments_from(base: &Path) -> HashMap<String, IdeAttachment> {
    let mut out: HashMap<String, IdeAttachment> = HashMap::new();

    let Ok(entries) = fs::read_dir(base) else {
        return out;
    };

    // Sort by filename so "first file wins" collisions resolve deterministically
    // regardless of filesystem enumeration order.
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&data) else {
            continue;
        };

        let ide_name = v
            .get("ideName")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();

        let workspaces: Vec<String> = v
            .get("workspaceFolders")
            .and_then(|w| w.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .map(normalise_workspace)
                    .collect()
            })
            .unwrap_or_default();

        for ws in workspaces {
            let att = IdeAttachment {
                ide_name: ide_name.clone(),
                workspace: ws.clone(),
            };
            // First lock wins on collision.
            out.entry(ws).or_insert(att);
        }
    }

    out
}

/// Count artefact files under the memory/ and file-history/ subdirectories
/// of a single project. Counts only the top level so large checkpoint
/// trees don't stall discovery; `ps` needs a rough signal, not a census.
pub fn load_project_artifacts(project_key: &str) -> ProjectArtifacts {
    let Ok(claude) = claude_home() else {
        return ProjectArtifacts::default();
    };
    load_project_artifacts_from(&claude.join("projects").join(project_key))
}

/// Same as [`load_project_artifacts`] but reads from an explicit project
/// directory (the caller provides `~/.claude/projects/<key>` itself).
pub fn load_project_artifacts_from(project_dir: &Path) -> ProjectArtifacts {
    ProjectArtifacts {
        memory_files: count_dir_entries(&project_dir.join("memory")),
        file_history_snapshots: count_dir_entries(&project_dir.join("file-history")),
    }
}

/// Look up artefacts by cwd. Convenience for the common call site in `ps`
/// which carries the team's cwd rather than the derived project key.
pub fn load_project_artifacts_for_cwd(cwd: &str) -> ProjectArtifacts {
    load_project_artifacts(&cwd_to_project_key(cwd))
}

/// Count entries (files + directories) directly under `dir`. Returns 0
/// when the directory is absent or unreadable — callers treat both as
/// "no artefacts" rather than reporting an error.
fn count_dir_entries(dir: &Path) -> usize {
    match fs::read_dir(dir) {
        Ok(entries) => entries.flatten().count(),
        Err(_) => 0,
    }
}

/// Read `~/.claude/settings.json` and extract the list of configured hook
/// event names plus a count of permission rules. Returns an empty summary
/// on any IO/parse failure (best-effort).
pub fn load_settings_summary() -> SettingsSummary {
    let Ok(claude) = claude_home() else {
        return SettingsSummary::default();
    };
    load_settings_summary_from(&claude.join("settings.json"))
}

/// Same as [`load_settings_summary`] but reads from an explicit path.
pub fn load_settings_summary_from(path: &Path) -> SettingsSummary {
    let Ok(data) = fs::read_to_string(path) else {
        return SettingsSummary::default();
    };
    let Ok(v) = serde_json::from_str::<Value>(&data) else {
        return SettingsSummary::default();
    };

    // `hooks` is `{"<EventName>": [...]}`. We surface the sorted list of
    // event names, which happens to be what `ps` wants to show.
    let mut hooks: Vec<String> = v
        .get("hooks")
        .and_then(|h| h.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    hooks.sort();

    let permission_rules = v
        .get("permissions")
        .and_then(|p| p.as_object())
        .map(|o| {
            ["allow", "deny", "ask"]
                .iter()
                .filter_map(|k| o.get(*k).and_then(|v| v.as_array()).map(|a| a.len()))
                .sum::<usize>()
        })
        .unwrap_or(0);

    SettingsSummary {
        hooks,
        permission_rules,
    }
}

/// Normalise a workspace path into an absolute string form when possible.
/// Falls back to the original string if canonicalisation fails (e.g. path does
/// not exist on this machine).
fn normalise_workspace(s: &str) -> String {
    let p = PathBuf::from(s);
    match fs::canonicalize(&p) {
        Ok(c) => c.to_string_lossy().into_owned(),
        Err(_) => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_all_todos_empty_dir_returns_empty() {
        let dir = std::env::temp_dir().join("cc-test-todos-empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let map = load_all_todos_from(&dir);
        assert!(map.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_todos_returns_empty_on_missing_dir() {
        // Best-effort: missing dir must not error.
        let dir = std::env::temp_dir().join("cc-test-todos-missing-xyz-never-created");
        let _ = fs::remove_dir_all(&dir);

        let map = load_all_todos_from(&dir);
        assert!(map.is_empty());
    }

    #[test]
    fn load_all_todos_counts_by_status() {
        let dir = std::env::temp_dir().join("cc-test-todos-counts");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Plain `{session}.json`
        let session_a = "11111111-2222-3333-4444-555555555555";
        let content_a = r#"[
            {"content":"step 1","status":"pending","activeForm":"Doing step 1","id":"1"},
            {"content":"step 2","status":"in_progress","activeForm":"Doing step 2","id":"2"},
            {"content":"step 3","status":"completed","activeForm":"Done step 3","id":"3"},
            {"content":"step 4","status":"pending","activeForm":"Doing step 4","id":"4"}
        ]"#;
        fs::write(dir.join(format!("{session_a}.json")), content_a).unwrap();

        // `{session}-agent-{other}.json` — key must be the leading stem.
        let session_b = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let agent_id = "ffffffff-0000-0000-0000-000000000000";
        let content_b = r#"[
            {"content":"do thing","status":"completed","activeForm":"","id":"1"},
            {"content":"unknown bucket","status":"weird","activeForm":"","id":"2"}
        ]"#;
        fs::write(
            dir.join(format!("{session_b}-agent-{agent_id}.json")),
            content_b,
        )
        .unwrap();

        // Malformed file is ignored.
        fs::write(dir.join("bad.json"), "not json").unwrap();
        // Non-array JSON is ignored.
        fs::write(dir.join("object.json"), r#"{"foo":1}"#).unwrap();
        // Non-.json file is ignored.
        fs::write(dir.join("readme.txt"), "hi").unwrap();

        let map = load_all_todos_from(&dir);

        let a = map
            .get(session_a)
            .unwrap_or_else(|| panic!("missing session_a; map = {map:?}"));
        assert_eq!(a.pending, 2);
        assert_eq!(a.in_progress, 1);
        assert_eq!(a.completed, 1);
        assert_eq!(a.total(), 4);

        let b = map
            .get(session_b)
            .unwrap_or_else(|| panic!("missing session_b; map = {map:?}"));
        assert_eq!(b.pending, 0);
        assert_eq!(b.in_progress, 0);
        assert_eq!(b.completed, 1);
        assert_eq!(b.total(), 1);

        // Malformed / non-array / non-json entries must not have produced keys.
        assert!(!map.contains_key("bad"));
        assert!(!map.contains_key("object"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_ide_attachments_from_base() {
        let dir = std::env::temp_dir().join("cc-test-ide-locks");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Use the temp dir itself as a workspace so canonicalize succeeds.
        let ws = dir.to_string_lossy().into_owned();

        let lock1 = format!(
            r#"{{"pid":12345,"workspaceFolders":["{ws}"],"ideName":"VS Code","transport":"ws"}}"#
        );
        fs::write(dir.join("12345.lock"), &lock1).unwrap();

        // Second lock with same workspace — first must win.
        let lock2 = format!(
            r#"{{"pid":99999,"workspaceFolders":["{ws}"],"ideName":"Cursor","transport":"ws"}}"#
        );
        fs::write(dir.join("99999.lock"), &lock2).unwrap();

        // Non-.lock file ignored.
        fs::write(dir.join("not-a-lock.json"), r#"{"ideName":"X"}"#).unwrap();

        // Malformed lock ignored.
        fs::write(dir.join("bad.lock"), "not json").unwrap();

        let canonical = fs::canonicalize(&dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let map = load_ide_attachments_from(&dir);

        let att = map
            .get(&canonical)
            .unwrap_or_else(|| panic!("expected attachment for {canonical}; map = {map:?}"));
        assert_eq!(att.ide_name, "VS Code");
        assert_eq!(att.workspace, canonical);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_ide_attachments_returns_empty_on_missing_dir() {
        let dir = std::env::temp_dir().join("cc-test-ide-missing-xyz-never-created");
        let _ = fs::remove_dir_all(&dir);
        let map = load_ide_attachments_from(&dir);
        assert!(map.is_empty());
    }

    #[test]
    fn project_artifacts_counts_top_level_entries() {
        let dir = std::env::temp_dir().join("cc-test-project-artifacts");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("memory")).unwrap();
        fs::create_dir_all(dir.join("file-history")).unwrap();

        fs::write(dir.join("memory/notes.md"), "hello").unwrap();
        fs::write(dir.join("memory/goals.md"), "x").unwrap();
        fs::create_dir_all(dir.join("file-history/abc")).unwrap();
        fs::create_dir_all(dir.join("file-history/def")).unwrap();
        fs::create_dir_all(dir.join("file-history/ghi")).unwrap();

        let art = load_project_artifacts_from(&dir);
        assert_eq!(art.memory_files, 2);
        assert_eq!(art.file_history_snapshots, 3);
        assert!(!art.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_artifacts_absent_dirs_return_zero() {
        let dir = std::env::temp_dir().join("cc-test-project-artifacts-missing");
        let _ = fs::remove_dir_all(&dir);
        let art = load_project_artifacts_from(&dir);
        assert_eq!(art.memory_files, 0);
        assert_eq!(art.file_history_snapshots, 0);
        assert!(art.is_empty());
    }

    #[test]
    fn settings_summary_extracts_hook_names_and_counts() {
        let dir = std::env::temp_dir().join("cc-test-settings-summary");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        fs::write(
            &path,
            r#"{
                "hooks": {
                    "PreToolUse": [{"matcher": "Bash"}],
                    "PostToolUse": [],
                    "SessionStart": [{"command": "echo hi"}]
                },
                "permissions": {
                    "allow": ["Bash", "Read"],
                    "deny": ["Write"],
                    "ask": []
                }
            }"#,
        )
        .unwrap();

        let s = load_settings_summary_from(&path);
        assert_eq!(s.hooks, vec!["PostToolUse", "PreToolUse", "SessionStart"]);
        assert_eq!(s.permission_rules, 3);
        assert!(!s.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_summary_missing_file_is_empty() {
        let s = load_settings_summary_from(Path::new(
            "/tmp/cc-settings-never-exists-xyz/settings.json",
        ));
        assert!(s.is_empty());
    }

    #[test]
    fn settings_summary_malformed_is_empty() {
        let dir = std::env::temp_dir().join("cc-test-settings-malformed");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, "not json").unwrap();
        assert!(load_settings_summary_from(&path).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn todos_session_stem_splits_agent_suffix() {
        assert_eq!(todos_session_stem("abc"), "abc");
        assert_eq!(todos_session_stem("abc-agent-xyz"), "abc");
        assert_eq!(
            todos_session_stem("11111111-2222-3333-4444-555555555555"),
            "11111111-2222-3333-4444-555555555555"
        );
    }
}
