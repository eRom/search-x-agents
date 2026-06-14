## Build & Install

```bash
cargo build --release && cp target/release/search-x-agents ~/.cargo/bin/
```

Le binaire est ensuite dispo partout : `search-x-agents "ma query"`

---

## References projet (_memory_)

Le dossier `_memory_/` contient la cartographie persistante du projet :
- `architecture.md` — vue d'ensemble, stack, flux de données
- `key-files.md` — fichiers critiques et leur rôle
- `patterns.md` — conventions et patterns récurrents
- `gotchas.md` — pièges, bugs résolus, workarounds

**Ne lis PAS ces fichiers au démarrage.** Lis-les à la demande, uniquement quand la question de l'utilisateur touche au domaine concerné.
