# claude-compose

`docker compose logs` for Claude Code Agent Teams.

See what every agent is doing, in real time, from one terminal.

```
$ claude-compose logs -f
[08:46:49] team-lead  │ Let me break this into three parallel tasks...
[08:46:51] backend    │ 🔧 Read: src/api/handlers.rs
[08:46:51] frontend   │ 🔧 Glob: **/*.tsx
[08:46:52] backend    │ Found the issue — missing error handler on line 42
[08:46:53] frontend   │ 📨 → team-lead: Component tree analysis complete
[08:46:55] tests      │ 🔧 Bash: cargo test
[08:46:58] tests      │ ✅ Task #3 completed
```

## Why not `tail -f | jq`?

- **Multiple agents, one view** — Agent Team logs are scattered across `~/.claude/projects/*/subagents/*.jsonl`. claude-compose finds and merges them automatically.
- **Streaming** — `tail -f` doesn't follow multiple files with interleaved timestamps. claude-compose does, sorted chronologically.
- **Human-readable** — Raw JSONL is walls of nested JSON. claude-compose shows agent names, color-coded output, tool calls with icons, and truncated results.
- **Filter & focus** — Show only specific agents or message types (`--type tool_use`). Pipe JSON output (`--json`) to your own tools.

## Install

```bash
cargo install claude-compose
```

Or build from source:

```bash
git clone https://github.com/HikaruEgashira/claude-compose.git
cd claude-compose
cargo install --path .
```

## Usage

### Stream logs

```bash
# Show last 50 lines from the active team
claude-compose logs

# Follow in real time (like docker compose logs -f)
claude-compose logs -f

# Follow specific agents only
claude-compose logs -f backend frontend

# Filter by message type
claude-compose logs -f --type tool_use

# JSON output for piping
claude-compose logs --json | jq '.agent_name'

# Specify team explicitly
claude-compose logs -f --team my-compiler-team
```

### Show agent status

```bash
$ claude-compose ps
Team: my-project
NAME                 STATUS     TASK
--------------------------------------------------
team-lead            active     Coordinating implementation
backend              active     Implementing API endpoints
frontend             idle       -
tests                active     Running integration tests
```

## How it works

claude-compose reads the JSONL transcript files that Claude Code Agent Teams write to `~/.claude/`. It:

1. Discovers team config from `~/.claude/teams/{name}/config.json`
2. Finds all agent session files (team lead + subagents)
3. Parses each JSONL line into structured log entries
4. Merges, sorts by timestamp, and renders with color-coded agent names
5. In `-f` mode, watches files for changes and streams new entries

No hooks, no database, no web server. Just reads the files Claude Code already writes.

## Requirements

- Claude Code with Agent Teams enabled (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=true`)
- An active or completed Agent Team session
- Rust 1.85+ (for building from source)

## Contributing & Feedback

This is an early-stage project. Feedback from real Agent Team users is invaluable.

- **Bug reports & feature requests**: [GitHub Issues](https://github.com/HikaruEgashira/claude-compose/issues)
- **Questions & discussion**: [GitHub Discussions](https://github.com/HikaruEgashira/claude-compose/discussions)
- **X/Twitter**: Share your experience with `#claudecompose`

If claude-compose saves you even one `cat | jq` pipeline, consider giving it a star.

## License

MIT
