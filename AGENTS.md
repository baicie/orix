# Agent Guide - orix

## 沟通约定

- 所有回复使用中文。
- 开始实现前，先阅读相关设计文档，优先参考 `docs/.project/design/`。
- 默认保持改动小而聚焦，遵循现有 crate 边界和项目风格。
- 不要提交或交付未通过必要检查的代码；完整检查以 `make check` 为准。

## 项目定位

orix 是一个用 Rust 实现的高性能包管理器，采用 pnpm 兼容思路，MVP 聚焦：

- package.json 解析
- lockfile 生成与 frozen-lockfile 校验
- npm registry 拉包
- tarball 下载、完整性校验、解压
- CAS 全局缓存
- node_modules/.pnpm 结构生成
- 根依赖和子依赖链接
- workspace 最小支持

MVP 暂不覆盖：

- peerDependencies 完整算法
- 全模式 hoist
- publish / patch / catalogs / deploy
- 复杂 lifecycle scripts 沙箱

## 代码结构

```txt
crates/
├── cli              # 命令行入口，负责参数解析和调用 core
├── config           # .npmrc / registry / proxy / store 配置
├── manifest         # package.json 解析和验证
├── resolver         # semver 解析、依赖图构建
├── registry         # npm registry API，packument 和 tarball 元数据
├── fetcher          # tarball 下载、完整性验证、解压
├── store            # 内容可寻址包缓存
├── lockfile         # orix-lock.yaml 读写和 diff
├── linker           # node_modules/.pnpm 结构、硬链接和符号链接
├── workspace        # workspace 发现、pnpm-workspace.yaml 解析
├── domain           # 共享领域类型
├── utils            # 共享工具函数
├── macros           # 过程宏预留
└── core             # 安装管道编排

xtask/               # 开发自动化
docs/.project/       # 架构设计文档
```

依赖方向必须保持无环：

```txt
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

## 安装流程

```txt
orix install
  -> Config.resolve()        加载 .npmrc、env、CLI 参数
  -> Manifest.read()         解析 package.json
  -> Workspace.discover()    查找 pnpm-workspace.yaml
  -> Lockfile.read()         加载现有 lockfile
  -> Resolver.resolve()      构建依赖图
     -> Registry.fetch_packument()
     -> semver 匹配
  -> Fetcher.fetch_all()     下载并解压 tarball
     -> TarballCache.get_or_fetch()
     -> 完整性校验
     -> 解压到临时目录
  -> Store.import_package()  文件级去重并导入 CAS store
  -> Lockfile.update()       写入 orix-lock.yaml
  -> Linker.link_graph()     构建 node_modules
     -> 创建 .pnpm 树
     -> 从 store 硬链接
     -> 创建虚拟依赖链接
  -> done
```

## 设计文档

| 文档 | Crate | 用途 |
| --- | --- | --- |
| [设计总览](docs/.project/design/index.md) | - | 整体架构、数据流、设计原则 |
| [CAS Store](docs/.project/design/store.md) | `crates/store` | 内容可寻址全局包缓存 |
| [Linker](docs/.project/design/linker.md) | `crates/linker` | node_modules/.pnpm 结构 |
| [Resolver](docs/.project/design/resolver.md) | `crates/resolver` | 依赖图构建 |
| [Registry & Fetcher](docs/.project/design/fetcher.md) | `crates/registry`, `crates/fetcher` | HTTP 客户端、tarball 下载 |
| [Lockfile](docs/.project/design/lockfile.md) | `crates/lockfile` | orix-lock.yaml 管理 |
| [Workspace](docs/.project/design/workspace.md) | `crates/workspace` | monorepo 支持 |
| [CLI & Config](docs/.project/design/cli-config.md) | `crates/cli`, `crates/config` | CLI 命令、配置加载 |
| [安装管道](docs/.project/design/core.md) | `crates/core` | 完整安装流程编排 |

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `cargo build` | 构建项目 |
| `cargo test` | 运行测试 |
| `cargo fmt` | 格式化代码 |
| `cargo clippy -- -D warnings` | 运行 clippy，警告视为错误 |
| `cargo doc --no-deps` | 构建文档 |
| `make check` | fmt + clippy + test，等价于 `cargo xtask check` |
| `cargo deny check` | 依赖审计 |
| `cargo machete` | 检测未使用依赖 |

## 编码规范

### Rust 风格

- 遵循 idiomatic Rust。
- fallible API 优先返回 `Result`。
- library crate 使用 `thiserror` 定义结构化错误。
- binary / application 边界使用 `anyhow`。
- library code 避免 `panic!`，除非违反明确不变量。
- `unsafe` 必须隔离、说明原因并覆盖测试。

### 依赖管理

- 所有第三方依赖版本只写在根 `Cargo.toml` 的 `[workspace.dependencies]`。
- 各 crate 的 `Cargo.toml` 使用 `.workspace = true`，不要硬编码版本。
- 新增依赖时先检查 MSRV 影响；项目 MSRV 是 Rust `1.80`。

### Crate 边界

- `cli` 只做参数解析和用户交互，业务逻辑下沉到 library crate。
- `core` 负责编排安装流程，可以依赖其他业务 crate。
- 其他 crate 保持自包含，避免跨层调用。
- public API 保持最小；内部实现优先 `pub(crate)` 或 private。

### 测试

- 单元测试放在对应 `src/` 文件的 `#[cfg(test)] mod tests`。
- 集成测试放在 workspace root 的 `tests/integration.rs` 或对应 crate 的 `tests/`。
- 公共 API 可以在 `src/lib.rs` doc comment 中补 doctest。
- 修复 bug 时优先补回归测试。

## 架构原则

1. 无循环依赖：crate 依赖图必须严格无环。
2. 边界错误有类型：每个 crate 定义自己的错误类型，`core` 统一包装。
3. I/O 异步，CPU 同步：registry 和下载走 `tokio`，哈希和文件链接使用阻塞 I/O。
4. 文件级 CAS 去重：store 按文件内容去重，不按包整体去重。
5. 平台感知文件系统：Windows 目录链接优先 junction；硬链接优先复制并自动回退。
6. Lockfile 优先可重现性：`--frozen-lockfile` 必须保证跨机器一致安装。

## 当前状态

已完成：

- Phase 1：本地 manifest + CLI
- Phase 2：registry resolver
- Phase 3：fetch + extract
- Phase 4：CAS store
- Phase 5：linker
- Phase 6：lockfile
- Phase 7：workspace

待完善：

- Phase 8：lifecycle scripts 执行、`orix run` 命令。
- Phase 9：peerDependencies 完整算法、pnpm-lock.yaml 兼容、`patch` 协议、catalogs / deploy。

高风险区域：

- peerDependencies 解析：算法复杂度最高。
- lifecycle scripts：涉及执行安全边界。
- pnpm-lock.yaml 兼容：格式复杂且需要兼容真实生态。
- Windows symlink / junction：平台行为差异大。
- registry auth token：安全敏感，避免日志泄露。

## Skills

项目常用技能：

| Skill | 使用场景 |
| --- | --- |
| `/setup-rust` | 首次配置 Rust 工作区上下文 |
| `/tdd` | 按 red-green-refactor 开发功能或修 bug |
| `/diagnose` | 诊断编译错误、panic、错误输出或性能回退 |
| `/zoom-out` | 理解某段代码在整体架构中的位置 |
| `/improve-codebase-architecture` | 寻找架构改进和重构机会 |
| `/grill-me` | 实现前压力测试方案 |
| `/caveman` | 极简回复模式 |
| `/rust-async-patterns` | Tokio async、channels、graceful shutdown |
| `/rust-best-practices` | Rust 借用、错误、泛型、clippy 等最佳实践 |
| `/rust-security` | cargo-audit、cargo-deny、RUSTSEC、fuzzing、Miri |
| `/tdd-rust` | Rust 专用 TDD 流程 |

## CI/CD

- CI 覆盖 Ubuntu、Windows、macOS，见 `.github/workflows/ci.yml`。
- 安全扫描每周运行。
- tag push 触发多平台 binary release。
