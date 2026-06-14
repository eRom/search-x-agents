# Gotchas — search-cross-agents

**Dernière mise à jour : 2026-06-13**

## Ripgrep + dossiers cachés (Antigravity)

**Problème :** Antigravity retournait 0 résultats.
**Cause racine :** ripgrep ignore les fichiers/dossiers cachés (`.system_generated/`) par défaut.
**Fix :** Ajouter `--hidden` aux args ripgrep dans `antigravity.rs`.
**Commit :** `aafcd0a`

## Edition Rust 2026 non supportée

**Problème :** `Cargo.toml` avait `edition = "2026"`.
**Cause :** Rust stable 1.92 ne supporte que l'edition 2024 max.
**Fix :** Remplacer par `edition = "2024"`.

## Modules non déclarés dans mod.rs

**Problème :** `cargo check` échouait — `codex`, `antigravity`, `cowork` non trouvés.
**Cause :** Oubli des déclarations `pub mod codex; pub mod antigravity; pub mod cowork;` dans `src/sources/mod.rs`.
**Fix :** Ajouter les 3 déclarations manquantes.

## Import `BufRead` manquant dans les modules

**Problème :** `no method named 'lines'` dans codex.rs, antigravity.rs, cowork.rs.
**Cause :** `use std::io::BufRead;` absent — `BufReader::lines()` nécessite le trait en scope.
**Fix :** Ajouter l'import dans les 3 modules.

## Extraction tool_result dans Claude Code

**Problème :** `extract_content_array` dans `mod.rs` extrait `tool_result.content` via `.to_string()` — peut produire du JSON brut non lisible plutôt que le texte.
**Impact :** Les snippets contenant des tool_results peuvent être du JSON brut.
**Non corrigé** — bug préexistant, pas bloquant pour le search.

## Worktree EnterWorktree ne fonctionne pas

**Problème :** `EnterWorktree` retourne "not in a git repository" même quand le projet est un repo git valide.
**Workaround :** Créer le worktree manuellement avec `git worktree add` et travailler dedans avec des chemins absolus.
