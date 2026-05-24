# Contributing

Thank you for your interest in contributing to orix!

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/your-username/orix.git`
3. Add the upstream remote: `git remote add upstream https://github.com/orix/orix.git`
4. Create a feature branch: `git checkout -b feat/your-feature`

## Development Setup

### Requirements

- Rust stable (MSRV: 1.80)
- `rustfmt`
- `clippy`

### Recommended Tools

```bash
cargo install cargo-deny cargo-audit cargo-machete cargo-llvm-cov git-cliff
```

### Verify Setup

```bash
cargo xtask check
```

## Project Structure

orix is a Rust workspace with the following crates:

| Crate | Purpose |
|---|---|
| `cli` | Command-line entry point, argument parsing |
| `core` | Install pipeline orchestration |
| `config` | `.npmrc` / environment variable configuration |
| `domain` | Shared types: `PackageId`, `Version`, `DependencyGraph` |
| `manifest` | `package.json` parsing and validation |
| `resolver` | Semver resolution, dependency graph building |
| `registry` | npm registry API client |
| `fetcher` | Tarball download, integrity verification, extraction |
| `store` | Content-addressable global package cache |
| `lockfile` | `orix-lock.yaml` read/write and diff |
| `linker` | `node_modules/.pnpm` structure generation |
| `workspace` | Workspace discovery, `pnpm-workspace.yaml` parsing |
| `utils` | Shared utility functions |
| `macros` | Procedural macros (reserved) |

Dependency direction (no cycles):

```
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

## Code Standards

Before submitting a PR, ensure:

- [ ] `cargo xtask check` passes
- [ ] `cargo fmt` has been run
- [ ] No new clippy warnings introduced
- [ ] Tests pass: `cargo test --workspace`
- [ ] Docs build: `cargo doc --workspace --no-deps`

### API Design Conventions

- Fallible APIs return `Result<T>`.
- Library crates use `thiserror` for structured errors.
- Application boundaries (`cli`, `core`) use `anyhow`.
- Library code avoids `panic!` unless an explicit invariant is violated.
- Public APIs stay minimal; internal implementation prefers `pub(crate)` or private.
- All third-party dependency versions live only in `Cargo.toml`'s `[workspace.dependencies]`.
- Each crate's `Cargo.toml` uses `.workspace = true` instead of hardcoding versions.

## Testing

- Unit tests live in `#[cfg(test)] mod tests` within the corresponding source file.
- Integration tests live in `tests/integration.rs` at the crate root or workspace root.
- Fixing a bug should include a regression test.
- For mock HTTP servers in tests, ensure the server thread signals readiness before sending requests.

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(core): add new configuration option
fix(cli): handle missing config file gracefully
docs: update contributing guide
refactor(utils): simplify string helper functions
chore: update dependencies
```

## Pull Request Process

1. Update documentation if needed
2. Add tests for new functionality
3. Ensure CI passes on all platforms
4. Request review from maintainers
5. Address feedback promptly

## Branch Protection

- `main` is a protected branch
- Direct pushes are forbidden
- All PRs require review and passing CI

## Questions?

Open an issue on GitHub for bugs, feature requests, or questions.
