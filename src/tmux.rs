use std::process::Command;

use crate::cli::{DownOpts, UpOpts};

fn session_name(path: &str) -> anyhow::Result<String> {
    let abs = std::fs::canonicalize(path)
        .map_err(|e| anyhow::anyhow!("invalid path '{}': {}", path, e))?;
    let base = abs
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("cannot determine basename of '{}'", path))?
        .to_string_lossy();
    Ok(format!("dev-{base}"))
}

pub fn run_up(opts: UpOpts) -> anyhow::Result<()> {
    let session = session_name(&opts.path)?;
    let abs = std::fs::canonicalize(&opts.path)?;

    // Attach if session already exists
    if Command::new("tmux")
        .args(["has-session", "-t", &session])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        let status = Command::new("tmux")
            .args(["attach", "-t", &session])
            .status()?;
        std::process::exit(status.code().unwrap_or(0));
    }

    // Create new session, start claude, attach
    let new = Command::new("tmux")
        .args(["new-session", "-d", "-s", &session, "-c", &abs.to_string_lossy()])
        .output()?;
    if !new.status.success() {
        anyhow::bail!(
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&new.stderr)
        );
    }

    let send = Command::new("tmux")
        .args(["send-keys", "-t", &session, "claude", "Enter"])
        .output()?;
    if !send.status.success() {
        anyhow::bail!(
            "tmux send-keys failed: {}",
            String::from_utf8_lossy(&send.stderr)
        );
    }

    let status = Command::new("tmux")
        .args(["attach", "-t", &session])
        .status()?;
    std::process::exit(status.code().unwrap_or(0));
}

pub fn run_down(opts: DownOpts) -> anyhow::Result<()> {
    let session = session_name(&opts.path)?;

    let output = Command::new("tmux")
        .args(["kill-session", "-t", &session])
        .output()?;
    if output.status.success() {
        eprintln!("killed: {session}");
    } else {
        eprintln!("no session: {session}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_from_current_dir() {
        let name = session_name(".").unwrap();
        assert!(name.starts_with("dev-"));
        assert!(!name.contains('/'));
    }

    #[test]
    fn session_name_invalid_path() {
        assert!(session_name("/nonexistent/path/xyz").is_err());
    }
}
