# search-x-agents

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Search across all your AI agent session histories. Fast. IA-friendly output.

## Why?

Claude Code forgets. Codex forgets. Pi forgets. After a few sessions, you've lost context — that clever regex, the architecture decision, the API you debugged at 2am.

Your session history is still there, buried in `~/.claude/projects/`, `~/.codex/sessions/`, `~/.gemini/antigravity/`. But good luck finding anything across terabytes of JSONL files.

**search-x-agents** fixes that. One binary. No indexing step. No database. Sub-second search across 7 agents.

## Supported Agents

| Agent | Source flag | Session path |
|---|---|---|
| Antigravity | `--antigravity` | `~/.gemini/antigravity*/brain/` |
| Claude Code | `--claude` | `~/.claude/projects/` |
| Codex | `--codex` | `~/.codex/sessions/` |
| Cowork | `--cowork` | `~/Library/Application Support/Claude/local-agent-mode-sessions/` |
| Hermes | `--hermes` | `~/.hermes/state.db` |
| OpenClaw | `--openclaw` | `~/.openclaw/agents/<agent>/sessions/` |
| Pi | `--pi` | `~/.pi/agent/sessions/` |

If no source flag is passed, all available agents are searched.

## Output Format

YAML, designed to be parsed by LLMs:

```yaml
cross_agent_search:
  query: "tmux pane"
  total_matches: 35
  top_results:
    - id: 1
      source: Claude Code
      project: ~/dev/caserne-net
      date: 2026-06-13T14:17:00Z
      session: 8f4cf72f-5555-5555-b891-3d7b9e4c20de
      snippet: "...Layout TMUX Panes\n\n**Skill**: skills/net-autolayout/SKILL.md..."
```

## Quick Start

### Build from source

```bash
git clone https://github.com/eRom/search-x-agents.git
cd search-x-agents
cargo build --release
```

Requires Rust (any recent stable toolchain). Optional: install [ripgrep](https://github.com/BurntSushi/ripgrep) for 3-5x faster search.

## Usage

```bash
# Search across all agents
search-x-agents "kubernetes RBAC"

# Search a specific agent
search-x-agents --claude "docker compose"

# Filter by project
search-x-agents "auth" --project myapp

# Limit results (default: 20)
search-x-agents "deploy" --limit 10

# Filter by date range
search-x-agents "deploy" --since "3 days ago"
search-x-agents "auth" --since 2026-02-01 --until 2026-02-15
search-x-agents "bug" --date today
search-x-agents "refactor" --since "last week"
```

## Options
| args | description |
| --- | --- |
| --claude             | Search Claude Code sessions |
| --openclaw           | Search OpenClaw sessions |
| --codex              | Search Codex sessions |
| --pi                 | Search Pi sessions |
| --antigravity        | Search Antigravity sessions |
| --cowork             | Search Cowork sessions |
| --hermes             | Search Hermes sessions |
| --limit <LIMIT>      | Maximum results to show [default: 20] |
| --project <PROJECT>  | Filter to sessions from projects matching this substring |
| --agent <AGENT>      | OpenClaw agent to search (default: main) [default: main] |
| --since <SINCE>      | Filter results from this date/time |
| --until <UNTIL>      | Filter results until this date/time |
| --date <DATE>        | Shorthand for --since <date> --until <date+1day> |


## Docs

- [Architecture](_memory_/architecture.md)
- [Key Files](_memory_/key-files.md)
- [Patterns](_memory_/patterns.md)
- [Gotchas](_memory_/gotchas.md)
- [Changelog](CHANGELOG.md)

## License

MIT
