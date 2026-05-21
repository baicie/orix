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

### 代码体量与模块组织

单文件过长会拖慢 review、增加合并冲突、让 Agent 难以定位逻辑。以下规范适用于所有 `crates/` 与 `xtask/` 源码；**新增或修改代码时必须遵守**，存量超大文件按「技术债清单」渐进拆分，不要在一次无关 PR 里做大范围搬家。

#### 行数阈值

以 `wc -l` 统计**不含空行也可，但全文件行数为准**，阈值对 `.rs` 与测试文件同样适用：

| 级别 | 行数 | 要求 |
| --- | --- | --- |
| 健康 | ≤ 400 | 默认目标；新逻辑优先落在此范围 |
| 警戒 | 401–600 | 禁止继续堆叠无关职责；新增功能应新建子模块 |
| 超标 | 601–800 | 仅允许修 bug / 小改动；同一 PR 若新增 >50 行须同步拆出子模块 |
| 禁止增长 | > 800 | **不得再增大**；任何非 trivial 改动应顺带拆模块，或单独开 refactor PR |

例外（仍不得突破「禁止增长」）：

- `#[cfg(test)]` 可迁到 `tests/` 或 `src/**/tests.rs`，不必留在实现文件里撑行数。
- 纯数据表（如大 `match` 错误码映射）可单独 `errors.rs` / `codes.rs`。
- 平台分支（`cfg(windows)` 等）优先 `platform/windows.rs`，不要和通用逻辑混在一个 2000 行文件里。

#### `lib.rs` 职责

- `lib.rs` 只做：crate 文档、`mod` 声明、`pub use` 重导出、少量 crate 级类型（如 `LinkReport`）。
- **目标 ≤ 150 行**；业务实现放在命名子模块，禁止把整 crate 写进单个 `lib.rs`（`domain` 当前形态视为待拆分债务）。

#### 何时拆文件

满足任一条件就应拆模块，而不是继续加长当前文件：

1. 文件已超过「警戒」且本次改动会新增独立概念（新命令、新子算法、新错误族）。
2. 同一文件内出现 **2 个以上** 互不调用的顶层类型/流程（例如「安装管道」与「store 维护」混在 `pipeline.rs`）。
3. `impl Struct` 块合计超过 **250 行** → 按职责拆 `impl` 到 `struct/action.rs` 或 `linker/layout.rs` 等。
4. 私有辅助函数超过 **15 个** 且可按主题分组（路径解析、平台链接、semver 解析等）。
5. 需要 `#[cfg(...)]` 的大段平台代码（>80 行）→ 独立 `platform/` 子模块。

#### 推荐目录形态

保持与现有 crate 边界一致，优先**按领域概念**而非按「第几段代码」切文件：

```txt
crates/<crate>/src/
├── lib.rs              # mod + pub use，薄
├── error.rs            # 本 crate 错误类型（可选）
├── <feature>/          # 功能域目录
│   ├── mod.rs
│   ├── types.rs        # 纯类型 + Serialize/Deserialize
│   └── ops.rs          # 主要算法 / I/O
└── platform/
    ├── mod.rs
    ├── unix.rs
    └── windows.rs
```

命名约定：

- 类型与解析：`types.rs`、`parse.rs`、`validate.rs`
- 管道阶段：`install.rs`、`fetch.rs`、`link.rs`（由 `pipeline/mod.rs` 编排）
- 对外 API 仍在 `lib.rs` 通过 `pub use` 保持兼容，**避免** 为了拆分而改动其他 crate 的 import 路径，除非刻意做 breaking refactor。

#### Agent 工作流

开始改某个 crate 前：

1. 对目标文件执行 `wc -l`；若 > 600，先判断新逻辑应落在哪个**新子模块**，而不是写在原文件末尾。
2. 只改与任务相关的模块；禁止借机重排无关代码。
3. 若必须在超标文件上修 bug，改动应**局部**；新增测试优先放在新文件或 `tests/`。
4. 拆分后运行 `make check`；public API 行为不变，必要时在 `lib.rs` 补 `pub use` 保持原导出路径。
5. 拆分 PR 与功能 PR 分离：功能 PR 小、可 review；纯搬家 refactor 单独提交，便于 bisect。

#### 测试与超大文件

- 实现文件内 `mod tests` 超过 **200 行** → 迁到 `src/<module>/tests.rs`（`#[cfg(test)] mod tests;`）或 `crates/<crate>/tests/*.rs`。
- 集成测试 `cli/tests/integration.rs` 可按命令拆成 `tests/install.rs`、`tests/store.rs` 等，由 `tests/common/mod.rs` 共享 harness。

#### 技术债清单（超标文件，禁止继续增长）

`crates/` 内业务源码已无超过 800 行的单文件。新增功能**不得**再往下列警戒文件堆逻辑，应在新子模块实现并由 `mod` 引入：

| 文件 | 行数 | 说明 |
| --- | ---: | --- |
| `crates/lockfile/src/pnpm.rs` | ~537 | pnpm 兼容；格式变更保持独立 |
| `crates/core/src/script/runner.rs` | ~509 | 警戒区 |
| `crates/resolver/src/resolver/mod.rs` | ~500 | 警戒区 |

**已完成模块化（勿再向旧单文件路径堆代码）**：`domain/`、`lockfile/{types,ops,resolve}.rs`、`store/`、`resolver/`、`core/pipeline/install/{mod,fast_path,fetch_phase,resolve,link,finish,workspace_link}.rs`、`core/script/`、`linker/linker/{layout,link_graph/{mod,import_files,bins,symlinks}}` + `linker_platform.rs`、`linker/tests/{helpers,link_graph_*,layout,platform}.rs`、`cli/{args,cmd/{store,install,script,lockfile},reporter/frame/{mod,sections,steps,util,tests}}.rs`、`config/{types,load,platform}.rs`、`workspace/workspace/{types,discover,cycles}.rs`。

相关设计参考：[Manifest、Domain 与 Utils](docs/.project/design/manifest-domain-utils.md) 中 domain 按概念拆分的边界说明。

#### 自检命令

```bash
# 列出 crates 内最大的 Rust 源文件
find crates -name '*.rs' -exec wc -l {} + | sort -n | tail -20
```

CI 暂不强制行数门禁；评审与 Agent 以本表为准。若后续接入自动化，在 `xtask` 增加 `size-check` 再更新本节。

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
