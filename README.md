# claude-compose

`docker compose logs` for Claude Code Agent Teams.

```
$ claude-compose logs -f
[08:46:49] team-lead  │ Let me break this into three parallel tasks...
[08:46:51] backend    │  Read: src/api/handlers.rs
[08:46:51] frontend   │  Glob: **/*.tsx
[08:46:52] backend    │ Found the issue — missing error handler on line 42
[08:46:53] frontend   │  → team-lead: Component tree analysis complete
[08:46:55] tests      │  Bash: cargo test
[08:46:58] tests      │  Task #3 → completed
```

## Install

```
cargo install claude-compose
```

## Commands

```bash
claude-compose logs -f                        # follow all agents
claude-compose logs -f backend frontend       # filter by agent
claude-compose logs --type assistant           # filter by message type
claude-compose logs --json | jq '.agent_name'  # pipe-friendly
claude-compose ps                              # agent status table
claude-compose ps --json                       # JSON output for scripting
claude-compose up                              # start team in tmux
claude-compose down                            # stop team
```

No hooks, no database, no web server. Reads `~/.claude/` directly.

## License

MIT
