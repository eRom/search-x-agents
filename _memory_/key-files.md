# Key Files — search-cross-agents

**Dernière mise à jour : 2026-06-13**

| Fichier | Rôle |
|---------|------|
| `src/main.rs` | CLI (clap 6 flags agent), `DateRange`, `build_sources()`, `print_results()`, `main()` |
| `src/sources/mod.rs` | Trait `SessionSource`, struct `DeepMatch`, helpers (`get_snippet`, `matches_all_terms`, `extract_content_array`, `parse_rg_line`, `find_jsonl_files`, `is_ripgrep_available`), `search_all_sources()` dispatch |
| `src/sources/claude.rs` | `ClaudeSource` — format `{type, message.content[{text}]}`, session dans `sessionId`, cwd dans `cwd` |
| `src/sources/openclaw.rs` | `OpenClawSource` — format `{type: "message", message: {role, content}}`, header `type: "session"` pour métadonnées |
| `src/sources/pi.rs` | `PiSource` — même format qu'OpenClaw, sessions dans `--<project>--/` sous-dossiers |
| `src/sources/codex.rs` | `CodexSource` — format `{timestamp, type, payload}`, `response_item` avec `payload.content[{input_text, output_text}]`, session_meta pour cwd |
| `src/sources/antigravity.rs` | `AntigravitySource` — 3 roots (`antigravity/brain/`, `antigravity-ide/brain/`, `antigravity-cli/brain/`), `transcript*.jsonl` dans `.system_generated/logs/` |
| `src/sources/cowork.rs` | `CoworkSource` — `audit.jsonl` format Anthropic API, métadonnées dans `local_<UUID>.json` |
| `Cargo.toml` | Package `search-cross-agents` v0.3.0, edition 2024 |
| `tests/integration_tests.rs` | 15 tests : parsing, CLI flags, date range, help output |
