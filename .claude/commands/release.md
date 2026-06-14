Release a new version of search-x-agents: bump version, tag, push, and create a GitHub Release.

## Usage

```
/release           # patch bump (0.7.0 → 0.7.1)
/release patch     # same
/release minor     # 0.7.0 → 0.8.0
/release major     # 0.7.0 → 1.0.0
```

## Procedure

### 0. Parse argument

The first word of `$ARGUMENTS` is the bump level. Default to `patch` if empty or missing. Valid values: `patch`, `minor`, `major`.

### 1. Pre-flight checks — STOP if any fails

Run each check. If any fails, report the issue and STOP.

```bash
# a) No unstaged changes
git diff --quiet || { echo "FAIL: unstaged changes"; exit 1; }

# b) No staged but uncommitted changes
git diff --cached --quiet || { echo "FAIL: staged changes not committed"; exit 1; }

# c) No unpushed commits on current branch
BRANCH=$(git branch --show-current)
git log origin/$BRANCH..HEAD --oneline | grep -q . && { echo "FAIL: unpushed commits on $BRANCH"; exit 1; }

# d) Remote is reachable and up to date
git fetch origin --quiet 2>&1 || { echo "FAIL: cannot reach remote"; exit 1; }
git diff --quiet $BRANCH origin/$BRANCH || { echo "FAIL: local not synced with remote"; exit 1; }
```

All checks passed → continue.

### 2. Read current version and compute new version

```bash
CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
echo "Current: $CURRENT"

IFS='.' read -r MAJ MIN PAT <<< "$CURRENT"

case "${1:-patch}" in
  major)
    NEW="$((MAJ + 1)).0.0"
    ;;
  minor)
    NEW="$MAJ.$((MIN + 1)).0"
    ;;
  patch)
    NEW="$MAJ.$MIN.$((PAT + 1))"
    ;;
  *)
    echo "FAIL: unknown bump level '${1:-patch}'. Use patch, minor, or major."
    exit 1
    ;;
esac

echo "New:     $NEW"
```

### 3. Bump Cargo.toml

Update the version field. Then run `cargo check` to update `Cargo.lock`.

```bash
sed -i '' "s/^version = \"$CURRENT\"/version = \"$NEW\"/" Cargo.toml
cargo check --quiet 2>&1
```

### 4. Commit the version bump

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to $NEW"
```

### 5. Tag and push

```bash
git tag "v$NEW"
git push origin "$(git branch --show-current)"
git push origin "v$NEW"
```

### 6. Create GitHub Release (non-draft)

Wait ~30 seconds for the tag to propagate, then create the release. The CI workflow (`release.yml`) will attach binaries once they're built.

```bash
gh release create "v$NEW" \
  --title "v$NEW" \
  --generate-notes \
  --draft=false \
  --latest
```

### 7. Monitor CI

Announce the release URL and suggest watching the CI run:

```
https://github.com/eRom/search-x-agents/releases/tag/v$NEW
https://github.com/eRom/search-x-agents/actions
```
