# Release

## Quick Start

```bash
# 1. Preview the release plan (no changes made)
cargo xtask release --dry-run

# 2. Run full checks before releasing
cargo xtask check

# 3. Execute the release
cargo xtask release
```

## Release Flow

```
cargo xtask release [--version X.Y.Z] [--dry-run] [--crates-only] [--skip-crates] [--force]
  → [1/5] cargo xtask check      (fmt + clippy + test)
  → [2/5] cargo publish          (14 crates in topological order)
  → [3/5] git tag vX.Y.Z
  → [4/5] git push origin vX.Y.Z
  → [5/5] GitHub Release artifacts
```

Push of the tag triggers GitHub Actions (`release.yml`):

1. CI checks on Linux / Windows / macOS
2. Builds release binaries for all platforms
3. Creates GitHub Release with attached `.tar.gz` / `.zip` artifacts

## Options

| Flag | Effect |
|------|--------|
| `--dry-run` | Preview all steps without making changes |
| `--crates-only` | Only publish crates, skip git tag and push |
| `--skip-crates` | Only create and push git tag, skip crates.io publish |
| `--force` | Yank existing crates at this version first, then re-publish |
| `--version X.Y.Z` | Override version from Cargo.toml (also sets the git tag) |
| `--tag-prefix PREFIX` | Custom tag prefix (default: `v`) |

## Version Override

By default, version is read from `version` in `Cargo.toml`.

```bash
# Release 0.2.0 (updates Cargo.toml and all crate versions)
cargo xtask release --version 0.2.0

# Preview what would happen
cargo xtask release --dry-run --version 0.2.0
```

`--version` validates semver format and updates all crate `Cargo.toml` files.

## Publishing to crates.io

Crates are published in strict dependency topological order:

```
domain → manifest → utils → registry → store → lockfile →
resolver → linker → workspace → fetcher → config → core → cli → macros
```

### Prerequisites

```bash
export CARGO_REGISTRY_TOKEN=<your-token>
```

### Show Publish Plan

```bash
cargo xtask publish-crates --dry-run
# or
make publish
```

### Publish for Real

```bash
cargo xtask publish-crates
# or
make publish orix_dry_run=0
```

### Force Re-publish Same Version

When you need to re-publish the same version (e.g. security fix):

```bash
# Step 1: yank the existing version
cargo xtask yank 0.1.0
# or specific crates
cargo xtask yank 0.1.0 --crates orix-cli --crates orix-core

# Step 2: re-publish
cargo xtask publish-crates --force --version 0.1.0
# or the shorthand in release
cargo xtask release --force --version 0.1.0
```

The `--force` flag combines both steps automatically.

## Changelog

Use [git-cliff](https://github.com/orf/git-cliff) for automated changelog:

```bash
cargo install git-cliff
git-cliff --config cliff.toml --repository https://github.com/baicie/orix
```

GitHub Actions release workflow generates release notes automatically via `orhun/git-cliff-action`.
