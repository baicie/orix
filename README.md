# Rust 工作区模板

适用于多 crate CLI/库项目的生产级 Rust 工作区模板。

## 功能特性

- 多 crate Cargo 工作区
- CLI + core + config + utils + 可选过程宏 crate
- `rustfmt`、`clippy`、测试、文档
- `xtask` 开发命令
- Linux、macOS、Windows 上的 GitHub Actions CI
- `cargo-deny`、`cargo-audit`、`cargo-machete` 安全检查
- `cargo llvm-cov` 覆盖率
- 多平台二进制文件的 GitHub Release
- 手动 crates.io 发布流程
- Dependabot、issue 模板、PR 模板
- VSCode 推荐设置

## 目录结构

```txt
crates/
  cli/       # 二进制 crate
  core/      # 核心业务逻辑
  config/    # 配置加载
  utils/     # 共享工具
  macros/    # 可选过程宏 crate
xtask/       # 仓库自动化命令
```

## 快速开始

```bash
cargo build --workspace
cargo test --workspace
cargo run -p your-cli -- hello Zeus
```

## 开发命令

```bash
cargo xtask check
cargo xtask fmt
cargo xtask lint
cargo xtask test
cargo xtask doc
cargo xtask security
```

安装可选工具：

```bash
cargo install cargo-deny cargo-audit cargo-machete cargo-llvm-cov git-cliff
```

## 重命名检查清单

替换以下占位符：

- `your`
- `your-cli`
- `your-core`
- `your-config`
- `your-utils`
- `your-macros`
- `your-org/your-repo`
- 根 `Cargo.toml` 中的作者元数据

## 发布

创建并推送 tag：

```bash
git tag v0.1.0
git push origin v0.1.0
```

Release 工作流会构建多平台二进制文件并创建 GitHub Release。

## 发布到 crates.io

在 GitHub 仓库 secrets 中设置 `CARGO_REGISTRY_TOKEN`，然后手动运行 `Publish crates` 工作流。

默认模式为 dry-run。准备就绪后关闭 `dry_run`。
