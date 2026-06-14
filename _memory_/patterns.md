# Patterns — search-cross-agents

**Dernière mise à jour : 2026-06-13**

## Pattern architectural : Trait + Modules

Chaque source implémente `SessionSource` dans son propre fichier. Pas d'abstraction superflue — le trait a 4 méthodes seulement. Les structs sont des unit structs (sauf `OpenClawSource` qui a un champ `agent`).

## Pattern de recherche : ripgrep + fallback Rust

```rust
fn search_*(...) -> Vec<DeepMatch> {
    if !is_ripgrep_available() {
        return search_*_rust(...);  // fallback
    }
    // ripgrep via Command::new("rg")
    // Si erreur → fallback
}
```

- Ripgrep est préféré pour la vitesse (3-5x)
- Le fallback Rust pur utilise `BufReader::lines()` + parsing manuel
- `OnceLock` utilisé pour les statics (`RIPGREP_AVAILABLE`, `RIPGREP_WARNING_SHOWN`)

## Pattern d'extraction : TextExtractor par source

Chaque source a sa propre fonction `extract_text_*` adaptée à son format JSON :
- `extract_text_claude(value) -> String` — `message.content[{text, tool_result}]`
- `extract_text_openclaw(value) -> (role, text)` — `message.content[{text}]`
- `extract_text_codex(value) -> (role, text)` — `payload.content[{input_text, output_text}]`
- `extract_text_cowork(value) -> (role, text)` — Anthropic API blocks (`text`, `tool_use`, `tool_result`)
- `extract_text_antigravity(value) -> String` — `content` + `tool_calls[].args`

## Convention de nommage

- Modules : snake_case (`claude.rs`, `openclaw.rs`)
- Structs : PascalCase (`ClaudeSource`, `OpenClawSource`)
- Fonctions internes : snake_case avec préfixe source (`search_claude`, `extract_text_codex`)
- Constantes : UPPER_SNAKE (`MAX_MATCHES_PER_SESSION`, `MAX_SNIPPET_LEN`)

## Gestion d'erreur

- Bypass silencieux : si `session_roots()` retourne vide → pas d'erreur, juste pas de résultats
- Ripgrep failure → fallback Rust avec warning stderr
- Fichier illisible → `continue` (skip silencieux)
- JSON invalide → `continue` (skip silencieux)

## Tests

- Intégration uniquement (pas de tests unitaires)
- Les tests lancent le binaire compilé via `Command`
- 15 tests dans `tests/integration_tests.rs`
