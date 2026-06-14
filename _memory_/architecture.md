# Architecture — search-cross-agents

**Dernière mise à jour : 2026-06-13**

## Type

CLI Rust — recherche full-text unifiée dans l'historique de sessions de 6 agents IA.

## Stack

- Rust edition 2024 (pas 2026 — non supporté par stable 1.92)
- clap 4 (derive), serde/serde_json, chrono, glob, dirs
- ripgrep (`rg`) en binaire externe pour la recherche rapide, fallback Rust pur

## Arborescence clé

```
src/
  main.rs              # CLI (Clap) + dispatch + output (310 lignes)
  sources/
    mod.rs             # Trait SessionSource, DeepMatch, helpers partagés
    claude.rs          # ~/.claude/projects/
    openclaw.rs        # ~/.openclaw/agents/<agent>/sessions/
    pi.rs              # ~/.pi/agent/sessions/ (même format qu'OpenClaw)
    codex.rs           # ~/.codex/sessions/YYYY/MM/DD/
    antigravity.rs     # ~/.gemini/antigravity*/brain/
    cowork.rs          # ~/Library/Application Support/Claude/local-agent-mode-sessions/
tests/
  integration_tests.rs # Tests d'intégration (15 tests)
```

## Flux

1. `main()` parse CLI → détermine quelles sources activer
2. `build_sources()` instancie les structs implémentant `SessionSource`
3. `search_all_sources()` itère sur chaque source → `source.search()`
4. Chaque source utilise ripgrep (ou fallback Rust) sur ses fichiers JSONL
5. Résultats mergés, triés par timestamp descendant, formatés

## Trait central

```rust
trait SessionSource {
    fn name(&self) -> &str;
    fn session_roots(&self) -> Vec<PathBuf>;  // vide si répertoire absent
    fn search(&self, query, project_filter, date_range) -> Vec<DeepMatch>;
    fn resume_command(&self, session_id, project_path) -> Option<String>;
}
```

## Dispatch

- Aucun flag → tous les agents dont le répertoire existe (bypass silencieux)
- Un ou plusieurs flags → uniquement ceux listés
- `--agent` ne s'applique qu'à OpenClaw, ignoré pour les autres
