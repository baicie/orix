# orix examples

This directory contains small JavaScript projects that exercise different parts of orix.

Run an example from the repository root:

```bash
cargo run -p orix-cli -- --dir examples/basic-install install
```

Or from inside an example:

```bash
cd examples/basic-install
cargo run -p orix-cli -- install
```

## Examples

| Example | Focus |
| --- | --- |
| `basic-install` | Regular production dependencies and CommonJS entry point. |
| `dev-dependencies` | `devDependencies`, ESM, and script metadata parsing. |
| `optional-dependencies` | Optional dependency handling, including platform-specific packages. |
| `bin-package` | `bin` entries and executable package metadata. |
| `workspace-monorepo` | `pnpm-workspace.yaml` discovery and `workspace:*` local packages. |

These projects are intentionally tiny so they can be used as manual fixtures while developing resolver, fetcher, lockfile, linker, and workspace behavior.
