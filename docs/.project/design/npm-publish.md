最推荐方案就是这一个：

> **Rust 做完整 CLI 可执行文件；npm 只做分发。**
> **不用 NAPI，不自己搓 binding，不用 postinstall 下载/编译。**

这个方案最适合“pnpm Rust 化”这种项目，因为包管理器本质是一个完整命令行程序，需要控制 `stdout/stderr/exit code/signal/env/cwd/子进程`，不是 JS 高频调用 Rust 函数。

---

# 1. 总体架构

```text
用户执行 pnpm-rs install
        ↓
npm 创建的 bin shim
        ↓
npm/main/bin/pnpm-rs.js
        ↓
选择当前平台的 native package
        ↓
spawn Rust binary
        ↓
Rust CLI 执行完整逻辑
```

npm 主包通过 `optionalDependencies` 依赖多个平台包；平台包用 `os / cpu / libc` 限制安装目标平台，npm 的安装配置也支持这些平台相关字段。([npm 文档][1])

---

# 2. 包设计

最终发布这些 npm 包：

```text
@baicie/pnpm-rs
@baicie/pnpm-rs-darwin-arm64
@baicie/pnpm-rs-darwin-x64
@baicie/pnpm-rs-linux-x64-gnu
@baicie/pnpm-rs-linux-arm64-gnu
@baicie/pnpm-rs-linux-x64-musl
@baicie/pnpm-rs-win32-x64-msvc
```

主包只放：

```text
JS wrapper
README
LICENSE
package.json
```

平台包只放：

```text
Rust 编译出来的 binary
package.json
README
LICENSE
```

不要在安装阶段做：

```text
postinstall
cargo build
curl 下载 binary
node-gyp rebuild
```

这样可以避免你之前遇到的 `pnpm approve-builds`、CI 忽略 build scripts、安全策略等问题。

---

# 3. 仓库目录结构

```text
pnpm-rs/
├─ Cargo.toml
├─ Cargo.lock
├─ crates/
│  ├─ pnpm-rs-cli/
│  │  ├─ Cargo.toml
│  │  └─ src/
│  │     └─ main.rs
│  ├─ pnpm-rs-core/
│  │  ├─ Cargo.toml
│  │  └─ src/
│  │     ├─ lib.rs
│  │     ├─ command.rs
│  │     ├─ error.rs
│  │     └─ context.rs
│  ├─ pnpm-rs-lockfile/
│  ├─ pnpm-rs-resolver/
│  ├─ pnpm-rs-fetcher/
│  ├─ pnpm-rs-store/
│  ├─ pnpm-rs-lifecycle/
│  └─ pnpm-rs-workspace/
├─ npm/
│  ├─ main/
│  │  ├─ package.json
│  │  ├─ README.md
│  │  ├─ LICENSE
│  │  └─ bin/
│  │     └─ pnpm-rs.js
│  ├─ darwin-arm64/
│  │  ├─ package.json
│  │  └─ bin/
│  ├─ darwin-x64/
│  ├─ linux-x64-gnu/
│  ├─ linux-arm64-gnu/
│  ├─ linux-x64-musl/
│  └─ win32-x64-msvc/
├─ scripts/
│  ├─ sync-version.mjs
│  ├─ prepare-npm-package.mjs
│  ├─ check-package.mjs
│  └─ local-pack.mjs
├─ .github/
│  └─ workflows/
│     ├─ ci.yml
│     └─ release.yml
├─ package.json
├─ pnpm-workspace.yaml
├─ rust-toolchain.toml
└─ README.md
```

---

# 4. Rust workspace 设计

根目录 `Cargo.toml`：

```toml
[workspace]
resolver = "2"
members = [
  "crates/pnpm-rs-cli",
  "crates/pnpm-rs-core",
  "crates/pnpm-rs-lockfile",
  "crates/pnpm-rs-resolver",
  "crates/pnpm-rs-fetcher",
  "crates/pnpm-rs-store",
  "crates/pnpm-rs-lifecycle",
  "crates/pnpm-rs-workspace"
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/baicie/pnpm-rs"

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

CLI crate：

```toml
[package]
name = "pnpm-rs-cli"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "pnpm-rs"
path = "src/main.rs"

[dependencies]
anyhow.workspace = true
clap.workspace = true
tokio.workspace = true
pnpm-rs-core = { path = "../pnpm-rs-core" }
```

---

# 5. Rust 代码边界

CLI 层只负责：

```text
解析参数
初始化日志
设置 cwd/env
调用 core
映射退出码
```

核心逻辑不要写在 `main.rs`。

`crates/pnpm-rs-cli/src/main.rs`：

```rust
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pnpm-rs")]
#[command(version)]
#[command(about = "A Rust-powered pnpm-like package manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install {
        #[arg(long)]
        frozen_lockfile: bool,
    },
    Add {
        packages: Vec<String>,
    },
    Remove {
        packages: Vec<String>,
    },
    Run {
        script: String,
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = run(cli).await;

    if let Err(err) = result {
        eprintln!("pnpm-rs error: {err:?}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Install { frozen_lockfile } => {
            pnpm_rs_core::install::install(frozen_lockfile).await?;
        }
        Commands::Add { packages } => {
            pnpm_rs_core::add::add(packages).await?;
        }
        Commands::Remove { packages } => {
            pnpm_rs_core::remove::remove(packages).await?;
        }
        Commands::Run { script, args } => {
            pnpm_rs_core::run_script::run_script(script, args).await?;
        }
    }

    Ok(())
}
```

`core` 暴露稳定能力：

```rust
pub mod add;
pub mod install;
pub mod remove;
pub mod run_script;
pub mod error;
pub mod context;
```

这样后续如果你真想做 Node API，只需要新增：

```text
crates/pnpm-rs-napi
```

让它依赖 `pnpm-rs-core`，而不是把 NAPI 混进主链路。

---

# 6. npm 主包设计

`npm/main/package.json`：

```json
{
  "name": "@baicie/pnpm-rs",
  "version": "0.1.0",
  "description": "A Rust-powered pnpm-like package manager",
  "license": "MIT",
  "type": "commonjs",
  "bin": {
    "pnpm-rs": "./bin/pnpm-rs.js"
  },
  "files": ["bin", "README.md", "LICENSE"],
  "engines": {
    "node": ">=18"
  },
  "optionalDependencies": {
    "@baicie/pnpm-rs-darwin-arm64": "0.1.0",
    "@baicie/pnpm-rs-darwin-x64": "0.1.0",
    "@baicie/pnpm-rs-linux-x64-gnu": "0.1.0",
    "@baicie/pnpm-rs-linux-arm64-gnu": "0.1.0",
    "@baicie/pnpm-rs-linux-x64-musl": "0.1.0",
    "@baicie/pnpm-rs-win32-x64-msvc": "0.1.0"
  },
  "publishConfig": {
    "access": "public"
  }
}
```

注意几点：

```text
optionalDependencies 版本必须精确锁定
主包最后发布
平台包先发布
不要写 ^0.1.0
不要写 latest
```

因为主包和平台包必须强绑定，否则容易出现：

```text
JS wrapper 是 0.2.0
native binary 还是 0.1.0
```

---

# 7. 平台包设计

`npm/darwin-arm64/package.json`：

```json
{
  "name": "@baicie/pnpm-rs-darwin-arm64",
  "version": "0.1.0",
  "description": "Native binary for @baicie/pnpm-rs on macOS arm64",
  "license": "MIT",
  "os": ["darwin"],
  "cpu": ["arm64"],
  "files": ["bin", "README.md", "LICENSE"],
  "publishConfig": {
    "access": "public"
  }
}
```

`npm/linux-x64-gnu/package.json`：

```json
{
  "name": "@baicie/pnpm-rs-linux-x64-gnu",
  "version": "0.1.0",
  "description": "Native binary for @baicie/pnpm-rs on Linux x64 glibc",
  "license": "MIT",
  "os": ["linux"],
  "cpu": ["x64"],
  "libc": ["glibc"],
  "files": ["bin", "README.md", "LICENSE"],
  "publishConfig": {
    "access": "public"
  }
}
```

`npm/linux-x64-musl/package.json`：

```json
{
  "name": "@baicie/pnpm-rs-linux-x64-musl",
  "version": "0.1.0",
  "description": "Native binary for @baicie/pnpm-rs on Linux x64 musl",
  "license": "MIT",
  "os": ["linux"],
  "cpu": ["x64"],
  "libc": ["musl"],
  "files": ["bin", "README.md", "LICENSE"],
  "publishConfig": {
    "access": "public"
  }
}
```

`npm/win32-x64-msvc/package.json`：

```json
{
  "name": "@baicie/pnpm-rs-win32-x64-msvc",
  "version": "0.1.0",
  "description": "Native binary for @baicie/pnpm-rs on Windows x64 MSVC",
  "license": "MIT",
  "os": ["win32"],
  "cpu": ["x64"],
  "files": ["bin", "README.md", "LICENSE"],
  "publishConfig": {
    "access": "public"
  }
}
```

scoped public package 首次发布时需要 `npm publish --access public`，npm 官方文档也是这样要求的。([npm 文档][2])

---

# 8. JS bin wrapper

`npm/main/bin/pnpm-rs.js`：

```js
#!/usr/bin/env node

"use strict";

const { spawnSync } = require("node:child_process");

function isMusl() {
  if (process.platform !== "linux") return false;

  try {
    const report =
      process.report && process.report.getReport
        ? process.report.getReport()
        : null;

    const glibcVersionRuntime =
      report && report.header && report.header.glibcVersionRuntime;

    return !glibcVersionRuntime;
  } catch {
    return false;
  }
}

function getNativePackageName() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "darwin" && arch === "arm64") {
    return "@baicie/pnpm-rs-darwin-arm64";
  }

  if (platform === "darwin" && arch === "x64") {
    return "@baicie/pnpm-rs-darwin-x64";
  }

  if (platform === "win32" && arch === "x64") {
    return "@baicie/pnpm-rs-win32-x64-msvc";
  }

  if (platform === "linux" && arch === "x64") {
    return isMusl()
      ? "@baicie/pnpm-rs-linux-x64-musl"
      : "@baicie/pnpm-rs-linux-x64-gnu";
  }

  if (platform === "linux" && arch === "arm64") {
    return "@baicie/pnpm-rs-linux-arm64-gnu";
  }

  throw new Error(
    `Unsupported platform: ${platform} ${arch}. ` +
      `Please open an issue with your OS and CPU info.`,
  );
}

function getBinaryPath() {
  const pkg = getNativePackageName();
  const binName = process.platform === "win32" ? "pnpm-rs.exe" : "pnpm-rs";

  try {
    return require.resolve(`${pkg}/bin/${binName}`);
  } catch (error) {
    const message = [
      `Failed to resolve native binary package: ${pkg}`,
      "",
      "Possible reasons:",
      "1. optionalDependencies were disabled during install.",
      "2. The package manager skipped platform-specific packages.",
      "3. The installation cache is corrupted.",
      "",
      "Try reinstalling:",
      "  npm i -g @baicie/pnpm-rs",
      "  pnpm add -g @baicie/pnpm-rs",
      "",
      `Original error: ${error && error.message ? error.message : String(error)}`,
    ].join("\n");

    throw new Error(message, { cause: error });
  }
}

const binPath = getBinaryPath();

const result = spawnSync(binPath, process.argv.slice(2), {
  stdio: "inherit",
  cwd: process.cwd(),
  env: process.env,
  windowsHide: false,
});

if (result.error) {
  throw result.error;
}

if (typeof result.status === "number") {
  process.exit(result.status);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
}

process.exit(1);
```

这个 wrapper 只干一件事：

```text
选 binary，然后转发参数。
```

不要在这里做业务逻辑。

---

# 9. 根 package.json

根目录 `package.json` 只做开发脚本，不发布：

```json
{
  "name": "pnpm-rs-repo",
  "private": true,
  "type": "module",
  "packageManager": "pnpm@10.0.0",
  "scripts": {
    "check": "cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace",
    "build": "cargo build --release --bin pnpm-rs",
    "sync-version": "node scripts/sync-version.mjs",
    "pack:local": "node scripts/local-pack.mjs"
  },
  "devDependencies": {}
}
```

`pnpm-workspace.yaml`：

```yaml
packages:
  - "npm/*"
```

这里虽然用了 pnpm workspace，但它只是管理 npm 包目录，不影响 Rust workspace。

---

# 10. 版本同步脚本

`scripts/sync-version.mjs`：

```js
import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const version = process.argv[2];

if (!version) {
  console.error("Usage: node scripts/sync-version.mjs <version>");
  process.exit(1);
}

const packages = [
  "main",
  "darwin-arm64",
  "darwin-x64",
  "linux-x64-gnu",
  "linux-arm64-gnu",
  "linux-x64-musl",
  "win32-x64-msvc",
];

for (const dir of packages) {
  const file = path.join(root, "npm", dir, "package.json");
  const json = JSON.parse(fs.readFileSync(file, "utf8"));

  json.version = version;

  if (dir === "main") {
    for (const name of Object.keys(json.optionalDependencies)) {
      json.optionalDependencies[name] = version;
    }
  }

  fs.writeFileSync(file, `${JSON.stringify(json, null, 2)}\n`);
}

console.log(`Synced npm package versions to ${version}`);
```

---

# 11. CI 设计

`.github/workflows/ci.yml`：

```yaml
name: CI

on:
  pull_request:
  push:
    branches:
      - main

jobs:
  rust:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v6

      - uses: dtolnay/rust-toolchain@stable

      - name: Check formatting
        run: cargo fmt --all --check

      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings

      - name: Test
        run: cargo test --workspace

      - name: Build
        run: cargo build --release --bin pnpm-rs
```

---

# 12. Release 设计

GitHub Actions 可以用 artifact 在不同 job 之间传递构建产物，官方文档说明了 `upload-artifact` 和 `download-artifact` 的用法。([GitHub Docs][3])

`.github/workflows/release.yml`：

```yaml
name: Release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: read
  id-token: write

jobs:
  build:
    name: Build ${{ matrix.package }}
    runs-on: ${{ matrix.os }}

    strategy:
      fail-fast: false
      matrix:
        include:
          - package: darwin-arm64
            os: macos-latest
            target: aarch64-apple-darwin
            bin: pnpm-rs

          - package: darwin-x64
            os: macos-latest
            target: x86_64-apple-darwin
            bin: pnpm-rs

          - package: linux-x64-gnu
            os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            bin: pnpm-rs

          - package: linux-x64-musl
            os: ubuntu-latest
            target: x86_64-unknown-linux-musl
            bin: pnpm-rs

          - package: linux-arm64-gnu
            os: ubuntu-latest
            target: aarch64-unknown-linux-gnu
            bin: pnpm-rs

          - package: win32-x64-msvc
            os: windows-latest
            target: x86_64-pc-windows-msvc
            bin: pnpm-rs.exe

    steps:
      - uses: actions/checkout@v6

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install musl tools
        if: matrix.target == 'x86_64-unknown-linux-musl'
        run: sudo apt-get update && sudo apt-get install -y musl-tools

      - name: Install Linux arm64 linker
        if: matrix.target == 'aarch64-unknown-linux-gnu'
        run: sudo apt-get update && sudo apt-get install -y gcc-aarch64-linux-gnu

      - name: Configure Linux arm64 linker
        if: matrix.target == 'aarch64-unknown-linux-gnu'
        run: |
          mkdir -p .cargo
          cat > .cargo/config.toml <<'EOF'
          [target.aarch64-unknown-linux-gnu]
          linker = "aarch64-linux-gnu-gcc"
          EOF

      - name: Build Rust binary
        run: cargo build --release --locked --bin pnpm-rs --target ${{ matrix.target }}

      - name: Prepare package directory
        shell: bash
        run: |
          mkdir -p dist/${{ matrix.package }}/bin
          cp npm/${{ matrix.package }}/package.json dist/${{ matrix.package }}/package.json
          cp README.md dist/${{ matrix.package }}/README.md
          cp LICENSE dist/${{ matrix.package }}/LICENSE
          cp target/${{ matrix.target }}/release/${{ matrix.bin }} dist/${{ matrix.package }}/bin/${{ matrix.bin }}

      - name: Upload package
        uses: actions/upload-artifact@v4
        with:
          name: npm-${{ matrix.package }}
          path: dist/${{ matrix.package }}

  publish:
    name: Publish npm packages
    needs: build
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v6

      - uses: actions/setup-node@v6
        with:
          node-version: 24
          registry-url: https://registry.npmjs.org

      - name: Install latest npm
        run: npm install -g npm@latest

      - name: Extract version
        id: version
        shell: bash
        run: |
          VERSION="${GITHUB_REF_NAME#v}"
          echo "version=$VERSION" >> "$GITHUB_OUTPUT"

      - name: Sync versions
        run: node scripts/sync-version.mjs ${{ steps.version.outputs.version }}

      - name: Download artifacts
        uses: actions/download-artifact@v5
        with:
          path: artifacts

      - name: Restore platform packages
        shell: bash
        run: |
          rm -rf dist
          mkdir -p dist

          for dir in artifacts/npm-*; do
            name="$(basename "$dir" | sed 's/^npm-//')"
            mkdir -p "dist/$name"
            cp -R "$dir"/. "dist/$name/"
          done

      - name: Prepare main package
        shell: bash
        run: |
          mkdir -p dist/main
          cp -R npm/main/. dist/main/
          cp README.md dist/main/README.md
          cp LICENSE dist/main/LICENSE

      - name: Publish platform packages
        shell: bash
        run: |
          npm publish ./dist/darwin-arm64 --access public
          npm publish ./dist/darwin-x64 --access public
          npm publish ./dist/linux-x64-gnu --access public
          npm publish ./dist/linux-arm64-gnu --access public
          npm publish ./dist/linux-x64-musl --access public
          npm publish ./dist/win32-x64-msvc --access public

      - name: Publish main package
        run: npm publish ./dist/main --access public
```

npm Trusted Publishing 允许从 CI/CD 通过 OIDC 发布，避免长期 npm token；GitHub Actions 场景需要 `id-token: write`，并且 npm 文档要求 npm CLI 11.5.1+、Node 22.14.0+。([npm 文档][4])

---

# 13. npm Trusted Publishing 配置

首次发包后，在 npmjs 后台给每个包配置 Trusted Publisher：

```text
Package Settings
  → Trusted Publisher
  → GitHub Actions
```

填写：

```text
Organization/User: baicie
Repository: pnpm-rs
Workflow filename: release.yml
Environment: 可不填
```

注意：

```text
每个 npm 包都要配置一次
主包和所有平台包都要配置
```

配置好以后，CI 发布不需要 `NPM_TOKEN`。

---

# 14. 发布顺序

必须是：

```text
1. 构建所有平台 binary
2. 打包所有平台 npm package
3. 发布所有平台包
4. 发布主包 @baicie/pnpm-rs
```

不要反过来。

否则主包发布后，用户安装时会发现：

```text
optionalDependencies 里引用的平台包还不存在
```

---

# 15. 本地调试方案

先本地编译：

```bash
cargo build --release --bin pnpm-rs
```

复制当前平台 binary：

```bash
mkdir -p npm/darwin-arm64/bin
cp target/release/pnpm-rs npm/darwin-arm64/bin/pnpm-rs
```

本地 pack：

```bash
cd npm/darwin-arm64
npm pack

cd ../main
npm pack
```

本地全局安装主包测试：

```bash
npm i -g ./baicie-pnpm-rs-0.1.0.tgz
pnpm-rs --version
```

如果你想测试平台包联动，可以先在临时目录里：

```bash
npm i ../npm/darwin-arm64/baicie-pnpm-rs-darwin-arm64-0.1.0.tgz
npm i ../npm/main/baicie-pnpm-rs-0.1.0.tgz
```

---

# 16. 校验脚本

`scripts/check-package.mjs`：

```js
import fs from "node:fs";
import path from "node:path";

const root = process.cwd();

const packages = [
  "main",
  "darwin-arm64",
  "darwin-x64",
  "linux-x64-gnu",
  "linux-arm64-gnu",
  "linux-x64-musl",
  "win32-x64-msvc",
];

const versions = new Set();

for (const dir of packages) {
  const file = path.join(root, "npm", dir, "package.json");
  const json = JSON.parse(fs.readFileSync(file, "utf8"));

  versions.add(json.version);

  if (json.private) {
    throw new Error(`${dir} must not be private`);
  }

  if (!json.publishConfig || json.publishConfig.access !== "public") {
    throw new Error(`${dir} missing publishConfig.access=public`);
  }

  if (dir !== "main") {
    if (!json.files || !json.files.includes("bin")) {
      throw new Error(`${dir} must include bin in files`);
    }
  }
}

if (versions.size !== 1) {
  throw new Error(
    `Package versions are inconsistent: ${[...versions].join(", ")}`,
  );
}

console.log("Package metadata check passed");
```

CI 发布前跑：

```bash
node scripts/check-package.mjs
```

---

# 17. 是否需要 NAPI？

当前不需要。

推荐阶段设计：

```text
阶段 1：
Rust binary + npm 分发

阶段 2：
如果需要 JS API，再新增 @baicie/pnpm-rs-api

阶段 3：
@baicie/pnpm-rs-api 使用 napi-rs，复用 pnpm-rs-core
```

未来如果你要提供：

```ts
import { parseLockfile, resolveWorkspace } from "@baicie/pnpm-rs-api";
```

再做 NAPI。

但主 CLI 仍然保持：

```text
JS wrapper → spawn Rust binary
```

不要把 CLI 强行塞进 NAPI。

---

# 18. 为什么不用 postinstall

不要这样：

```json
{
  "scripts": {
    "postinstall": "node install.js"
  }
}
```

原因：

```text
1. pnpm 默认可能拦 build scripts
2. CI 经常设置 --ignore-scripts
3. 用户安全策略可能禁用 install scripts
4. 国内网络下载 binary 容易失败
5. 失败后用户体验很差
6. 安装阶段执行脚本容易被安全工具标红
```

你之前吐槽 `approve-builds` 难用，这个方案正好绕开它。

---

# 19. 最小 MVP 发布范围

第一版不要一口气支持所有平台。

我建议 MVP 先发：

```text
@baicie/pnpm-rs
@baicie/pnpm-rs-darwin-arm64
@baicie/pnpm-rs-linux-x64-gnu
@baicie/pnpm-rs-win32-x64-msvc
```

也就是：

```text
你的 Mac M1/M2/M3
大部分 Linux CI
Windows 用户
```

稳定后再补：

```text
darwin-x64
linux-x64-musl
linux-arm64-gnu
```

---

# 20. 最终方案总结

你这个项目最优设计就是：

```text
Rust workspace
  ├─ core：核心包管理逻辑
  ├─ cli：命令行入口
  ├─ resolver：依赖解析
  ├─ fetcher：包下载
  ├─ store：虚拟 store / hardlink / symlink
  ├─ lifecycle：scripts 执行
  └─ workspace：monorepo workspace 处理

npm packages
  ├─ @baicie/pnpm-rs：主包，只放 JS wrapper
  ├─ @baicie/pnpm-rs-darwin-arm64：平台 binary
  ├─ @baicie/pnpm-rs-linux-x64-gnu：平台 binary
  └─ @baicie/pnpm-rs-win32-x64-msvc：平台 binary

发布方式
  ├─ tag v0.1.0 触发
  ├─ GitHub Actions matrix 构建
  ├─ artifacts 汇总
  ├─ 先发布平台包
  ├─ 最后发布主包
  └─ npm Trusted Publishing，不用 NPM_TOKEN
```

一句话：

> **CLI 主链路直接 Rust binary；npm 只是安装入口；NAPI 留给未来 JS API；绝不自己搓 binding。**

[1]: https://docs.npmjs.com/cli/v11/commands/npm-install?utm_source=chatgpt.com "npm-install"
[2]: https://docs.npmjs.com/creating-and-publishing-an-organization-scoped-package/?utm_source=chatgpt.com "Creating and publishing an organization scoped package"
[3]: https://docs.github.com/en/actions/tutorials/store-and-share-data "Store and share data with workflow artifacts - GitHub Docs"
[4]: https://docs.npmjs.com/trusted-publishers/ "Trusted publishing for npm packages | npm Docs"
