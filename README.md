# <img src="packaging/appimage/orix.png" width="48" height="48" style="vertical-align: middle;" /> orix

High-performance package manager written in Rust, inspired by pnpm's isolated layout approach.

[![build](https://github.com/baicie/orix/actions/workflows/ci.yml/badge.svg)](https://github.com/baicie/orix/actions/workflows/ci.yml)
[![license](https://img.shields.io/github/license/baicie/orix)](LICENSE)
[![Rust MSRV 1.80](https://img.shields.io/badge/Rust-1.80%2B-blue?logo=rust)](https://www.rust-lang.org)

## Features

- **Global CAS Cache** — Content-addressable storage, tarball files reused across projects
- **Orix Virtual Store** — Generates `node_modules/.orix` structure with workspace protocol support
- **Fast Installation** — Concurrent downloads + file-level deduplication + hard links
- **Lockfile** — Reproducible installs, supports `--frozen-lockfile` for CI verification
- **Workspace Support** — Monorepo multi-package management with `workspace:*`, `workspace:^`, `workspace:~`, `workspace:>=`, `workspace:file:` protocol variants
- **Cross-Platform** — Linux, macOS, Windows (with junction fallback)

## Installation

### Build from source

```bash
cargo build -p orix-cli
./target/debug/orix --help
```

### Using Cargo install

```bash
cargo install --path crates/cli
```

### Download prebuilt binaries

Download the compressed package for your platform from [GitHub Releases](https://github.com/baicie/orix/releases) and extract it.

## Quick Start

```bash
# Install dependencies
orix install

# Verify install from lockfile (for CI)
orix install --frozen-lockfile

# Use local cache only
orix install --offline

# Specify registry
orix install --registry https://registry.npmmirror.com
```

## Project Structure

```
crates/
├── cli          # CLI entry point
├── core         # Installation pipeline orchestration
├── config       # .npmrc configuration loading
├── domain       # Shared domain types
├── manifest     # package.json parsing
├── resolver     # Dependency graph construction + semver resolution
├── registry     # npm registry API client
├── fetcher      # tarball download and extraction
├── store        # Content-addressable global cache
├── lockfile     # orix-lock.yaml management
├── linker       # node_modules/.orix structure generation
├── workspace    # Workspace discovery and circular dependency detection
├── utils        # Shared utility functions
└── macros       # Procedural macros (reserved)
```

## Installation Pipeline

```
orix install
  → Config.resolve()    Load .npmrc / env / CLI args
  → Manifest.read()     Parse package.json
  → Workspace.discover()  Find pnpm-workspace.yaml
  → Lockfile.read()     Load existing lockfile
  → Resolver.resolve()  Build dependency graph (with workspace protocol)
  → Registry.fetch_packument()  Fetch packument
  → semver match + platform filter
  → Fetcher.fetch_all()  Concurrent tarball download
  → Store.import_package()  CAS deduplication + hard link
  → Lockfile.update()   Write orix-lock.yaml
  → Linker.link_graph() Generate .orix structure
```

## Development

```bash
# Format + linter + tests (full check)
cargo xtask check

# Build
cargo build --workspace

# Test
cargo test --workspace

# Documentation
cargo doc --no-deps
```

Optional tools:

```bash
cargo install cargo-deny cargo-machete cargo-llvm-cov
cargo machete
cargo deny check
```

## Status

orix MVP covers the following capabilities:

| Capability | Status |
|---|---|
| package.json parsing | ✅ |
| lockfile generation and frozen-lockfile verification | ✅ |
| npm registry fetching | ✅ |
| tarball download, integrity check, extraction | ✅ |
| CAS global cache | ✅ |
| node_modules/.orix structure generation | ✅ |
| Root and child dependency linking | ✅ |
| Workspace minimal support | ✅ |

MVP does not yet cover: full peerDependencies algorithm, full-mode hoist, publish/patch/catalogs, complex lifecycle scripts sandbox.

## Architecture

See the design documents under [docs/.project/design/](docs/.project/design/).

## License

MIT
