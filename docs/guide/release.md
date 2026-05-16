# Release

## Quick Start

```bash
# 1. Preview the entire release flow (no changes made)
cargo xtask release --dry-run

# 2. Run full checks before releasing
cargo xtask check

# 3. Execute the release (publish crates + git tag + push)
cargo xtask release

# Or publish crates separately
make publish                    # show publish plan
make publish orix_dry_run=0     # actually publish
```

## Release Flow

```
cargo xtask release
  → [1/4] cargo xtask check     (fmt + clippy + test + doc)
  → [2/4] cargo publish         (14 crates in topological order)
  → [3/4] git tag v0.1.0
  → [4/4] git push origin v0.1.0
```

Push of the tag triggers GitHub Actions (`release.yml`):

1. Runs CI checks on Linux / Windows / macOS (x64 + ARM64)
2. Builds release binaries for all platforms
3. Creates a GitHub Release with attached `.tar.gz` / `.zip` artifacts

## Options

| Flag | Effect |
|------|--------|
| `--dry-run` | Preview steps 2-4 without making changes |
| `--crates-only` | Only publish crates, skip git tag and push |
| `--skip-crates` | Only create and push git tag, skip crates.io publish |
| `--tag-prefix PREFIX` | Custom tag prefix (default: `v`) |

## Publishing to crates.io

Crates are published in strict dependency topological order:

```
domain → manifest → utils → registry → store → lockfile →
resolver → linker → workspace → fetcher → config → core → cli → macros
```

### Prerequisites

Set your crates.io token:

```bash
export CARGO_REGISTRY_TOKEN=<your-token>
```

Or use `cargo login <token>` once to store it in `~/.cargo/credentials.toml`.

### Dry Run (recommended)

```bash
# Show what would be published
cargo xtask publish-crates --dry-run
# or
make publish
```

### For Real

```bash
make publish orix_dry_run=0
```

## Changelog

Use [git-cliff](https://github.com/orf/git-cliff) for automated changelog generation:

```bash
cargo install git-cliff
git-cliff --config cliff.toml --repository https://github.com/baicie/orix
```

The GitHub Actions release workflow generates release notes automatically via `orhun/git-cliff-action`.
