---
name: setup-rust
description: Configure project-specific Rust workspace context before using Rust skills. Use when first setting up Rust agent context for this repository, refreshing Rust workspace metadata, or preparing other Rust skills to work with the local crate graph and commands.
allowed-tools:
  - Read
  - Glob
  - Grep
  - Bash
effort: low
tags: [rust, workspace, setup, cargo, xtask]
---

# Rust Workspace Setup

Project-level configuration that the other Rust skills consume.

## What This Does

This skill configures the agent for Rust-first development in this workspace. Run once per project before using any other Rust skill.

## Configuration Steps

### 1. Detect Toolchain

Run `rustc --version` and `cargo --version` and note:

- MSRV (Minimum Supported Rust Version): `1.80`
- Latest installed stable version
- Whether `rustup` is available for toolchain switching

### 2. Detect xtask Commands

Read `xtask/src/main.rs` and build a command index:

| Command | Equivalent To |
| --- | --- |
| `cargo xtask check` | `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets --all-features -- -D warnings` + `cargo test --workspace --all-features` + `cargo doc --workspace --all-features --no-deps` |
| `cargo xtask fmt` | `cargo fmt --all` |
| `cargo xtask lint` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `cargo xtask test` | `cargo test --workspace --all-features` |
| `cargo xtask doc` | `cargo doc --workspace --all-features --no-deps` |
| `cargo xtask security` | `cargo-deny check` + `cargo-audit audit` + `cargo-machete --with-metadata`, skipping missing optional tools |

Prefer `cargo xtask check` over individual commands unless doing targeted work.

### 3. Identify Workspace Members

Parse root `Cargo.toml` `[workspace.members]` and confirm crate types:

```toml
# Binary crates
crates/cli

# Library crates
crates/core
crates/config
crates/utils
crates/macros
crates/domain
crates/manifest
crates/resolver
crates/registry
crates/fetcher
crates/store
crates/lockfile
crates/linker
crates/workspace

# Automation and examples
xtask
examples
```

### 4. Map Dependencies

Read each crate's `Cargo.toml` to understand the dependency graph. The intended hierarchy is:

```text
cli -> core, config
core -> utils, domain, manifest, resolver, registry, fetcher, store, lockfile, linker, workspace
manifest -> domain
resolver -> domain, registry
fetcher -> registry
store -> domain
lockfile -> domain
linker -> store, domain
workspace -> manifest
config -> none
utils -> none
macros -> none
domain -> none
```

Keep this graph acyclic. Do not introduce cross-layer dependencies without updating the design docs first.

### 5. Identify Config Files

| File | Purpose |
| --- | --- |
| `rustfmt.toml` | Code formatting rules |
| `clippy.toml` | Clippy lint configuration |
| `deny.toml` | cargo-deny dependency audit config |
| `cliff.toml` | git-cliff changelog generation |
| `.cargo/config.toml` | Cargo aliases and build configuration |
| `Makefile` | Project command wrapper; `make check` delegates to `cargo xtask check` |

### 6. Read Project Design Docs

Before implementation, read the relevant document under `docs/.project/design/`:

| Area | Design Doc |
| --- | --- |
| Overall architecture | `docs/.project/design/index.md` |
| CLI and config | `docs/.project/design/cli-config.md` |
| Install orchestration | `docs/.project/design/core.md` |
| Manifest parsing | `docs/.project/design/index.md` and crate docs |
| Resolver | `docs/.project/design/resolver.md` |
| Registry and fetcher | `docs/.project/design/fetcher.md` |
| Store | `docs/.project/design/store.md` |
| Linker | `docs/.project/design/linker.md` |
| Lockfile | `docs/.project/design/lockfile.md` |
| Workspace | `docs/.project/design/workspace.md` |

### 7. Security and Release Workflows

Note the CI workflow path: `.github/workflows/ci.yml`. Tests run on Ubuntu, Windows, and macOS. Security scans run weekly.

## Notes

- This workspace is `orix`, a Rust package manager with pnpm-compatible installation structure.
- MSRV is `1.80`; do not introduce dependencies requiring a higher version without warning.
- All third-party dependency versions live in root `Cargo.toml` `[workspace.dependencies]`.
- Individual crates should use `.workspace = true` for shared dependencies instead of hardcoding versions.
- Library crates should prefer structured errors with `thiserror`; application boundaries can use `anyhow`.
- Full validation is `make check` or `cargo xtask check`.
