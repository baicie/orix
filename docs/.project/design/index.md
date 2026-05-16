# 设计概览

本文档包含 orix 各核心组件的详细设计文档。

## Crate 架构

```
crates/
├── cli              # 命令行入口（clap, anyhow）
├── config           # .npmrc / registry / proxy / store 配置
├── manifest         # package.json 解析和验证
├── resolver         # semver 解析，依赖图构建
├── registry         # npm registry API（packument, tarball 元数据）
├── fetcher          # tarball 下载，完整性验证，解压
├── store            # 内容可寻址包缓存
├── lockfile         # orix-lock.yaml 读写/diff
├── linker           # node_modules/.pnpm 结构 + 符号链接/硬链接生成
├── workspace        # workspace 发现，pnpm-workspace.yaml 解析
├── domain           # 共享领域类型
├── utils            # 共享工具函数
├── macros           # 过程宏预留
└── core             # 安装管道编排
```

## 组件设计文档

| 文档 | Crate | 描述 |
|------|-------|------|
| [CAS Store](./store.md) | `crates/store` | 内容可寻址全局包缓存。按 SHA-256 哈希对文件去重。 |
| [Linker](./linker.md) | `crates/linker` | 构建 `node_modules/.pnpm/` 和符号链接树。平台感知（Windows junction 回退）。 |
| [Resolver](./resolver.md) | `crates/resolver` | 通过 npm registry 查询将 `package.json` 依赖转换为完全解析的依赖图。 |
| [Lockfile](./lockfile.md) | `crates/lockfile` | 管理 `orix-lock.yaml`：读写、diff、冻结 lockfile 验证。 |
| [Registry & Fetcher](./fetcher.md) | `crates/registry` + `crates/fetcher` | npm registry API 的 HTTP 客户端；tarball 下载、完整性验证、解压。 |
| [Workspace](./workspace.md) | `crates/workspace` | Monorepo 支持：`pnpm-workspace.yaml` 解析，`workspace:*` 协议解析。 |
| [CLI & Config](./cli-config.md) | `crates/cli` + `crates/config` | CLI 命令（`install`、`add`、`remove`、`store`），从 `.npmrc` 和环境变量加载配置。 |
| [安装管道](./core.md) | `crates/core` | 编排完整安装流程：resolve → fetch → store → link → lockfile。 |
| [Manifest、Domain 与 Utils](./manifest-domain-utils.md) | `crates/manifest` + `crates/domain` + `crates/utils` + `crates/macros` | `package.json` 输入模型、共享领域类型、integrity/parser、路径工具和过程宏边界。 |
| [测试、集成与质量](./testing-quality.md) | `tests/` + CI | 测试分层、端到端 fixture、Windows 链接测试、`make check` 和质量工具。 |

## TODO 覆盖情况

| TODO Phase | 设计覆盖 |
| --- | --- |
| Phase 1 本地 manifest + CLI | [Manifest、Domain 与 Utils](./manifest-domain-utils.md)、[CLI & Config](./cli-config.md) |
| Phase 2 Registry Resolver | [Resolver](./resolver.md)、[Registry & Fetcher](./fetcher.md) |
| Phase 3 Fetcher | [Registry & Fetcher](./fetcher.md) |
| Phase 4 CAS Store | [CAS Store](./store.md) |
| Phase 5 Linker | [Linker](./linker.md) |
| Phase 6 Lockfile | [Lockfile](./lockfile.md) |
| Phase 7 Workspace | [Workspace](./workspace.md) |
| Phase 8 Pipeline | [安装管道](./core.md) |
| Phase 9 Config | [CLI & Config](./cli-config.md) |
| Phase 10 Utils & Macros | [Manifest、Domain 与 Utils](./manifest-domain-utils.md) |
| Phase 11 Domain | [Manifest、Domain 与 Utils](./manifest-domain-utils.md) |
| Phase 12 测试 | [测试、集成与质量](./testing-quality.md) |
| Phase 13 集成 & 质量 | [测试、集成与质量](./testing-quality.md) |

## 设计原则

### 1. 无循环依赖

crate 依赖图严格无环。`core` 导入所有其他 crate；其他 crate 自包含。

### 2. 错误在边界处有类型

每个 crate 定义自己的 `thiserror` 枚举。`core` 将它们包装在 `CoreError` 中，提供统一的公共 API。

### 3. I/O 异步，CPU 同步

registry 调用和文件下载使用 `tokio`。哈希计算和符号链接创建使用阻塞 I/O，通过 `tokio::fs` 或 `std::fs` 适当处理。

### 4. 内容可寻址去重

store 在**文件内容**级别而非包级别去重。共享相同 `package.json` 内容的两个包共享一份物理文件。

### 5. 平台感知的文件系统操作

Windows junction 优先于符号链接用于目录。硬链接优先于复制，自动回退。

### 6. Lockfile 优先的可重现性

`--frozen-lockfile` 保证跨机器逐位完全相同的安装。lockfile 格式分离 `importers`（每个项目，合并友好）和 `packages`（共享，去重）。

## 关键数据流

### 安装流程

```
package.json → Manifest
                     → Resolver → DependencyGraph
                                          → Fetcher → tarball
                                                         → Store (CAS 导入)
                                                         → Lockfile.write()
                                                         → Linker → node_modules/
```

### 更新流程（add/remove）

```
package.json 变更 → Lockfile.read() → Lockfile.update()
                                          → Store.import()
                                          → Linker.relink()
                                          → Lockfile.write()
```

### 冻结安装流程（CI）

```
lockfile.read() → DependencyGraph (来自 lockfile)
                         → Fetcher (优先缓存)
                         → Store (验证存在)
                         → Linker
```

## 推迟到第三阶段+

- 完整的 peerDependencies 解析算法
- 生命周期脚本（preinstall、postinstall 等）
- pnpm-lock.yaml 导入/导出
- `patch` 协议
- catalogs
