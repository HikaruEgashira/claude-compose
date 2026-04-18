use super::types::TagKind;

/// Map an XML-like tag name (without angle brackets) to its `TagKind`.
/// Returns `None` for any tag we don't explicitly recognise, so unrelated
/// HTML-ish content in messages doesn't accidentally get tagged.
///
/// Hooks v2 adds many new `*-hook` variants (stop, subagent-stop,
/// session-start/end, pre/post-compact, notification, permission-denied,
/// cwd-changed, file-changed, task-created); they all collapse onto
/// `TagKind::Hook`.
pub fn classify_tag(tag_name: &str) -> Option<TagKind> {
    // Generic `*-hook` catch-all. Covers both v1 (user-prompt-submit,
    // pre/post-tool-use) and v2 (stop/subagent-stop/session-*/compact/
    // notification/permission-denied/cwd-changed/file-changed/
    // task-created, plus any future additions) without needing to list
    // every variant.
    if tag_name.ends_with("-hook") {
        return Some(TagKind::Hook);
    }
    match tag_name {
        "command-name" | "command-message" | "command-args" => Some(TagKind::SlashCommand),
        "system-reminder" => Some(TagKind::SystemReminder),
        "ide-selection" | "ide-diagnostic" | "ide-opened-files" => Some(TagKind::Ide),
        "local-command-stdout"
        | "local-command-stderr"
        | "bash-input"
        | "bash-stdout"
        | "bash-stderr" => Some(TagKind::Bash),
        "github-webhook-activity" => Some(TagKind::GitHubActivity),
        "available-skills"
        | "user-memory"
        | "current-branch"
        | "current-working-directory"
        | "current-cwd"
        | "env" => Some(TagKind::Env),
        _ => None,
    }
}

/// Scan `text` for the first recognised opening tag and return
/// `(kind, tag_name)`. The scan is allocation-free: we walk each `<...>`
/// occurrence and compare the raw name against the known set.
///
/// Only *opening* tags are matched (i.e. not `</foo>` and not self-closing
/// `<foo/>`), and only tags whose name contains alphanum/`-`/`_` characters.
pub fn detect_first_tag(text: &str) -> Option<(TagKind, &str)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Advance to the next '<'.
        let rel = text[i..].find('<')?;
        let start = i + rel;
        let after = start + 1;
        if after >= bytes.len() {
            return None;
        }
        // Skip closing tags `</…>` — we only care about openings.
        if bytes[after] == b'/' {
            i = after + 1;
            continue;
        }
        // Find the closing '>' of this tag.
        let rel_end = text[after..].find('>')?;
        let end = after + rel_end;
        let raw_name = &text[after..end];
        // A recognised tag name is ASCII lower-case with '-'; strip any
        // attribute suffix by splitting on whitespace.
        let name = raw_name
            .split(|c: char| c.is_ascii_whitespace())
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if let Some(kind) = classify_tag(name) {
            return Some((kind, name));
        }
        i = end + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_tag_recognises_known_tags() {
        assert_eq!(classify_tag("command-name"), Some(TagKind::SlashCommand));
        assert_eq!(classify_tag("command-message"), Some(TagKind::SlashCommand));
        assert_eq!(classify_tag("command-args"), Some(TagKind::SlashCommand));
        assert_eq!(classify_tag("user-prompt-submit-hook"), Some(TagKind::Hook));
        assert_eq!(classify_tag("pre-tool-use-hook"), Some(TagKind::Hook));
        assert_eq!(classify_tag("post-tool-use-hook"), Some(TagKind::Hook));
        assert_eq!(
            classify_tag("system-reminder"),
            Some(TagKind::SystemReminder)
        );
        assert_eq!(classify_tag("ide-selection"), Some(TagKind::Ide));
        assert_eq!(classify_tag("ide-diagnostic"), Some(TagKind::Ide));
        assert_eq!(classify_tag("local-command-stdout"), Some(TagKind::Bash));
        assert_eq!(classify_tag("bash-input"), Some(TagKind::Bash));
        assert_eq!(classify_tag("bash-stdout"), Some(TagKind::Bash));
        assert_eq!(classify_tag("bash-stderr"), Some(TagKind::Bash));
        assert_eq!(classify_tag("unknown-thing"), None);
        assert_eq!(classify_tag(""), None);
    }

    #[test]
    fn classify_tag_covers_v2_hooks() {
        for t in [
            "stop-hook",
            "subagent-stop-hook",
            "session-start-hook",
            "session-end-hook",
            "pre-compact-hook",
            "post-compact-hook",
            "notification-hook",
            "permission-denied-hook",
            "cwd-changed-hook",
            "file-changed-hook",
            "task-created-hook",
        ] {
            assert_eq!(classify_tag(t), Some(TagKind::Hook), "tag {t}");
        }
    }

    #[test]
    fn classify_tag_new_special_kinds() {
        assert_eq!(
            classify_tag("github-webhook-activity"),
            Some(TagKind::GitHubActivity)
        );
        assert_eq!(classify_tag("available-skills"), Some(TagKind::Env));
        assert_eq!(classify_tag("user-memory"), Some(TagKind::Env));
        assert_eq!(classify_tag("current-branch"), Some(TagKind::Env));
        assert_eq!(
            classify_tag("current-working-directory"),
            Some(TagKind::Env)
        );
    }

    #[test]
    fn detect_first_tag_returns_none_for_untagged_text() {
        assert!(detect_first_tag("plain text with no tags").is_none());
        // Unknown tag — must not be classified.
        assert!(detect_first_tag("<foo>body</foo>").is_none());
        // Closing tag alone — must not be matched.
        assert!(detect_first_tag("</system-reminder>").is_none());
    }

    #[test]
    fn detect_first_tag_finds_system_reminder() {
        let (kind, name) =
            detect_first_tag("prefix <system-reminder>be nice</system-reminder> suffix").unwrap();
        assert_eq!(kind, TagKind::SystemReminder);
        assert_eq!(name, "system-reminder");
    }

    #[test]
    fn detect_first_tag_returns_first_match() {
        let (kind, _) = detect_first_tag(
            "<command-name>/foo</command-name><system-reminder>x</system-reminder>",
        )
        .unwrap();
        assert_eq!(kind, TagKind::SlashCommand);
    }

    #[test]
    fn detect_first_tag_finds_github_activity() {
        let (kind, _) =
            detect_first_tag("<github-webhook-activity>push on main</github-webhook-activity>")
                .unwrap();
        assert_eq!(kind, TagKind::GitHubActivity);
    }

    #[test]
    fn detect_first_tag_finds_env_injection() {
        let (kind, _) =
            detect_first_tag("<available-skills>update-config</available-skills>").unwrap();
        assert_eq!(kind, TagKind::Env);
    }
}
